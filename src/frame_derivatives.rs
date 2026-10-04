//! Calibrated and registered copies of catalogued lights.
//!
//! Other software (PixInsight's WBPP, Siril, Astro Pixel Processor) writes
//! calibrated and registered copies of each light. PSF Guard pairs such a
//! copy with the light it came from instead of cataloguing it as a second
//! frame, and records the pair here, beside the scheduler tables and never in
//! them. See `docs/design/calibrated-subs.md`.

use crate::image_io::{FrameClass, FrameKind, KindEvidence, Producer};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// Shape of `psf_guard_frame_derivative`.
///
/// Bump this when the table changes and add the step that gets an older
/// catalog here to [`ensure_schema`], following the calibration library's
/// ladder (`calibration::CALIBRATION_SCHEMA_VERSION`).
///
/// 1: the original table.
pub const DERIVATIVE_SCHEMA_VERSION: i64 = 1;

/// Create the table, or check that an existing one is a shape this build
/// knows. A catalog a newer build has upgraded is refused rather than
/// written to.
pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS psf_guard_frame_derivative_schema (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            version   INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS psf_guard_frame_derivative (
            derivative_uuid     TEXT PRIMARY KEY,
            acquired_image_guid TEXT NOT NULL,
            kind                TEXT NOT NULL CHECK (kind IN ('calibrated', 'registered')),
            primary_source      INTEGER NOT NULL DEFAULT 0,
            file_name           TEXT NOT NULL,
            source_tail         TEXT,
            size                INTEGER,
            mtime               INTEGER,
            producer            TEXT NOT NULL,
            evidence            TEXT NOT NULL,
            created_at          INTEGER NOT NULL,
            updated_at          INTEGER NOT NULL,
            UNIQUE (acquired_image_guid, kind)
        );
        "#,
    )
    .context("creating the PSF Guard frame derivative table")?;
    conn.execute(
        "INSERT INTO psf_guard_frame_derivative_schema (singleton, version)
             VALUES (1, ?1)
             ON CONFLICT(singleton) DO NOTHING",
        [DERIVATIVE_SCHEMA_VERSION],
    )?;
    let version = recorded_schema_version(conn)?;
    if version > DERIVATIVE_SCHEMA_VERSION {
        anyhow::bail!(
            "PSF Guard frame derivative schema version {version} is newer than this build \
             supports (expected at most {DERIVATIVE_SCHEMA_VERSION}); upgrade PSF Guard"
        );
    }
    Ok(())
}

/// Whether the catalog has the table at all. A catalog no pairing has
/// touched has none, and holds no derivatives.
pub fn schema_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master
         WHERE type = 'table' AND name = 'psf_guard_frame_derivative'",
        [],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .unwrap_or(false)
}

fn recorded_schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT version FROM psf_guard_frame_derivative_schema WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?)
}

/// One calibrated or registered copy of a light.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DerivativeRecord {
    /// Stable identity, and the key sync matches records by.
    pub derivative_uuid: String,
    /// `acquiredimage.guid` of the light this is a copy of.
    pub acquired_image_guid: String,
    pub kind: FrameKind,
    /// The light row's own file is this derivative: the catalog had no raw
    /// frame for it.
    pub primary_source: bool,
    /// Basename, found again through the local directory tree.
    pub file_name: String,
    /// The last path segments where the file was found, to tell two files of
    /// one name apart.
    pub source_tail: Option<String>,
    pub size: Option<i64>,
    /// Modification time in epoch seconds.
    pub mtime: Option<i64>,
    pub producer: Producer,
    pub evidence: KindEvidence,
    pub created_at: i64,
    pub updated_at: i64,
}

fn kind_from_str(text: &str) -> FrameKind {
    match text {
        "registered" => FrameKind::Registered,
        _ => FrameKind::Calibrated,
    }
}

fn producer_from_str(text: &str) -> Producer {
    match text {
        "pixinsight" => Producer::Pixinsight,
        "siril" => Producer::Siril,
        "app" => Producer::App,
        _ => Producer::Unknown,
    }
}

fn evidence_from_str(text: &str) -> KindEvidence {
    match text {
        "header" => KindEvidence::Header,
        "name" => KindEvidence::Name,
        _ => KindEvidence::None,
    }
}

fn evidence_str(evidence: KindEvidence) -> &'static str {
    match evidence {
        KindEvidence::None => "none",
        KindEvidence::Header => "header",
        KindEvidence::Name => "name",
    }
}

/// Record a pairing. A light holds at most one copy of each kind: a newer
/// file of the same kind takes over the existing record, keeping its
/// `derivative_uuid` so synced catalogs follow the change instead of growing
/// a second record.
pub fn record_pairing(conn: &Connection, record: &DerivativeRecord) -> Result<()> {
    anyhow::ensure!(
        record.kind.is_derivative(),
        "only calibrated or registered copies are recorded, not {}",
        record.kind.as_str()
    );
    conn.execute(
        "INSERT INTO psf_guard_frame_derivative
             (derivative_uuid, acquired_image_guid, kind, primary_source, file_name,
              source_tail, size, mtime, producer, evidence, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(acquired_image_guid, kind) DO UPDATE SET
             primary_source = excluded.primary_source,
             file_name = excluded.file_name,
             source_tail = excluded.source_tail,
             size = excluded.size,
             mtime = excluded.mtime,
             producer = excluded.producer,
             evidence = excluded.evidence,
             updated_at = excluded.updated_at",
        params![
            record.derivative_uuid,
            record.acquired_image_guid,
            record.kind.as_str(),
            record.primary_source,
            record.file_name,
            record.source_tail,
            record.size,
            record.mtime,
            record.producer.as_str(),
            evidence_str(record.evidence),
            record.created_at,
            record.updated_at,
        ],
    )
    .context("recording a frame derivative")?;
    Ok(())
}

/// Every copy recorded for one light, calibrated first.
pub fn derivatives_for_light(
    conn: &Connection,
    acquired_image_guid: &str,
) -> Result<Vec<DerivativeRecord>> {
    if !schema_exists(conn) {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT derivative_uuid, acquired_image_guid, kind, primary_source, file_name,
                source_tail, size, mtime, producer, evidence, created_at, updated_at
         FROM psf_guard_frame_derivative
         WHERE acquired_image_guid = ?1
         ORDER BY kind",
    )?;
    let rows = statement.query_map([acquired_image_guid], |row| {
        Ok(DerivativeRecord {
            derivative_uuid: row.get(0)?,
            acquired_image_guid: row.get(1)?,
            kind: kind_from_str(&row.get::<_, String>(2)?),
            primary_source: row.get(3)?,
            file_name: row.get(4)?,
            source_tail: row.get(5)?,
            size: row.get(6)?,
            mtime: row.get(7)?,
            producer: producer_from_str(&row.get::<_, String>(8)?),
            evidence: evidence_from_str(&row.get::<_, String>(9)?),
            created_at: row.get(10)?,
            updated_at: row.get(11)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Forget a light's copy of one kind. The file itself is left alone.
pub fn forget(conn: &Connection, acquired_image_guid: &str, kind: FrameKind) -> Result<bool> {
    if !schema_exists(conn) {
        return Ok(false);
    }
    Ok(conn.execute(
        "DELETE FROM psf_guard_frame_derivative
         WHERE acquired_image_guid = ?1 AND kind = ?2",
        params![acquired_image_guid, kind.as_str()],
    )? > 0)
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

/// A capture time to the precision its writer recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureInstant {
    /// Whole seconds since the epoch, UTC.
    pub seconds: i64,
    /// The fraction, when the writer recorded one.
    pub nanos: Option<u32>,
}

/// Two writers of one frame may round a fraction differently; anything
/// closer than this is the same exposure.
const SUB_SECOND_TOLERANCE_NS: i64 = 1_000_000;

impl CaptureInstant {
    /// Parse a FITS `DATE-OBS` or a Target Scheduler `ExposureStartTime`:
    /// `YYYY-MM-DDTHH:MM:SS[.fraction][Z]`, read as UTC.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_matches('\'').trim();
        let text = text.strip_suffix('Z').unwrap_or(text);
        let (whole, fraction) = match text.split_once('.') {
            Some((whole, fraction)) => (whole, Some(fraction)),
            None => (text, None),
        };
        let seconds = chrono::NaiveDateTime::parse_from_str(whole, "%Y-%m-%dT%H:%M:%S")
            .ok()?
            .and_utc()
            .timestamp();
        let nanos = match fraction {
            None => None,
            Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
                let padded: String = digits
                    .chars()
                    .chain(std::iter::repeat('0'))
                    .take(9)
                    .collect();
                Some(padded.parse().ok()?)
            }
            Some(_) => return None,
        };
        Some(Self { seconds, nanos })
    }

    /// The same exposure start: equal to the second, and within a
    /// millisecond where both recorded a fraction.
    pub fn same_as(self, other: Self) -> bool {
        match (self.nanos, other.nanos) {
            (Some(left), Some(right)) => {
                let left = self.seconds as i128 * 1_000_000_000 + left as i128;
                let right = other.seconds as i128 * 1_000_000_000 + right as i128;
                (left - right).abs() <= SUB_SECOND_TOLERANCE_NS as i128
            }
            _ => self.seconds == other.seconds,
        }
    }
}

/// What pairing compares between a light and a derivative.
#[derive(Debug, Clone, Default)]
pub struct FrameIdentity {
    pub captured_at: Option<CaptureInstant>,
    pub exposure_s: Option<f64>,
    pub filter: Option<String>,
    /// `OBJECT` for a file; the target's name for a catalogued light.
    pub target: Option<String>,
    pub camera: Option<String>,
    /// File name without its extension.
    pub stem: String,
}

/// A catalogued light pairing may attach a derivative to.
#[derive(Debug, Clone)]
pub struct LightCandidate<K> {
    pub key: K,
    pub identity: FrameIdentity,
}

/// A calibrated or registered file looking for its light.
#[derive(Debug, Clone)]
pub struct DerivativeCandidate<D> {
    pub key: D,
    pub class: FrameClass,
    pub identity: FrameIdentity,
    /// Modification time, to prefer the newer of two copies.
    pub mtime: Option<i64>,
    /// The stem of the file the tool says this was made from, when it keeps
    /// such a record (Siril's `<sequence>_conversion.txt`; see
    /// [`parse_siril_conversion_log`]).
    pub source_stem: Option<String>,
}

/// What became of one derivative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairOutcome<K, D> {
    Paired {
        derivative: D,
        light: K,
    },
    /// Another copy of the same kind was preferred for this light.
    Superseded {
        derivative: D,
        light: K,
        by: D,
    },
    /// Matches several lights and no file name settles it.
    Ambiguous {
        derivative: D,
        lights: Vec<K>,
    },
    /// No catalogued light matches.
    Unmatched {
        derivative: D,
    },
}

const EXPOSURE_TOLERANCE_S: f64 = 0.001;

fn same_text(left: &str, right: &str) -> bool {
    let fold = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_uppercase()
    };
    fold(left) == fold(right)
}

/// Whether a derivative is a copy of this light. Calibration tools copy the
/// acquisition keywords forward, so every one recorded on both sides must
/// agree; the capture time and exposure are required, because a name or a
/// nearby time alone never pairs two frames.
pub fn identities_match(light: &FrameIdentity, derivative: &FrameIdentity) -> bool {
    let (Some(light_time), Some(derivative_time)) = (light.captured_at, derivative.captured_at)
    else {
        return false;
    };
    if !light_time.same_as(derivative_time) {
        return false;
    }
    let (Some(light_exposure), Some(derivative_exposure)) =
        (light.exposure_s, derivative.exposure_s)
    else {
        return false;
    };
    if (light_exposure - derivative_exposure).abs() > EXPOSURE_TOLERANCE_S {
        return false;
    }
    // A filter on one side only is a different frame: an OSC light has no
    // filter, and neither does its copy.
    let filters_agree = match (&light.filter, &derivative.filter) {
        (Some(left), Some(right)) => crate::utils::filter_names_match(left, right),
        (None, None) => true,
        _ => false,
    };
    if !filters_agree {
        return false;
    }
    let agrees_where_both_say = |left: &Option<String>, right: &Option<String>| match (left, right)
    {
        (Some(left), Some(right)) => same_text(left, right),
        _ => true,
    };
    agrees_where_both_say(&light.target, &derivative.target)
        && agrees_where_both_say(&light.camera, &derivative.camera)
}

/// Whether the derivative's own name, or the source its tool recorded, names
/// this light.
fn names_light<D>(derivative: &DerivativeCandidate<D>, light: &FrameIdentity) -> bool {
    let base = crate::image_io::derivative_base_stem(&derivative.identity.stem);
    base == light.stem || derivative.source_stem.as_deref() == Some(light.stem.as_str())
}

/// Pair each derivative with the light it was made from.
///
/// A derivative known only by its name (no processing mark in its header)
/// pairs only with a light its name or recorded source names. Siril's
/// registration writes no mark, and a registered *stack* still carries its
/// reference light's time, exposure and filter: matching keywords alone
/// would pair the stack with that light.
///
/// A derivative that matches several lights pairs only when exactly one of
/// them shares its base name (`..._0042_c.xisf` with `..._0042.fits`);
/// otherwise it is ambiguous and left alone. When several copies of one kind
/// match one light (WBPP can leave a `_r` and a `_c_r` of the same frame),
/// the one that was calibrated wins, then the newer, then the first by key;
/// the rest are superseded.
pub fn pair<K, D>(
    lights: &[LightCandidate<K>],
    derivatives: &[DerivativeCandidate<D>],
) -> Vec<PairOutcome<K, D>>
where
    K: Clone + Eq + std::hash::Hash,
    D: Clone + Ord,
{
    let mut outcomes = Vec::new();
    // (light index, kind) -> derivative indices that chose it.
    let mut claims: std::collections::HashMap<(usize, FrameKind), Vec<usize>> =
        std::collections::HashMap::new();
    for (index, derivative) in derivatives.iter().enumerate() {
        if !derivative.class.kind.is_derivative() {
            continue;
        }
        let named_only = derivative.class.evidence != KindEvidence::Header;
        let matches: Vec<usize> = lights
            .iter()
            .enumerate()
            .filter(|(_, light)| identities_match(&light.identity, &derivative.identity))
            .filter(|(_, light)| !named_only || names_light(derivative, &light.identity))
            .map(|(light_index, _)| light_index)
            .collect();
        let chosen = match matches.as_slice() {
            [] => {
                outcomes.push(PairOutcome::Unmatched {
                    derivative: derivative.key.clone(),
                });
                continue;
            }
            [only] => *only,
            several => {
                let named: Vec<usize> = several
                    .iter()
                    .copied()
                    .filter(|light_index| names_light(derivative, &lights[*light_index].identity))
                    .collect();
                if let [only] = named.as_slice() {
                    *only
                } else {
                    outcomes.push(PairOutcome::Ambiguous {
                        derivative: derivative.key.clone(),
                        lights: several.iter().map(|i| lights[*i].key.clone()).collect(),
                    });
                    continue;
                }
            }
        };
        claims
            .entry((chosen, derivative.class.kind))
            .or_default()
            .push(index);
    }
    let mut claims: Vec<_> = claims.into_iter().collect();
    claims.sort_by_key(|((light_index, kind), _)| (*light_index, kind.as_str()));
    for ((light_index, _), mut contenders) in claims {
        contenders.sort_by(|left, right| {
            let left = &derivatives[*left];
            let right = &derivatives[*right];
            right
                .class
                .includes_calibration
                .cmp(&left.class.includes_calibration)
                .then(right.mtime.cmp(&left.mtime))
                .then(left.key.cmp(&right.key))
        });
        let light = lights[light_index].key.clone();
        let winner = derivatives[contenders[0]].key.clone();
        outcomes.push(PairOutcome::Paired {
            derivative: winner.clone(),
            light: light.clone(),
        });
        for loser in &contenders[1..] {
            outcomes.push(PairOutcome::Superseded {
                derivative: derivatives[*loser].key.clone(),
                light: light.clone(),
                by: winner.clone(),
            });
        }
    }
    outcomes
}

/// Read Siril's `<sequence>_conversion.txt`: one `'source' -> 'frame'` line
/// per file it converted into the sequence. Returns (source path, sequence
/// file name) pairs. This is the only record of which file a Siril sequence
/// frame came from: Siril renames on conversion, and its later steps
/// (`pp_`, `r_`) only add prefixes to the sequence name.
pub fn parse_siril_conversion_log(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (source, frame) = line.split_once("' -> '")?;
            let source = source.trim().strip_prefix('\'')?;
            let frame = frame.trim().strip_suffix('\'')?;
            Some((source.to_string(), frame.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant(text: &str) -> CaptureInstant {
        CaptureInstant::parse(text).unwrap_or_else(|| panic!("{text} should parse"))
    }

    fn identity(time: &str, filter: &str, stem: &str) -> FrameIdentity {
        FrameIdentity {
            captured_at: Some(instant(time)),
            exposure_s: Some(75.0),
            filter: Some(filter.into()),
            target: Some("NGC 6543".into()),
            camera: Some("ZWO ASI2600MM Pro".into()),
            stem: stem.into(),
        }
    }

    fn class(kind: FrameKind, includes_calibration: bool) -> FrameClass {
        FrameClass {
            kind,
            evidence: KindEvidence::Header,
            producer: Producer::Pixinsight,
            includes_calibration,
        }
    }

    fn light(key: u32, time: &str, filter: &str, stem: &str) -> LightCandidate<u32> {
        LightCandidate {
            key,
            identity: identity(time, filter, stem),
        }
    }

    fn derivative(
        key: &'static str,
        kind: FrameKind,
        includes_calibration: bool,
        time: &str,
        stem: &str,
    ) -> DerivativeCandidate<&'static str> {
        DerivativeCandidate {
            key,
            class: class(kind, includes_calibration),
            identity: identity(time, "B", stem),
            mtime: Some(100),
            source_stem: None,
        }
    }

    #[test]
    fn capture_times_compare_to_the_precision_both_recorded() {
        // Target Scheduler and the FITS header of one real C925 frame.
        let catalog = instant("2026-06-07T10:20:00.6712661Z");
        let header = instant("'2026-06-07T10:20:00.6712661'");
        assert!(catalog.same_as(header));
        assert!(catalog.same_as(instant("2026-06-07T10:20:00.672")));
        assert!(catalog.same_as(instant("2026-06-07T10:20:00")));
        assert!(!catalog.same_as(instant("2026-06-07T10:20:00.700")));
        assert!(!catalog.same_as(instant("2026-06-07T10:20:01")));
        assert_eq!(CaptureInstant::parse("yesterday"), None);
        assert_eq!(CaptureInstant::parse("2026-06-07T10:20:00.x"), None);
    }

    #[test]
    fn a_calibrated_copy_pairs_with_its_raw_light() {
        let lights = [
            light(1, "2026-06-07T10:20:00.6712661", "B", "frame_0115"),
            light(2, "2026-06-07T10:21:21.1000000", "B", "frame_0116"),
        ];
        let derivatives = [derivative(
            "frame_0115_c.xisf",
            FrameKind::Calibrated,
            true,
            "2026-06-07T10:20:00.6712661",
            "frame_0115_c",
        )];
        assert_eq!(
            pair(&lights, &derivatives),
            [PairOutcome::Paired {
                derivative: "frame_0115_c.xisf",
                light: 1
            }]
        );
    }

    #[test]
    fn every_recorded_acquisition_keyword_must_agree() {
        let raw = identity("2026-06-07T10:20:00", "B", "frame");
        let mut other = raw.clone();
        other.filter = Some("R".into());
        assert!(!identities_match(&raw, &other));
        let mut other = raw.clone();
        other.exposure_s = Some(300.0);
        assert!(!identities_match(&raw, &other));
        let mut other = raw.clone();
        other.target = Some("M 31".into());
        assert!(!identities_match(&raw, &other));
        let mut other = raw.clone();
        other.camera = Some("QHY268M".into());
        assert!(!identities_match(&raw, &other));
        let mut other = raw.clone();
        other.filter = None;
        assert!(!identities_match(&raw, &other), "filter on one side only");

        // Case and spacing do not make a different filter or target, and a
        // keyword one side never recorded rules nothing out.
        let mut other = raw.clone();
        other.filter = Some(" b ".into());
        other.target = Some("ngc  6543".into());
        other.camera = None;
        assert!(identities_match(&raw, &other));

        // A name alone never pairs.
        let mut other = raw.clone();
        other.captured_at = None;
        assert!(!identities_match(&raw, &other));
        let mut other = raw.clone();
        other.exposure_s = None;
        assert!(!identities_match(&raw, &other));
    }

    #[test]
    fn the_base_name_settles_two_lights_with_one_timestamp() {
        // Two lights recorded with whole-second times in the same second.
        let lights = [
            light(1, "2026-06-07T10:20:00", "B", "frame_0115"),
            light(2, "2026-06-07T10:20:00", "B", "frame_0116"),
        ];
        let named = [derivative(
            "a",
            FrameKind::Calibrated,
            true,
            "2026-06-07T10:20:00",
            "frame_0116_c_cc",
        )];
        assert_eq!(
            pair(&lights, &named),
            [PairOutcome::Paired {
                derivative: "a",
                light: 2
            }]
        );
        let unnamed = [derivative(
            "b",
            FrameKind::Calibrated,
            true,
            "2026-06-07T10:20:00",
            "pp_light_00001",
        )];
        assert_eq!(
            pair(&lights, &unnamed),
            [PairOutcome::Ambiguous {
                derivative: "b",
                lights: vec![1, 2]
            }]
        );
    }

    #[test]
    fn the_calibrated_registered_copy_wins_over_a_registered_raw() {
        // WBPP left both `_0115_r.xisf` and `_0115_c_r.xisf` for one light on
        // the C925 NGC 6543 run.
        let lights = [light(1, "2026-06-07T10:20:00.6712661", "B", "frame_0115")];
        let time = "2026-06-07T10:20:00.6712661";
        let derivatives = [
            derivative(
                "frame_0115_r",
                FrameKind::Registered,
                false,
                time,
                "frame_0115_r",
            ),
            derivative(
                "frame_0115_c_r",
                FrameKind::Registered,
                true,
                time,
                "frame_0115_c_r",
            ),
            derivative(
                "frame_0115_c",
                FrameKind::Calibrated,
                true,
                time,
                "frame_0115_c",
            ),
        ];
        let mut outcomes = pair(&lights, &derivatives);
        outcomes.sort_by_key(|outcome| format!("{outcome:?}"));
        assert_eq!(
            outcomes,
            [
                PairOutcome::Paired {
                    derivative: "frame_0115_c",
                    light: 1
                },
                PairOutcome::Paired {
                    derivative: "frame_0115_c_r",
                    light: 1
                },
                PairOutcome::Superseded {
                    derivative: "frame_0115_r",
                    light: 1,
                    by: "frame_0115_c_r"
                },
            ]
        );
    }

    #[test]
    fn the_newer_of_two_equal_copies_wins() {
        let lights = [light(1, "2026-06-07T10:20:00", "B", "frame")];
        let mut older = derivative(
            "older",
            FrameKind::Calibrated,
            true,
            "2026-06-07T10:20:00",
            "frame_c",
        );
        older.mtime = Some(10);
        let mut newer = derivative(
            "newer",
            FrameKind::Calibrated,
            true,
            "2026-06-07T10:20:00",
            "frame_c",
        );
        newer.mtime = Some(20);
        let outcomes = pair(&lights, &[older, newer]);
        assert!(outcomes.contains(&PairOutcome::Paired {
            derivative: "newer",
            light: 1
        }));
    }

    #[test]
    fn unmatched_and_non_derivative_files() {
        let lights = [light(1, "2026-06-07T10:20:00", "B", "frame")];
        let stray = derivative(
            "stray",
            FrameKind::Calibrated,
            true,
            "2026-06-08T01:00:00",
            "x_c",
        );
        let raw = derivative("raw", FrameKind::Raw, false, "2026-06-07T10:20:00", "frame");
        assert_eq!(
            pair(&lights, &[stray, raw]),
            [PairOutcome::Unmatched {
                derivative: "stray"
            }]
        );
    }

    #[test]
    fn a_name_only_derivative_needs_a_name_link() {
        // Siril registered an APP stack: the result has no stack count and
        // carries the reference light's time, exposure and filter
        // (Radian61 NGC 7000, 2025-07-18).
        let time = "2025-07-19T06:25:31.457";
        let lights = [light(1, time, "B", "2025-07-18_23-25-31_Ha_0001")];
        let mut stack = derivative(
            "r_alpha-o3_00001",
            FrameKind::Registered,
            false,
            time,
            "r_alpha-o3_00001",
        );
        stack.class.evidence = KindEvidence::Name;
        stack.class.producer = Producer::Siril;
        assert_eq!(
            pair(&lights, std::slice::from_ref(&stack)),
            [PairOutcome::Unmatched {
                derivative: "r_alpha-o3_00001"
            }]
        );

        // The same frame from a sequence Siril converted from that light.
        stack.source_stem = Some("2025-07-18_23-25-31_Ha_0001".into());
        assert_eq!(
            pair(&lights, &[stack]),
            [PairOutcome::Paired {
                derivative: "r_alpha-o3_00001",
                light: 1
            }]
        );

        // A WBPP name carries the light's own stem.
        let mut wbpp = derivative(
            "w",
            FrameKind::Calibrated,
            true,
            time,
            "2025-07-18_23-25-31_Ha_0001_c",
        );
        wbpp.class.evidence = KindEvidence::Name;
        assert_eq!(
            pair(&lights, &[wbpp]),
            [PairOutcome::Paired {
                derivative: "w",
                light: 1
            }]
        );
    }

    #[test]
    fn siril_conversion_logs_name_each_sequence_frame_source() {
        let log = "'/Volumes/astrobin/2025-07-18/Target-Hydrogen-alpha-session_1.fits' -> 'alpha-o3_00001.fit'\n\
                   '/Volumes/astrobin/2025-07-18/Target-Oxygen_III-session_1.fits' -> 'alpha-o3_00002.fit'\n\
                   not a conversion line\n";
        assert_eq!(
            parse_siril_conversion_log(log),
            [
                (
                    "/Volumes/astrobin/2025-07-18/Target-Hydrogen-alpha-session_1.fits".to_string(),
                    "alpha-o3_00001.fit".to_string()
                ),
                (
                    "/Volumes/astrobin/2025-07-18/Target-Oxygen_III-session_1.fits".to_string(),
                    "alpha-o3_00002.fit".to_string()
                ),
            ]
        );
    }

    fn record(guid: &str, kind: FrameKind, file_name: &str, uuid: &str) -> DerivativeRecord {
        DerivativeRecord {
            derivative_uuid: uuid.into(),
            acquired_image_guid: guid.into(),
            kind,
            primary_source: false,
            file_name: file_name.into(),
            source_tail: Some("calibrated/Light_B".into()),
            size: Some(104_419_840),
            mtime: Some(1_750_000_000),
            producer: Producer::Pixinsight,
            evidence: KindEvidence::Header,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn a_newer_copy_of_one_kind_takes_over_the_record() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(!schema_exists(&conn));
        assert!(derivatives_for_light(&conn, "light").unwrap().is_empty());
        ensure_schema(&conn).unwrap();
        ensure_schema(&conn).unwrap();

        record_pairing(
            &conn,
            &record("light", FrameKind::Registered, "f_r.xisf", "u-r"),
        )
        .unwrap();
        record_pairing(
            &conn,
            &record("light", FrameKind::Calibrated, "f_c.xisf", "u-c"),
        )
        .unwrap();
        let mut replacement = record("light", FrameKind::Calibrated, "f_c_cc.xisf", "u-other");
        replacement.updated_at = 2;
        record_pairing(&conn, &replacement).unwrap();

        let stored = derivatives_for_light(&conn, "light").unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].kind, FrameKind::Calibrated);
        assert_eq!(stored[0].file_name, "f_c_cc.xisf");
        assert_eq!(
            stored[0].derivative_uuid, "u-c",
            "identity survives a newer file"
        );
        assert_eq!(stored[0].updated_at, 2);
        assert_eq!(
            stored[1],
            record("light", FrameKind::Registered, "f_r.xisf", "u-r")
        );

        assert!(forget(&conn, "light", FrameKind::Registered).unwrap());
        assert!(!forget(&conn, "light", FrameKind::Registered).unwrap());
        assert_eq!(derivatives_for_light(&conn, "light").unwrap().len(), 1);
        assert!(record_pairing(&conn, &record("light", FrameKind::Raw, "f.fits", "u")).is_err());
    }

    #[test]
    fn a_table_from_a_newer_build_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        conn.execute(
            "UPDATE psf_guard_frame_derivative_schema SET version = ?1",
            [DERIVATIVE_SCHEMA_VERSION + 1],
        )
        .unwrap();
        assert!(ensure_schema(&conn).is_err());
    }
}
