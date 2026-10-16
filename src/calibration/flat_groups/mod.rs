//! Flat groups: which nights' flats may share a master, measured rather
//! than guessed from the clock.
//!
//! A background pass reduces every flat session to a response, compares
//! sessions that could share a master, and asks the lights of sessions shot
//! during a change what they carry ([`job`]). The catalog keeps the numbers
//! ([`store`]); grouping, change events and where each light stands are
//! worked out from them on load ([`model`]), so changing a limit regroups
//! without reading a pixel. Matching then gives each light flats from its
//! own side of every change, or the closest stand-in the rig's measurements
//! allow (see `LibrarySnapshot::select_with`).
//!
//! See `docs/design/flat-grouping.md`.

mod epochs;
pub mod family_job;
pub mod family_store;
pub mod job;
pub mod measure;
#[cfg(test)]
mod selection_tests;
pub mod store;
#[cfg(test)]
pub(crate) mod testing;

pub use measure::{choice, evidence, lights, metric, model};
pub use seiza_calibration::LightRotation;

use anyhow::Result;
use model::{AngleTerm, Book, Event, Placement, Side};
use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

/// The cache category response planes live in, below the database's cache
/// folder.
pub const CACHE_CATEGORY: &str = "calibration-flat-responses";

static RESPONSE_DIRS: LazyLock<RwLock<HashMap<PathBuf, PathBuf>>> = LazyLock::new(Default::default);

fn catalog_key(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Say where a catalog's response planes are kept, so a master build that
/// has only the catalog's connection can find them.
pub fn register_response_dir(catalog: &Path, dir: PathBuf) {
    RESPONSE_DIRS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(catalog_key(catalog), dir);
}

/// Where a catalog's response planes are kept, when someone said.
pub fn response_dir(catalog: &Path) -> Option<PathBuf> {
    RESPONSE_DIRS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&catalog_key(catalog))
        .cloned()
}

pub fn response_path(dir: &Path, response_key: &str) -> PathBuf {
    dir.join(format!("{response_key}.resp"))
}

/// Read a response plane back from the cache.
pub fn read_plane(dir: &Path, response_key: &str) -> Option<metric::Plane> {
    let bytes = std::fs::read(response_path(dir, response_key)).ok()?;
    metric::decode_plane(&bytes)
}

/// The book for a catalog, built once per state of its rows and settings.
pub fn load_book(conn: &Connection) -> Result<Arc<Book>> {
    /// Per catalog: the digest the book was built from, and the book.
    type Memo = HashMap<String, (String, Arc<Book>)>;
    static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Default::default);
    if !store::tables_exist(conn) {
        return Ok(Arc::new(Book::default()));
    }
    let profile_rigs = ProfileRigs::read(conn);
    let mut hasher = Sha256::new();
    profile_rigs.digest(&mut hasher);
    store::digest_rows(conn, &mut hasher)?;
    // The family evidence and the lights' rotator offsets the measuring job
    // stored: a new solve reaches the book through them, never through a
    // look at the cache here.
    family_store::digest(conn, &mut hasher)?;
    hasher.update(
        super::rules::rule_book()
            .digest()
            .unwrap_or_default()
            .as_bytes(),
    );
    // Lights are only added and removed, never shot again: their summary,
    // taken inside SQLite, says whether imaging sessions could differ, so a
    // match per light does not read every light to find the book unchanged.
    hasher.update(lights_summary(conn).as_bytes());
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let catalog = conn.path().unwrap_or("").to_string();
    if let Some((known, book)) = MEMO
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&catalog)
        && *known == digest
    {
        return Ok(Arc::clone(book));
    }
    let lights = imaging_lights(conn)?;
    let mut book = Book::build(
        store::load_responses(conn)?,
        store::load_comparisons(conn)?,
        store::load_evidence(conn, evidence::EVIDENCE_ALGORITHM_VERSION)?,
        model::imaging_sessions(lights, |profile| profile_rigs.rigs_of(profile)),
        rig_limits,
    );
    book.epochs = epochs::epochs(&family_store::load_angles(conn)?);
    for row in family_store::load(conn)? {
        book.family_evidence
            .entry(row.session_key.clone())
            .or_default()
            .push(row);
    }
    let book = Arc::new(book);
    MEMO.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(catalog, (digest, Arc::clone(&book)));
    Ok(book)
}

/// Work out and store every light's rotator offset (see [`epochs`]): the
/// measuring job's step, reading the metadata of the lights and the
/// solves in the catalog's cache folder `cache` that changed since.
pub(crate) fn gather_angles(conn: &Connection, cache: Option<&Path>) -> Result<usize> {
    let profile_rigs = ProfileRigs::read(conn);
    epochs::gather(conn, cache, |profile| profile_rigs.rigs_of(profile))
}

/// Which rigs each N.I.N.A. profile shot with, from the library's
/// bindings. A light's imaging session places only that rig's lights.
struct ProfileRigs {
    bound: BTreeMap<String, Vec<String>>,
    /// The one rig of a library that has only one: it owns every light a
    /// profile with no binding shot.
    only: Option<String>,
}

impl ProfileRigs {
    fn read(conn: &Connection) -> Self {
        let mut bound: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Ok(mut statement) =
            conn.prepare("SELECT profile_id, rig_uuid FROM psf_guard_rig_binding ORDER BY rig_uuid")
            && let Ok(rows) = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
        {
            for (profile, rig) in rows.flatten() {
                bound.entry(profile).or_default().push(rig);
            }
        }
        let rigs: Vec<String> = conn
            .prepare("SELECT rig_uuid FROM psf_guard_rig")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        Self {
            bound,
            only: (rigs.len() == 1).then(|| rigs[0].clone()),
        }
    }

    fn rigs_of(&self, profile: Option<&str>) -> Vec<String> {
        profile
            .and_then(|profile| self.bound.get(profile).cloned())
            .unwrap_or_else(|| self.only.iter().cloned().collect())
    }

    fn digest(&self, hasher: &mut Sha256) {
        for (profile, rigs) in &self.bound {
            hasher.update(profile.as_bytes());
            for rig in rigs {
                hasher.update(rig.as_bytes());
            }
        }
        hasher.update(self.only.as_deref().unwrap_or("").as_bytes());
    }
}

/// How many lights there are, the last one's id, and the sums of their
/// capture times and profile lengths: what changes when any light that
/// makes imaging sessions comes, goes or moves.
fn lights_summary(conn: &Connection) -> String {
    let has_profile = conn
        .prepare("SELECT profileId FROM acquiredimage LIMIT 0")
        .is_ok();
    let sql = if has_profile {
        "SELECT COUNT(*), COALESCE(MAX(Id), 0), TOTAL(acquireddate),
                TOTAL(LENGTH(COALESCE(profileId, '')))
         FROM acquiredimage WHERE acquireddate IS NOT NULL"
    } else {
        "SELECT COUNT(*), COALESCE(MAX(Id), 0), TOTAL(acquireddate), 0
         FROM acquiredimage WHERE acquireddate IS NOT NULL"
    };
    conn.query_row(sql, [], |row| {
        Ok(format!(
            "{}:{}:{}:{}",
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, f64>(3)?
        ))
    })
    .unwrap_or_default()
}

/// Every light's id, profile and capture time, for imaging sessions.
fn imaging_lights(conn: &Connection) -> Result<Vec<(i64, Option<String>, i64)>> {
    let has_profile = conn
        .prepare("SELECT profileId FROM acquiredimage LIMIT 0")
        .is_ok();
    let sql = if has_profile {
        "SELECT Id, profileId, acquireddate FROM acquiredimage WHERE acquireddate IS NOT NULL"
    } else {
        "SELECT Id, NULL, acquireddate FROM acquiredimage WHERE acquireddate IS NOT NULL"
    };
    let Ok(mut statement) = conn.prepare(sql) else {
        return Ok(Vec::new());
    };
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The grouping limits a rig's settings put in force.
pub fn limits_of(rules: &super::Rules) -> model::Limits {
    model::Limits {
        measured: rules.flat_grouping == super::rules::FlatGrouping::Measured,
        dust: rules.flat_split_dust_pct / 100.0,
        broad: rules.flat_split_pattern_pct / 100.0,
        window: rules.flat_session_seconds as i64,
        rotation: rules.coherence_tolerances().rotation_deg,
    }
}

/// [`limits_of`] the rig's settings in force.
pub fn rig_limits(rig_uuid: &str) -> model::Limits {
    limits_of(&super::rules::rules_for_rig(rig_uuid))
}

/// A change as a light's account names it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventNote {
    pub key: String,
    /// The window it happened in, Unix seconds.
    pub start: i64,
    pub end: i64,
    /// Fractions of the background.
    pub dust: f64,
    pub broad: f64,
    pub camera_side: bool,
    pub filters: Vec<String>,
    /// "new dust: 2 shadows near (1,460, 3,570)".
    pub what: String,
}

impl EventNote {
    pub fn of(event: &Event) -> Self {
        Self {
            key: event.key.clone(),
            start: event.start,
            end: event.end,
            dust: event.dust,
            broad: event.broad,
            camera_side: event.camera_side,
            filters: event.filters.iter().cloned().collect(),
            what: describe_spots(&event.spots, event.dust, event.broad),
        }
    }
}

/// The spots of a change in a few words.
pub fn describe_spots(spots: &[metric::Spot], dust: f64, broad: f64) -> String {
    use metric::SpotKind;
    let strong: Vec<&metric::Spot> = spots
        .iter()
        .filter(|spot| f64::from(spot.peak) >= dust * 0.5)
        .collect();
    if strong.is_empty() {
        return format!("the light pattern changed by {:.1}%", broad * 100.0);
    }
    let kind = if strong.iter().any(|s| s.kind == SpotKind::Moved) {
        "dust moved"
    } else if strong.iter().all(|s| s.kind == SpotKind::Gone) {
        "dust gone"
    } else {
        "new dust"
    };
    let top = strong[0];
    let shadows = if strong.len() == 1 {
        "1 shadow".to_string()
    } else {
        format!("{} shadows", strong.len())
    };
    format!("{kind}: {shadows} near ({:.0}, {:.0})", top.x, top.y)
}

/// The correction a light's flat master takes: the flats at its angle,
/// from the other side of a change, carried across it by the change's map.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FlatCorrection {
    pub event: String,
    /// The before and after sessions the map is measured from.
    pub before_response: String,
    pub after_response: String,
    /// The side the light is on: `before` divides the flats by the map,
    /// `after` multiplies.
    pub light_side: Side,
    /// Key of the correction, for master and stack identities.
    pub key: String,
}

/// Why a light's flats are the ones they are.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(tag = "basis", rename_all = "snake_case")]
pub enum FlatBasis {
    /// No measured change touches the light: the nearest flats, as always.
    #[default]
    Nearest,
    /// Flats from the light's own side of each change near it.
    SameSide {
        event: EventNote,
        placement: Placement,
        /// Matching flats left out for being on the other side.
        left_out: usize,
    },
    /// None at its angle on its side; flats from the nearest other angle on
    /// its side, which this rig's measurements allow.
    OtherAngle {
        event: EventNote,
        placement: Placement,
        rotation: Option<f64>,
        angle_term: AngleTerm,
        /// The rotation tolerance the stacker must hold this light to so
        /// those flats pass: the angle between them and the light, plus the
        /// rig's own tolerance. Absent when either angle is unknown.
        rotation_tolerance_deg: Option<f64>,
    },
    /// None on its side at any allowed angle; flats at its angle from the
    /// other side, carried across by the change's map.
    Corrected {
        event: EventNote,
        placement: Placement,
        correction: FlatCorrection,
    },
    /// Its flats are all from the other side of a change, and nothing can
    /// stand in.
    NoSameSide {
        event: EventNote,
        placement: Placement,
    },
    /// Shot while a change happened; nothing tells which side it is on.
    Undetermined {
        event: EventNote,
        placement: Placement,
        /// What another change it does stand clear of chose, such as
        /// another angle's flats or a correction: its flats and gate are
        /// that one's, and the light keeps its Check for this one.
        #[serde(skip_serializing_if = "Option::is_none")]
        then: Option<Box<FlatBasis>>,
    },
}

impl FlatBasis {
    /// Whether the light is worth a look for it.
    pub fn flags(&self) -> bool {
        matches!(
            self,
            FlatBasis::NoSameSide { .. } | FlatBasis::Undetermined { .. }
        )
    }

    /// The change it chose around, when one touches the light.
    pub fn event(&self) -> Option<&EventNote> {
        match self {
            FlatBasis::Nearest => None,
            FlatBasis::SameSide { event, .. }
            | FlatBasis::OtherAngle { event, .. }
            | FlatBasis::Corrected { event, .. }
            | FlatBasis::NoSameSide { event, .. }
            | FlatBasis::Undetermined { event, .. } => Some(event),
        }
    }

    /// Its name, as the light's explanation gives it.
    pub fn name(&self) -> &'static str {
        match self {
            FlatBasis::Nearest => "nearest",
            FlatBasis::SameSide { .. } => "same_side",
            FlatBasis::OtherAngle { .. } => "other_angle",
            FlatBasis::Corrected { .. } => "corrected",
            FlatBasis::NoSameSide { .. } => "no_same_side",
            FlatBasis::Undetermined { .. } => "undetermined",
        }
    }

    /// The rotation tolerance this light's masters must carry, when its
    /// flats are from another angle.
    pub fn rotation_tolerance(&self) -> Option<f64> {
        match self {
            FlatBasis::OtherAngle {
                rotation_tolerance_deg,
                ..
            } => *rotation_tolerance_deg,
            FlatBasis::Undetermined {
                then: Some(then), ..
            } => then.rotation_tolerance(),
            _ => None,
        }
    }

    /// Whether an earlier change already left the light with flats from
    /// its other side and nothing to stand in.
    pub fn lacks_same_side(&self) -> bool {
        match self {
            FlatBasis::NoSameSide { .. } => true,
            FlatBasis::Undetermined {
                then: Some(then), ..
            } => then.lacks_same_side(),
            _ => false,
        }
    }

    pub fn correction(&self) -> Option<&FlatCorrection> {
        match self {
            FlatBasis::Corrected { correction, .. } => Some(correction),
            FlatBasis::Undetermined {
                then: Some(then), ..
            } => then.correction(),
            _ => None,
        }
    }

    /// `next`, chosen around a later change, in place of this basis. An
    /// undetermined placement keeps its Check whatever comes after it: the
    /// choice rides along inside it.
    pub fn followed_by(self, next: FlatBasis) -> FlatBasis {
        match (self, next) {
            (
                FlatBasis::Undetermined {
                    event,
                    placement,
                    then,
                },
                next,
            ) if !matches!(next, FlatBasis::Undetermined { .. }) => FlatBasis::Undetermined {
                event,
                placement,
                then: Some(Box::new(match then {
                    Some(earlier) => earlier.followed_by(next),
                    None => next,
                })),
            },
            // A second undetermined change, after another choice: the
            // light is flagged, and keeps what was chosen.
            (
                earlier,
                FlatBasis::Undetermined {
                    event, placement, ..
                },
            ) => FlatBasis::Undetermined {
                event,
                placement,
                then: match earlier {
                    FlatBasis::Nearest => None,
                    FlatBasis::Undetermined { then, .. } => then,
                    other => Some(Box::new(other)),
                },
            },
            (_, next) => next,
        }
    }
}

/// The response family a light's flats come from, when grouping measured
/// it: which family, how it was chosen, and the gate a stack holds the
/// light to for it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FlatFamily {
    /// The family's key (`model::Family::key`).
    pub key: String,
    /// Its filter, as the flats name it, and measured angle range: "Ha at
    /// 72.6° & 82.2°".
    pub label: String,
    /// How it was chosen.
    #[serde(flatten)]
    pub basis: lights::FamilyBasis,
    /// The family its lights pointed to, when that move was not confirmed
    /// (`disputed`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointed: Option<String>,
    /// Whether the family has flats at the light's recorded angle.
    pub at_recorded: bool,
    /// The light's recorded angle, and how far the rotator's epochs turn
    /// it into the family's nearest session.
    pub light_angle: f64,
    pub shift: f64,
    /// The candidates within the angle prior, nearest first, as labels.
    pub candidates: Vec<String>,
    /// How the stacker's gate reads the light's angle: as recorded, or as
    /// the angle of the flats its master is built from when measurement
    /// gave it flats at another angle (seiza-calibration's
    /// `LightRotation`).
    pub light_rotation: LightRotation,
}

impl FlatFamily {
    pub fn of(book: &Book, choice: &lights::FamilyChoice) -> Self {
        let label = |family: usize| {
            format!(
                "{} at {}",
                book.families[family]
                    .filter
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("flats"),
                book.angle_label(family)
            )
        };
        Self {
            key: book.families[choice.family].key.clone(),
            label: label(choice.family),
            basis: choice.basis.clone(),
            pointed: match &choice.basis {
                lights::FamilyBasis::Disputed { pointed, .. } => Some(label(*pointed)),
                _ => None,
            },
            at_recorded: choice.at_recorded,
            light_angle: choice.light_angle,
            shift: choice.shift,
            candidates: choice.candidates.iter().map(|f| label(*f)).collect(),
            light_rotation: LightRotation::Recorded,
        }
    }

    /// Whether the light's own pixels chose it.
    pub fn by_pixels(&self) -> bool {
        matches!(self.basis, lights::FamilyBasis::Pixels { .. })
    }

    /// Whether the light is worth a look for it: its pixels pointed to
    /// another family and nothing confirmed it.
    pub fn flags(&self) -> bool {
        matches!(self.basis, lights::FamilyBasis::Disputed { .. })
    }
}

/// The lights and targets of a family's evidence, in words: "8 of M 27".
fn of_targets(lights: usize, targets: &[String]) -> String {
    if targets.is_empty() {
        format!("{lights} of its targets")
    } else {
        format!("{lights} of {}", targets.join(", "))
    }
}

/// How a light's response family was chosen, for its Calibration panel:
/// `None` when the flats are simply the ones at its recorded angle.
pub fn describe_family(family: &FlatFamily) -> Option<String> {
    use lights::FamilyBasis;
    let angle = if family.shift.abs() >= 0.5 {
        format!(
            "{:.1}° recorded, {:.1}° in the flats' rotator epoch",
            family.light_angle,
            (family.light_angle + family.shift).rem_euclid(360.0)
        )
    } else {
        format!("{:.1}° recorded", family.light_angle)
    };
    let others = family
        .candidates
        .iter()
        .filter(|candidate| **candidate != family.label)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match &family.basis {
        FamilyBasis::Angle => family.label.contains(" & ").then(|| {
            format!(
                "Flats of the {} family, whose flats at those angles measure alike ({angle})",
                family.label
            )
        }),
        FamilyBasis::Pixels {
            lights,
            targets,
            moved: false,
            ..
        } => Some(format!(
            "Chosen by its own pixels: its imaging session's lights ({}) follow the flats of \
             the {} family, at its recorded angle, rather than {others} ({angle})",
            of_targets(*lights, targets),
            family.label
        )),
        FamilyBasis::Pixels {
            lights,
            targets,
            corroborated,
            ..
        } => Some(format!(
            "Chosen by its own pixels, and {} agree: its imaging session's lights ({}) follow \
             the flats of the {} family rather than {others} ({angle})",
            corroborated.as_deref().unwrap_or("its other evidence"),
            of_targets(*lights, targets),
            family.label
        )),
        FamilyBasis::Nearest { undecided } => Some(format!(
            "Flats of the {} family, at its recorded angle: {others} could stand in within \
             {:.0}° of its angle, and {} ({angle})",
            family.label,
            lights::ANGLE_PRIOR_DEG,
            if *undecided {
                "its imaging session's lights could not tell them apart"
            } else {
                "its lights have not been read to tell them apart yet"
            }
        )),
        FamilyBasis::Disputed { .. } => describe_family_flag(family),
    }
}

/// Why a light is flagged for its flat family, or `None` when it is not:
/// its lights pointed to another family than the one at its recorded
/// angle, and nothing confirmed the move.
pub fn describe_family_flag(family: &FlatFamily) -> Option<String> {
    let lights::FamilyBasis::Disputed {
        lights, targets, ..
    } = &family.basis
    else {
        return None;
    };
    let kept = if family.at_recorded {
        format!(
            "it keeps the flats of the {} family, at its recorded angle",
            family.label
        )
    } else {
        "it keeps the flats matched by its recorded angle".to_string()
    };
    Some(format!(
        "Its imaging session's lights ({}) follow the flats of the {} family, at another \
         angle than the {:.1}° it recorded, but neither its other filters nor the rotator's \
         epochs confirm the move: {kept}",
        of_targets(*lights, targets),
        family.pointed.as_deref().unwrap_or("another"),
        family.light_angle
    ))
}

/// The key of one correction: both sessions, the side, and the pedestals
/// taken out of them, since the map is made with those out.
pub fn correction_key(
    before: &str,
    after: &str,
    side: Side,
    pedestals: (Option<f64>, Option<f64>),
) -> String {
    let digest = Sha256::digest(
        format!(
            "{}\u{1f}{before}\u{1f}{after}\u{1f}{side:?}\u{1f}{:?}\u{1f}{:?}",
            metric::COMPARE_ALGORITHM_VERSION,
            pedestals.0,
            pedestals.1
        )
        .as_bytes(),
    );
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn when(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| at.to_string())
}

fn day(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| at.to_string())
}

/// A change with its window, for a note that lists the changes in force:
/// "new dust: 1 shadow near (1423, 3545), between 2026-09-17 12:27 UTC and
/// 2026-09-21 12:28 UTC, seen in B, G, R".
pub fn describe_change(event: &EventNote, rules: &super::Rules) -> String {
    format!(
        "{}, between {} and {}, {}",
        describe_event(event, rules),
        when(event.start),
        when(event.end),
        scope(event)
    )
}

/// The change in words with its size against the limit: "new dust: 2
/// shadows near (1460, 3570), up to 1.6% of the sky background (limit 1%)".
pub fn describe_event(event: &EventNote, rules: &super::Rules) -> String {
    let limit = |key: &str, value: f64| format!("limit {}%{}", value, rules.set_by_suffix(key));
    if event.dust > 0.0 {
        format!(
            "{}, up to {:.1}% of the sky background ({})",
            event.what,
            event.dust * 100.0,
            limit("flat_split_dust_pct", rules.flat_split_dust_pct)
        )
    } else {
        format!(
            "{} ({})",
            event.what,
            limit("flat_split_pattern_pct", rules.flat_split_pattern_pct)
        )
    }
}

fn scope(event: &EventNote) -> String {
    if event.camera_side {
        format!(
            "seen in {} flats, so on the camera side and in every filter and angle",
            event.filters.join(", ").to_uppercase()
        )
    } else {
        format!("in {} flats", event.filters.join(", ").to_uppercase())
    }
}

/// Why a light is flagged for its flats, or `None` when it is not.
pub fn describe_flag(basis: &FlatBasis, rules: &super::Rules) -> Option<String> {
    match basis {
        FlatBasis::Undetermined {
            event, placement, ..
        } => {
            let pixels = match placement {
                Placement::Undetermined {
                    pixels: Some(note), ..
                } => format!("; {note}"),
                _ => String::new(),
            };
            Some(format!(
                "Shot during a measured change: {}, between {} and {}, {}. Nothing tells which \
                 side it is on{pixels}, so its flats may not match",
                describe_event(event, rules),
                when(event.start),
                when(event.end),
                scope(event)
            ))
        }
        FlatBasis::NoSameSide { event, placement } => {
            let side = match placement.side() {
                Some(Side::Before) => "before",
                _ => "after",
            };
            Some(format!(
                "No flats from {side} the change at its angle: {}, between {} and {}. Its \
                 flats are from the other side and may not match. Retake flats at this angle, \
                 or let flats from another angle stand in once this rig's flats are measured \
                 alike across angles",
                describe_event(event, rules),
                day(event.start),
                day(event.end)
            ))
        }
        _ => None,
    }
}

/// How a light's flats were chosen, for its Calibration panel: evidence and
/// prediction named apart.
pub fn describe_basis(basis: &FlatBasis, rules: &super::Rules) -> Option<String> {
    let placed = |placement: &Placement| match placement {
        Placement::Time { side } => format!(
            "shot {} the change by its time",
            if *side == Side::Before {
                "before"
            } else {
                "after"
            }
        ),
        Placement::Session { side, start, end } => format!(
            "its imaging session ({} to {}) {} the change's window, and a change does not \
             land inside one session",
            when(*start),
            when(*end),
            if *side == Side::Before {
                "began before"
            } else {
                "ran past"
            }
        ),
        Placement::Pixels {
            side,
            targets,
            filters,
            lights,
        } => format!(
            "its session's own lights ({lights} of {} in {}) {} the change",
            targets.join(", "),
            filters.join(", "),
            if *side == Side::Before {
                "lack"
            } else {
                "carry"
            }
        ),
        Placement::Undetermined { .. } => "nothing tells which side it is on".into(),
    };
    match basis {
        FlatBasis::Nearest => None,
        FlatBasis::SameSide {
            event,
            placement,
            left_out,
        } => Some(format!(
            "Flats from its side of a measured change ({}): {}. {left_out} matching flat(s) \
             from the other side left out",
            describe_event(event, rules),
            placed(placement)
        )),
        FlatBasis::OtherAngle {
            event,
            placement,
            rotation,
            angle_term,
            ..
        } => Some(format!(
            "No flats from its side of a measured change ({}) at its angle: {}. Flats from \
             its side at {} stand in; this rig's flats differ by at most {:.1}% broad and \
             {:.1}% dust between angles",
            describe_event(event, rules),
            placed(placement),
            rotation.map_or_else(|| "another angle".to_string(), |r| format!("{r:.1}°")),
            angle_term.broad * 100.0,
            angle_term.dust * 100.0
        )),
        FlatBasis::Corrected {
            event, placement, ..
        } => Some(format!(
            "No flats from its side of a measured change ({}) at any angle that may stand in: \
             {}. Its flats at its angle, from the other side, are carried across with the \
             change's map",
            describe_event(event, rules),
            placed(placement)
        )),
        FlatBasis::Undetermined {
            then: Some(then), ..
        } => {
            let flag = describe_flag(basis, rules);
            match (flag, describe_basis(then, rules)) {
                (Some(flag), Some(then)) => Some(format!("{flag}. {then}")),
                (flag, then) => flag.or(then),
            }
        }
        FlatBasis::NoSameSide { .. } | FlatBasis::Undetermined { .. } => {
            describe_flag(basis, rules)
        }
    }
}

#[cfg(test)]
mod basis_tests {
    use super::*;

    fn note(key: &str) -> EventNote {
        EventNote {
            key: key.into(),
            start: 0,
            end: 1,
            what: String::new(),
            dust: 0.02,
            broad: 0.0,
            camera_side: true,
            filters: Vec::new(),
        }
    }

    fn undetermined(key: &str) -> FlatBasis {
        FlatBasis::Undetermined {
            event: note(key),
            placement: Placement::Undetermined {
                session: None,
                pixels: None,
            },
            then: None,
        }
    }

    fn other_angle(key: &str) -> FlatBasis {
        FlatBasis::OtherAngle {
            event: note(key),
            placement: Placement::Time { side: Side::Before },
            rotation: Some(3.2),
            angle_term: AngleTerm::default(),
            rotation_tolerance_deg: Some(112.6),
        }
    }

    #[test]
    fn an_undetermined_change_keeps_its_check_whatever_another_chooses() {
        // Undetermined first, then another angle for a later change.
        let basis = undetermined("a").followed_by(other_angle("b"));
        assert!(basis.flags());
        assert_eq!(basis.rotation_tolerance(), Some(112.6));
        // Another angle first, then shot during a later change.
        let basis = other_angle("a").followed_by(undetermined("b"));
        assert!(basis.flags());
        assert_eq!(basis.rotation_tolerance(), Some(112.6));
        // Nothing undetermined: the later choice stands alone.
        assert_eq!(
            FlatBasis::Nearest.followed_by(other_angle("b")),
            other_angle("b")
        );
    }
}

#[cfg(test)]
mod memo_tests {
    use super::*;

    #[test]
    fn the_book_is_built_again_only_when_a_light_or_a_row_changes() {
        let temp = tempfile::tempdir().unwrap();
        let conn = Connection::open(temp.path().join("catalog.sqlite")).unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        crate::calibration::ensure_schema(&conn).unwrap();
        store::add_tables(&conn).unwrap();
        let first = load_book(&conn).unwrap();
        assert!(
            Arc::ptr_eq(&first, &load_book(&conn).unwrap()),
            "nothing changed"
        );
        conn.execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
             gradingStatus, metadata) VALUES (1, 1, 1, 200, 'Ha', 0, '{}')",
            [],
        )
        .unwrap();
        let second = load_book(&conn).unwrap();
        assert!(
            !Arc::ptr_eq(&first, &second),
            "a new light can make a session"
        );
        assert!(Arc::ptr_eq(&second, &load_book(&conn).unwrap()));
    }
}
