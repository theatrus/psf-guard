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

pub mod job;
pub mod measure;
#[cfg(test)]
mod selection_tests;
pub mod store;
#[cfg(test)]
pub(crate) mod testing;

pub use measure::{evidence, metric, model};

use anyhow::Result;
use model::{AngleTerm, Book, Event, Placement, Side};
use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
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
    let lights = imaging_lights(conn)?;
    let mut hasher = Sha256::new();
    store::digest_rows(conn, &mut hasher)?;
    hasher.update(
        super::rules::rule_book()
            .digest()
            .unwrap_or_default()
            .as_bytes(),
    );
    for (id, profile, at) in &lights {
        hasher.update(id.to_le_bytes());
        hasher.update(profile.as_deref().unwrap_or("").as_bytes());
        hasher.update(at.to_le_bytes());
    }
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
    let book = Arc::new(Book::build(
        store::load_responses(conn)?,
        store::load_comparisons(conn)?,
        store::load_evidence(conn, evidence::EVIDENCE_ALGORITHM_VERSION)?,
        model::imaging_sessions(lights),
        rig_limits,
    ));
    MEMO.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(catalog, (digest, Arc::clone(&book)));
    Ok(book)
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

    pub fn correction(&self) -> Option<&FlatCorrection> {
        match self {
            FlatBasis::Corrected { correction, .. } => Some(correction),
            _ => None,
        }
    }
}

pub fn correction_key(before: &str, after: &str, side: Side) -> String {
    let digest = Sha256::digest(
        format!(
            "{}\u{1f}{before}\u{1f}{after}\u{1f}{side:?}",
            metric::COMPARE_ALGORITHM_VERSION
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
        FlatBasis::Undetermined { event, placement } => {
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
        FlatBasis::NoSameSide { .. } | FlatBasis::Undetermined { .. } => {
            describe_flag(basis, rules)
        }
    }
}
