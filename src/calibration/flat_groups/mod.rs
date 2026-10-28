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
pub mod override_store;
#[cfg(test)]
mod override_tests;
#[cfg(test)]
mod selection_tests;
pub mod store;
#[cfg(test)]
pub(crate) mod testing;

pub use measure::{choice, evidence, lights, metric, model, overrides};
pub use seiza_calibration::library::{
    correction_key, describe_spots, edge_words, family_label, place_light, vignetting_leads,
    AlikeRangeNote, ChoiceNote, EventNote, FlatBasis, FlatCorrection, FlatFamily, GroupNote,
};
pub use seiza_calibration::LightRotation;

use super::rules::{SetBy, SettingKey};
use anyhow::Result;
#[cfg(test)]
use model::AngleTerm;
use model::{Book, Event, Placement, Side};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
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

/// Where a session's dust is kept: beside its response plane, named for
/// the dust algorithm, so a new one measures again.
pub fn dust_path(dir: &Path, response_key: &str) -> PathBuf {
    dir.join(format!(
        "{response_key}.dust-v{}.json",
        seiza_calibration::dust::DUST_ALGORITHM_VERSION
    ))
}

/// A flat session's dust, found on the stack of its flats
/// (`seiza_calibration::dust::find_session_dust`), and each flat's view of
/// it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDust {
    /// The dust the stack holds, with in how many flats each feature shows.
    pub map: seiza_calibration::dust::DustMap,
    /// One per flat of the session that has a measurement; none while it
    /// waits for them.
    pub frames: Vec<FrameDustRow>,
    /// Whether the flats' own features have said in how many of them each
    /// feature shows. Until then the stack's dust stands alone.
    #[serde(default)]
    pub persistence: bool,
}

/// One flat's view of its session's dust.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameDustRow {
    pub frame_uuid: String,
    /// Indices into the session's features that this flat shows.
    pub seen: Vec<usize>,
    /// What this flat holds that the stack does not: dew drying, a passing
    /// particle. Not dust of the light path.
    pub transients: Vec<seiza_calibration::dust::DustFeature>,
}

/// Read a session's dust back from the cache.
pub fn read_dust(dir: &Path, response_key: &str) -> Option<SessionDust> {
    let text = std::fs::read_to_string(dust_path(dir, response_key)).ok()?;
    serde_json::from_str(&text).ok()
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
    // A person's splits, joins and choices for lights.
    override_store::digest(conn, &mut hasher)?;
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
    let nights = super::Nights::load(conn);
    let mut book = Book::build_with_overrides(
        store::load_responses(conn)?,
        store::load_comparisons(conn)?,
        store::load_evidence(conn, evidence::EVIDENCE_ALGORITHM_VERSION)?,
        model::imaging_sessions(lights, |profile| profile_rigs.rigs_of(profile)),
        rig_limits,
        override_store::load_group_overrides(conn)?,
        |rig, frame| nights.frame(rig, frame).boundary_seconds,
    );
    book.light_choices = override_store::load_light_choices(conn)?;
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

/// The grouping limits of the rig's settings in force.
pub fn rig_limits(rig_uuid: &str) -> model::Limits {
    super::rules::rules_for_rig(rig_uuid).grouping_limits()
}

/// What a change is, in a few words, by its rig's settings in force.
pub fn describe_what(event: &Event) -> String {
    seiza_calibration::library::describe_what(event, &super::rules::rules_for_rig(&event.rig_uuid))
}

/// A change as a light's account names it, by its rig's settings in force.
pub fn event_note(event: &Event) -> EventNote {
    EventNote::of(event, &super::rules::rules_for_rig(&event.rig_uuid))
}

/// What a person can choose for one light's flats.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChoiceOptions {
    /// Changes it can be put on a side of: those whose window holds it,
    /// those its flats were chosen around, and those with a choice.
    pub changes: Vec<ChangeOption>,
    /// Groups its flats can come from: every group of the families whose
    /// flats pass its other gates, the one it takes now first.
    pub groups: Vec<GroupOption>,
}

/// One change a light can be put on a side of.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChangeOption {
    pub key: String,
    pub start: i64,
    pub end: i64,
    /// "new dust: 1 shadow near (1423, 3545)".
    pub what: String,
    pub camera_side: bool,
    pub filters: Vec<String>,
    /// Where its time, session or pixels put the light, and by which.
    pub side: Option<Side>,
    pub by: &'static str,
    /// The side a person chose, when one did.
    pub chosen: Option<Side>,
    /// The light was shot inside the change's window.
    pub inside: bool,
}

/// One group a light can take.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupOption {
    /// The response key of the group's first session: what a choice names.
    pub response_key: String,
    pub family: String,
    pub first_night: String,
    pub last_night: String,
    pub sessions: usize,
    /// The light's flats come from it now.
    pub current: bool,
    /// A person chose it for the light.
    pub chosen: bool,
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
    let basis = match &family.basis {
        FamilyBasis::Angle => family.label.contains(" & ").then(|| {
            format!(
                "Flats from the {} family: measured responses match across these angles ({angle})",
                family.label
            )
        }),
        FamilyBasis::Pixels {
            lights,
            targets,
            moved: false,
            ..
        } => Some(format!(
            "Selected based on pixel analysis: lights from this imaging session ({}) match \
             the {} flat family at the recorded angle, rather than {others} ({angle})",
            of_targets(*lights, targets),
            family.label
        )),
        FamilyBasis::Pixels {
            lights,
            targets,
            corroborated,
            ..
        } => Some(format!(
            "Reassigned based on pixel analysis, confirmed by {}: lights from this imaging \
             session ({}) match the {} flat family rather than {others} ({angle})",
            corroborated.as_deref().unwrap_or("other evidence"),
            of_targets(*lights, targets),
            family.label
        )),
        FamilyBasis::Nearest { undecided } => Some(format!(
            "Using the {} flat family at the recorded angle. Other candidates within {:.0}°: \
             {others}. {} ({angle})",
            family.label,
            lights::ANGLE_PRIOR_DEG,
            if *undecided {
                "Pixel analysis of this imaging session could not distinguish the candidate families"
            } else {
                "Pixel analysis of this imaging session has not run yet"
            }
        )),
        FamilyBasis::Disputed { .. } => describe_family_flag(family),
        FamilyBasis::Person => Some(format!(
            "Selected by you: flats from the {} family ({angle})",
            family.label
        )),
    };
    let range = family.alike_range.as_ref().map(describe_alike_range);
    match (basis, range) {
        (Some(basis), Some(range)) => Some(format!("{basis}. {range}")),
        (basis, range) => basis.or(range),
    }
}

/// Explain why measured responses allow flats at another angle and state
/// the matching angle used for stack calibration.
pub fn describe_alike_range(range: &AlikeRangeNote) -> String {
    format!(
        "Using flats at {:.1}°: measured responses match across {} ({} pairs over {} nights{}). \
         Stack calibration uses {:.1}° as the matching angle for this light",
        range.flats_angle,
        range.label,
        range.pairs,
        range.nights,
        if range.carried {
            ", measured across a change of dust on the camera side only"
        } else {
            ""
        },
        range.flats_angle
    )
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
            "PSF Guard retains the {} flat family at the recorded angle",
            family.label
        )
    } else {
        "PSF Guard retains the flats matched by the recorded angle".to_string()
    };
    Some(format!(
        "Pixel analysis of this imaging session ({}) suggests the {} flat family, at a different \
         angle from the recorded {:.1}°. Neither analysis of other filters nor the rotator's \
         epochs confirm this reassignment. {kept}",
        of_targets(*lights, targets),
        family.pointed.as_deref().unwrap_or("another"),
        family.light_angle
    ))
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
    let limit =
        |key: SettingKey, value: f64| format!("limit {}%{}", value, rules.who_set_suffix(key));
    if event.by_hand {
        return event.what.clone();
    }
    if event.dust > 0.0 {
        format!(
            "{}, up to {:.1}% of the sky background ({})",
            event.what,
            event.dust * 100.0,
            limit(SettingKey::FlatSplitDustPct, rules.flat_split_dust_pct)
        )
    } else {
        format!(
            "{} ({})",
            event.what,
            limit(
                SettingKey::FlatSplitPatternPct,
                rules.flat_split_pattern_pct
            )
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
                "Captured during the interval of a measured change: {}, between {} and {}, {}. \
                 Evidence does not establish whether this light was captured before or after \
                 the change{pixels}, so its flats may not match",
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
                "No flats from {side} the change at this light's angle: {}, between {} and {}. \
                 The selected flats are from the other side of the change and may not match. \
                 Retake flats at this angle. Flats from another angle can be used once \
                 measurements show matching responses across those angles",
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
            "the capture time places this light {} the change",
            if *side == Side::Before {
                "before"
            } else {
                "after"
            }
        ),
        Placement::Session { side, start, end } => format!(
            "the imaging session ({} to {}) {} the change interval; \
             PSF Guard treats each continuous session as being on one side of the change",
            when(*start),
            when(*end),
            if *side == Side::Before {
                "began before"
            } else {
                "ended after"
            }
        ),
        Placement::Pixels {
            side,
            targets,
            filters,
            lights,
        } => format!(
            "pixel analysis of this imaging session ({lights} lights of {} in {}) \
             {} the measured change",
            targets.join(", "),
            filters.join(", "),
            if *side == Side::Before {
                "does not show"
            } else {
                "shows"
            }
        ),
        Placement::Undetermined { .. } => {
            "evidence does not establish whether this light was captured \
             before or after the change"
                .into()
        }
        Placement::Person { side } => format!(
            "chosen by you: {} the change",
            if *side == Side::Before {
                "before"
            } else {
                "after"
            }
        ),
    };
    match basis {
        FlatBasis::Nearest => None,
        FlatBasis::Group { group } => Some(format!(
            "Selected by you: flat group {}, overriding the selection based on measured changes",
            group.label()
        )),
        FlatBasis::SameSide {
            event,
            placement,
            left_out,
        } => Some(format!(
            "Using flats from the same side of the measured change as this light ({}): {}. \
             Excluded {left_out} matching flat(s) from the other side of the change",
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
            "No flats at this light's angle from the same side of the measured change ({}): {}. \
             Using flats from the same side at {}; measured differences between angles \
             are at most {:.1}% in light pattern and {:.1}% in dust",
            describe_event(event, rules),
            placed(placement),
            rotation.map_or_else(|| "another angle".to_string(), |r| format!("{r:.1}°")),
            angle_term.broad * 100.0,
            angle_term.dust * 100.0
        )),
        FlatBasis::Corrected {
            event, placement, ..
        } => Some(format!(
            "No suitable flats from the same side of the measured change as this light ({}), \
             including at other angles: {}. Using flats at this light's angle from the other \
             side, corrected with the measured change map",
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
            by_hand: false,
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

#[cfg(test)]
mod describe_tests {
    use super::*;
    use seiza_calibration::dust::DustShape;

    fn spot(
        kind: metric::SpotKind,
        shape: DustShape,
        peak: f32,
        within_tolerance: bool,
    ) -> metric::Spot {
        metric::Spot {
            kind,
            darker: kind != metric::SpotKind::Gone,
            x: 3080.0,
            y: 2970.0,
            radius: 295.0,
            area: 270_000.0,
            peak,
            shape,
            hole: None,
            bounds: [0; 4],
            within_tolerance,
            offset: None,
            distance_mm: None,
        }
    }

    #[test]
    fn large_shadows_are_named_by_shape_and_moves_within_focus_travel_are_not_counted() {
        use metric::SpotKind::{Moved, New};
        let disks = [
            spot(New, DustShape::Disk, 0.0035, false),
            spot(New, DustShape::Disk, 0.003, false),
        ];
        assert_eq!(
            describe_spots(&disks, 0.0035, 0.001),
            "new dust: 2 disks near (3080, 2970)"
        );
        let donut = [
            spot(Moved, DustShape::Donut, 0.009, true),
            spot(New, DustShape::Donut, 0.004, false),
        ];
        assert_eq!(
            describe_spots(&donut, 0.004, 0.001),
            "new dust: 1 donut near (3080, 2970)"
        );
        // Only a move within tolerance: nothing to count, so the pattern speaks.
        let only = [spot(Moved, DustShape::Donut, 0.009, true)];
        assert_eq!(
            describe_spots(&only, 0.0, 0.004),
            "the light pattern changed by 0.4%"
        );
    }
}
