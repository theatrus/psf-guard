//! Calibrated and registered copies of catalogued lights.
//!
//! Other software (PixInsight's WBPP, Siril, Astro Pixel Processor) writes
//! calibrated and registered copies of each light. PSF Guard pairs such a
//! copy with the light it came from instead of cataloguing it as a second
//! frame, and records the pair here, beside the scheduler tables and never in
//! them. See `docs/design/calibrated-subs.md`.

use crate::image_io::{FrameClass, FrameKind, KindEvidence, ProcessingSteps, Producer};
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
            steps               TEXT NOT NULL,
            primary_source      INTEGER NOT NULL DEFAULT 0,
            file_name           TEXT NOT NULL,
            source_tail         TEXT,
            size                INTEGER,
            mtime               INTEGER,
            width               INTEGER,
            height              INTEGER,
            producer            TEXT NOT NULL,
            evidence            TEXT NOT NULL,
            created_at          INTEGER NOT NULL,
            updated_at          INTEGER NOT NULL,
            UNIQUE (acquired_image_guid, file_name)
        );
        CREATE INDEX IF NOT EXISTS idx_psf_guard_frame_derivative_light
            ON psf_guard_frame_derivative(acquired_image_guid);
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

/// One calibrated or registered copy of a light. A light can have several,
/// one per [`OptionKey`]: WBPP's `_c`, `_c_cc`, `_r` and `_c_r` of one frame
/// hold different pixels, and each is a view of the light worth keeping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DerivativeRecord {
    /// Stable identity, and the key sync matches records by.
    pub derivative_uuid: String,
    /// `acquiredimage.guid` of the light this is a copy of.
    pub acquired_image_guid: String,
    pub kind: FrameKind,
    /// What was done to it; with the size, this tells two copies apart.
    pub steps: ProcessingSteps,
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
    /// Image size in pixels, when the header gave it. A registration onto
    /// another reference or a crop can change it.
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub producer: Producer,
    pub evidence: KindEvidence,
    pub created_at: i64,
    pub updated_at: i64,
}

/// What makes two copies of one light different options: their steps and
/// their image size. A copy with the same steps and size as a recorded one
/// is a newer file of that option and replaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OptionKey {
    pub steps: ProcessingSteps,
    pub dimensions: Option<(i64, i64)>,
}

impl OptionKey {
    /// The same option. An unknown size matches any, so a header that left
    /// the size out does not split one option in two.
    pub fn same_as(self, other: Self) -> bool {
        self.steps == other.steps
            && match (self.dimensions, other.dimensions) {
                (Some(left), Some(right)) => left == right,
                _ => true,
            }
    }
}

impl DerivativeRecord {
    pub fn option(&self) -> OptionKey {
        OptionKey {
            steps: self.steps,
            dimensions: self.width.zip(self.height),
        }
    }
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
        "dss" => Producer::Dss,
        "astap" => Producer::Astap,
        "maxim" => Producer::Maxim,
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

const RECORD_COLUMNS: &str = "derivative_uuid, acquired_image_guid, kind, steps, primary_source, \
     file_name, source_tail, size, mtime, width, height, producer, evidence, created_at, \
     updated_at";

fn record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DerivativeRecord> {
    Ok(DerivativeRecord {
        derivative_uuid: row.get(0)?,
        acquired_image_guid: row.get(1)?,
        kind: kind_from_str(&row.get::<_, String>(2)?),
        steps: ProcessingSteps::from_text(&row.get::<_, String>(3)?),
        primary_source: row.get(4)?,
        file_name: row.get(5)?,
        source_tail: row.get(6)?,
        size: row.get(7)?,
        mtime: row.get(8)?,
        width: row.get(9)?,
        height: row.get(10)?,
        producer: producer_from_str(&row.get::<_, String>(11)?),
        evidence: evidence_from_str(&row.get::<_, String>(12)?),
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn record_params(record: &DerivativeRecord) -> Vec<rusqlite::types::Value> {
    use rusqlite::types::Value;
    let text = |value: &str| Value::Text(value.to_string());
    let integer = |value: Option<i64>| value.map_or(Value::Null, Value::Integer);
    vec![
        text(&record.derivative_uuid),
        text(&record.acquired_image_guid),
        text(record.kind.as_str()),
        text(&record.steps.as_text()),
        Value::Integer(record.primary_source.into()),
        text(&record.file_name),
        record.source_tail.as_deref().map_or(Value::Null, text),
        integer(record.size),
        integer(record.mtime),
        integer(record.width),
        integer(record.height),
        text(record.producer.as_str()),
        text(evidence_str(record.evidence)),
        Value::Integer(record.created_at),
        Value::Integer(record.updated_at),
    ]
}

/// Record a copy. The same file of the same light updates its record in
/// place, keeping its `derivative_uuid`.
pub fn record_pairing(conn: &Connection, record: &DerivativeRecord) -> Result<()> {
    anyhow::ensure!(
        record.kind.is_derivative(),
        "only calibrated or registered copies are recorded, not {}",
        record.kind.as_str()
    );
    conn.execute(
        &format!(
            "INSERT INTO psf_guard_frame_derivative ({RECORD_COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(acquired_image_guid, file_name) DO UPDATE SET
                 kind = excluded.kind,
                 steps = excluded.steps,
                 primary_source = excluded.primary_source,
                 source_tail = excluded.source_tail,
                 size = excluded.size,
                 mtime = excluded.mtime,
                 width = excluded.width,
                 height = excluded.height,
                 producer = excluded.producer,
                 evidence = excluded.evidence,
                 updated_at = excluded.updated_at"
        ),
        rusqlite::params_from_iter(record_params(record)),
    )
    .context("recording a frame derivative")?;
    Ok(())
}

/// Point an existing record at a newer file of the same option, keeping its
/// `derivative_uuid` so synced catalogs follow the change instead of
/// growing a second record.
pub fn replace_record(conn: &Connection, uuid: &str, record: &DerivativeRecord) -> Result<()> {
    let mut replacement = record.clone();
    replacement.derivative_uuid = uuid.to_string();
    conn.execute(
        "DELETE FROM psf_guard_frame_derivative
         WHERE acquired_image_guid = ?1 AND file_name = ?2 AND derivative_uuid <> ?3",
        params![record.acquired_image_guid, record.file_name, uuid],
    )?;
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO psf_guard_frame_derivative ({RECORD_COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
        ),
        rusqlite::params_from_iter(record_params(&replacement)),
    )
    .context("replacing a frame derivative")?;
    Ok(())
}

/// Every copy recorded for one light: calibrated before registered, fewer
/// steps first.
pub fn derivatives_for_light(
    conn: &Connection,
    acquired_image_guid: &str,
) -> Result<Vec<DerivativeRecord>> {
    if !schema_exists(conn) {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(&format!(
        "SELECT {RECORD_COLUMNS} FROM psf_guard_frame_derivative
         WHERE acquired_image_guid = ?1"
    ))?;
    let mut records: Vec<DerivativeRecord> = statement
        .query_map([acquired_image_guid], record_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    records.sort_by(|left, right| {
        (
            left.kind.as_str(),
            left.steps.count(),
            left.steps,
            &left.file_name,
        )
            .cmp(&(
                right.kind.as_str(),
                right.steps.count(),
                right.steps,
                &right.file_name,
            ))
    });
    Ok(records)
}

/// One record by its uuid.
pub fn record_by_uuid(conn: &Connection, uuid: &str) -> Result<Option<DerivativeRecord>> {
    if !schema_exists(conn) {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            &format!(
                "SELECT {RECORD_COLUMNS} FROM psf_guard_frame_derivative
                 WHERE derivative_uuid = ?1"
            ),
            [uuid],
            record_from_row,
        )
        .optional()?)
}

/// Forget one recorded copy. The file itself is left alone.
pub fn forget(conn: &Connection, uuid: &str) -> Result<bool> {
    if !schema_exists(conn) {
        return Ok(false);
    }
    Ok(conn.execute(
        "DELETE FROM psf_guard_frame_derivative WHERE derivative_uuid = ?1",
        [uuid],
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
    /// Image size in pixels, when the header gave it.
    pub dimensions: Option<(i64, i64)>,
}

impl<D> DerivativeCandidate<D> {
    pub fn option(&self) -> OptionKey {
        OptionKey {
            steps: self.class.steps,
            dimensions: self.dimensions,
        }
    }
}

/// What became of one derivative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairOutcome<K, D> {
    Paired {
        derivative: D,
        light: K,
    },
    /// A newer file of the same option (same steps, same size) was preferred
    /// for this light.
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
/// otherwise it is ambiguous and left alone. Every distinct copy of a light
/// pairs: WBPP's `_r` and `_c_r` of one frame are two options, since their
/// pixels differ. Only copies of the same option (same steps, same size) of
/// one light compete; the newer file wins, then the first by key, and the
/// rest are superseded.
pub fn pair<K, D>(
    lights: &[LightCandidate<K>],
    derivatives: &[DerivativeCandidate<D>],
) -> Vec<PairOutcome<K, D>>
where
    K: Clone + Eq + std::hash::Hash,
    D: Clone + Ord,
{
    let mut outcomes = Vec::new();
    // (light index, option) -> derivative indices that chose it.
    type Claim = (usize, ProcessingSteps, Option<(i64, i64)>);
    let mut claims: std::collections::HashMap<Claim, Vec<usize>> = std::collections::HashMap::new();
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
            .entry((chosen, derivative.class.steps, derivative.dimensions))
            .or_default()
            .push(index);
    }
    let mut claims: Vec<_> = claims.into_iter().collect();
    claims.sort_by_key(|(claim, _)| *claim);
    for ((light_index, _, _), mut contenders) in claims {
        contenders.sort_by(|left, right| {
            let left = &derivatives[*left];
            let right = &derivatives[*right];
            right.mtime.cmp(&left.mtime).then(left.key.cmp(&right.key))
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

// ---------------------------------------------------------------------------
// Pairing files against a catalog
// ---------------------------------------------------------------------------

/// A catalogued light, as pairing names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CatalogLight {
    pub id: i64,
    pub guid: String,
}

fn stem_of(file_name: &str) -> String {
    let basename = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    crate::image_io::strip_image_extension(basename).to_string()
}

/// What pairing compares, read from a frame's headers.
pub fn identity_of(frame: &crate::commands::import::headers::FrameMeta) -> FrameIdentity {
    FrameIdentity {
        captured_at: frame
            .date_obs_utc
            .as_deref()
            .and_then(CaptureInstant::parse),
        exposure_s: frame.exposure_s,
        filter: frame.filter.clone(),
        target: frame.object.clone(),
        camera: frame.camera.clone(),
        stem: stem_of(&frame.basename()),
    }
}

/// A light row's identity: the capture fields N.I.N.A. (or an import)
/// recorded in its metadata, and its target's name.
pub fn light_identity(
    metadata: &str,
    acquired_date: Option<i64>,
    filter_column: Option<String>,
    target: Option<String>,
) -> FrameIdentity {
    let json: serde_json::Value = serde_json::from_str(metadata).unwrap_or_default();
    let text = |key: &str| {
        json.get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let captured_at = text("ExposureStartTime")
        .as_deref()
        .and_then(CaptureInstant::parse)
        .or(acquired_date.map(|seconds| CaptureInstant {
            seconds,
            nanos: None,
        }));
    FrameIdentity {
        captured_at,
        exposure_s: json
            .get("ExposureDuration")
            .and_then(|value| value.as_f64()),
        filter: text("FilterName").or(filter_column),
        target,
        camera: None,
        stem: text("FileName")
            .map(|name| stem_of(&name))
            .unwrap_or_default(),
    }
}

/// Every light with a guid, as pairing candidates. A light without a guid
/// cannot carry a record that survives sync; `fill-guids` repairs those.
pub fn catalog_lights(conn: &Connection) -> Result<Vec<LightCandidate<CatalogLight>>> {
    let mut statement = conn.prepare(
        "SELECT a.Id, a.guid, a.metadata, a.acquireddate, a.filtername, t.name
         FROM acquiredimage a LEFT JOIN target t ON t.Id = a.targetId
         WHERE a.guid IS NOT NULL AND a.guid <> ''",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(LightCandidate {
            key: CatalogLight {
                id: row.get(0)?,
                guid: row.get(1)?,
            },
            identity: light_identity(
                &row.get::<_, String>(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Lower-case basenames of every recorded copy, so an automatic import can
/// drop them before reading a header.
pub fn recorded_file_names(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    if !schema_exists(conn) {
        return Ok(Default::default());
    }
    let mut statement = conn.prepare("SELECT file_name FROM psf_guard_frame_derivative")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|name| name.to_lowercase())
        .collect())
}

/// What a pairing run did, for the import report and the logs.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct PairingReport {
    /// Copies recorded against their light this run.
    pub paired: usize,
    /// Copies already recorded, unchanged.
    pub already_recorded: usize,
    /// Copies that lost to a preferred copy of the same kind.
    pub superseded: usize,
    /// Copies that matched several lights with nothing to choose between.
    pub ambiguous: usize,
    /// Copies no light matched.
    pub unmatched: usize,
    /// A few ambiguous files, so the report can name them.
    pub ambiguous_examples: Vec<String>,
}

impl PairingReport {
    const EXAMPLES: usize = 5;

    pub fn absorb(&mut self, other: PairingReport) {
        self.paired += other.paired;
        self.already_recorded += other.already_recorded;
        self.superseded += other.superseded;
        self.ambiguous += other.ambiguous;
        self.unmatched += other.unmatched;
        for example in other.ambiguous_examples {
            if self.ambiguous_examples.len() < Self::EXAMPLES {
                self.ambiguous_examples.push(example);
            }
        }
    }
}

/// The last two folders above a file, to tell two files of one name apart
/// when the record is found again.
fn source_tail(path: &std::path::Path) -> Option<String> {
    let parents: Vec<String> = path
        .parent()?
        .components()
        .rev()
        .take(2)
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!parents.is_empty()).then(|| parents.into_iter().rev().collect::<Vec<_>>().join("/"))
}

fn file_fingerprint(path: &std::path::Path) -> (Option<i64>, Option<i64>) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return (None, None);
    };
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64);
    (Some(metadata.len() as i64), mtime)
}

/// Siril's record of where each sequence frame came from, read from the
/// `*_conversion.txt` logs in one folder: sequence stem -> source stem.
/// Siril's later steps only prefix the sequence name (`pp_`, `r_`), so a
/// derivative is looked up by its base stem.
fn siril_sources(folder: &std::path::Path) -> HashMap<String, String> {
    let mut sources = HashMap::new();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return sources;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with("_conversion.txt") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for (source, frame) in parse_siril_conversion_log(&text) {
            sources.insert(stem_of(&frame), stem_of(&source));
        }
    }
    sources
}

use std::collections::HashMap;

fn now_epoch() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The record for one paired file.
fn record_for(
    frame: &crate::commands::import::headers::FrameMeta,
    light_guid: &str,
    primary_source: bool,
) -> DerivativeRecord {
    let (size, mtime) = file_fingerprint(&frame.path);
    let now = now_epoch();
    DerivativeRecord {
        derivative_uuid: uuid::Uuid::new_v4().to_string(),
        acquired_image_guid: light_guid.to_string(),
        kind: frame.class.kind,
        steps: frame.class.steps,
        primary_source,
        file_name: frame.basename(),
        source_tail: source_tail(&frame.path),
        size,
        mtime,
        width: frame.width,
        height: frame.height,
        producer: frame.class.producer,
        evidence: frame.class.evidence,
        created_at: now,
        updated_at: now,
    }
}

/// Derivative candidates for these frames, keyed by their index.
pub fn derivative_candidates(
    frames: &[crate::commands::import::headers::FrameMeta],
) -> Vec<DerivativeCandidate<usize>> {
    let mut siril: HashMap<std::path::PathBuf, HashMap<String, String>> = HashMap::new();
    frames
        .iter()
        .enumerate()
        .filter(|(_, frame)| frame.class.kind.is_derivative())
        .map(|(index, frame)| {
            let source_stem = frame.path.parent().and_then(|folder| {
                let sources = siril
                    .entry(folder.to_path_buf())
                    .or_insert_with(|| siril_sources(folder));
                let identity = identity_of(frame);
                sources
                    .get(crate::image_io::derivative_base_stem(&identity.stem))
                    .cloned()
            });
            DerivativeCandidate {
                key: index,
                class: frame.class,
                identity: identity_of(frame),
                mtime: file_fingerprint(&frame.path).1,
                source_stem,
                dimensions: frame.width.zip(frame.height),
            }
        })
        .collect()
}

/// Pair calibrated and registered files with the catalog's lights and record
/// each one. A copy of an option the light already has (same steps, same
/// size) under another name is a newer file of it and takes the record
/// over, unless that record is the light's own file: a light catalogued from
/// a calibrated copy keeps it, and the other file counts as superseded.
///
/// Returns the report and the indices of the frames no light matched.
pub fn pair_into_catalog(
    conn: &Connection,
    frames: &[crate::commands::import::headers::FrameMeta],
    lights: &[LightCandidate<CatalogLight>],
) -> Result<(PairingReport, Vec<usize>)> {
    let candidates = derivative_candidates(frames);
    let mut report = PairingReport::default();
    let mut unmatched = Vec::new();
    if candidates.is_empty() {
        return Ok((report, unmatched));
    }
    ensure_schema(conn)?;
    for outcome in pair(lights, &candidates) {
        match outcome {
            PairOutcome::Paired { derivative, light } => {
                let frame = &frames[derivative];
                let record = record_for(frame, &light.guid, false);
                let recorded = derivatives_for_light(conn, &light.guid)?;
                if recorded
                    .iter()
                    .any(|mine| mine.file_name == record.file_name)
                {
                    report.already_recorded += 1;
                    continue;
                }
                match recorded
                    .iter()
                    .find(|mine| mine.option().same_as(record.option()))
                {
                    Some(mine) if mine.primary_source => report.superseded += 1,
                    Some(mine) if mine.mtime > record.mtime => report.superseded += 1,
                    Some(mine) => {
                        replace_record(conn, &mine.derivative_uuid, &record)?;
                        report.paired += 1;
                    }
                    None => {
                        record_pairing(conn, &record)?;
                        report.paired += 1;
                    }
                }
            }
            PairOutcome::Superseded { .. } => report.superseded += 1,
            PairOutcome::Ambiguous { derivative, .. } => {
                report.absorb(PairingReport {
                    ambiguous: 1,
                    ambiguous_examples: vec![frames[derivative].path.display().to_string()],
                    ..Default::default()
                });
            }
            PairOutcome::Unmatched { derivative } => {
                report.unmatched += 1;
                unmatched.push(derivative);
            }
        }
    }
    unmatched.sort_unstable();
    Ok((report, unmatched))
}

/// Of the calibrated copies no light matched, the ones that become lights of
/// their own: one per acquisition, preferring the one with the most steps
/// (`_c_cc` over `_c`), then the newer, then the first by path. The other
/// copies pair with that light as options. Registered copies never become
/// lights:
/// their pixels are resampled, and a registered stack carries a light's
/// keywords.
pub fn choose_primaries(
    frames: &[crate::commands::import::headers::FrameMeta],
    unmatched: &[usize],
) -> Vec<usize> {
    let mut contenders: Vec<(usize, FrameIdentity, Option<i64>)> = unmatched
        .iter()
        .copied()
        .filter(|index| frames[*index].class.kind == FrameKind::Calibrated)
        .map(|index| {
            let frame = &frames[index];
            (index, identity_of(frame), file_fingerprint(&frame.path).1)
        })
        .collect();
    let steps = |index: usize| frames[index].class.steps.count();
    contenders.sort_by(|left, right| {
        steps(right.0)
            .cmp(&steps(left.0))
            .then(right.2.cmp(&left.2))
            .then(frames[left.0].path.cmp(&frames[right.0].path))
    });
    let mut chosen: Vec<(usize, FrameIdentity)> = Vec::new();
    for (index, identity, _) in contenders {
        if chosen
            .iter()
            .any(|(_, kept)| identities_match(kept, &identity))
        {
            continue;
        }
        chosen.push((index, identity));
    }
    let mut chosen: Vec<usize> = chosen.into_iter().map(|(index, _)| index).collect();
    chosen.sort_unstable();
    chosen
}

/// Record lights whose own file is a calibrated copy, found by the file name
/// the import wrote into their metadata.
pub fn record_primaries(
    conn: &Connection,
    frames: &[crate::commands::import::headers::FrameMeta],
) -> Result<usize> {
    if frames.is_empty() {
        return Ok(0);
    }
    ensure_schema(conn)?;
    let mut statement = conn.prepare(
        "SELECT guid FROM acquiredimage
         WHERE json_extract(metadata, '$.FileName') = ?1 AND guid IS NOT NULL AND guid <> ''",
    )?;
    let mut recorded = 0;
    for frame in frames {
        let path = frame.path.to_string_lossy();
        let Some(guid) = statement
            .query_row([path.as_ref()], |row| row.get::<_, String>(0))
            .optional()?
        else {
            continue;
        };
        record_pairing(conn, &record_for(frame, &guid, true))?;
        recorded += 1;
    }
    Ok(recorded)
}

/// Raw frames that are the acquisition behind a light the catalog took from
/// a calibrated copy. Each such light takes the raw file over: its
/// `FileName` moves to the raw, and the calibrated record stops being
/// primary. The row keeps its id, guid and grade, so nothing that refers to
/// the frame changes. Returns the indices of the frames adopted this way;
/// a raw that matches two such lights adopts neither.
pub fn adopt_raw_frames(
    conn: &Connection,
    frames: &[crate::commands::import::headers::FrameMeta],
) -> Result<Vec<usize>> {
    if !schema_exists(conn) || frames.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT a.Id, a.guid, a.metadata, a.acquireddate, a.filtername, t.name
         FROM psf_guard_frame_derivative d
         JOIN acquiredimage a ON a.guid = d.acquired_image_guid
         LEFT JOIN target t ON t.Id = a.targetId
         WHERE d.primary_source = 1",
    )?;
    let mut primaries: Vec<Option<LightCandidate<CatalogLight>>> = statement
        .query_map([], |row| {
            Ok(Some(LightCandidate {
                key: CatalogLight {
                    id: row.get(0)?,
                    guid: row.get(1)?,
                },
                identity: light_identity(
                    &row.get::<_, String>(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ),
            }))
        })?
        .collect::<rusqlite::Result<_>>()?;
    drop(statement);
    let mut adopted = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        if frame.class.kind != FrameKind::Raw {
            continue;
        }
        let identity = identity_of(frame);
        let matches: Vec<usize> = primaries
            .iter()
            .enumerate()
            .filter_map(|(position, light)| {
                let light = light.as_ref()?;
                identities_match(&light.identity, &identity).then_some(position)
            })
            .collect();
        let [position] = matches.as_slice() else {
            continue;
        };
        let light = primaries[*position].take().expect("matched a live primary");
        conn.execute(
            "UPDATE acquiredimage SET metadata = json_set(metadata, '$.FileName', ?1)
             WHERE Id = ?2",
            params![frame.path.to_string_lossy(), light.key.id],
        )?;
        conn.execute(
            "UPDATE psf_guard_frame_derivative SET primary_source = 0, updated_at = ?2
             WHERE acquired_image_guid = ?1 AND primary_source = 1",
            params![light.key.guid, now_epoch()],
        )?;
        adopted.push(index);
    }
    Ok(adopted)
}

/// A file's size and modification time, to tell when it changed.
pub type FileStamp = (Option<i64>, Option<i64>);

/// The most files one background pass reads headers for. The rest wait for
/// the next directory refresh, so a first pass over a large processing
/// tree never holds the catalog for long.
pub const MAX_HEADERS_PER_PASS: usize = 2_000;

/// Pair calibrated and registered files already on disk with the catalog's
/// lights. Unlike import this never adds a light: a copy no light matches
/// waits for an import, which decides whether it becomes one.
///
/// Only files named the way the tools name their output are considered: a
/// directory refresh cannot read every header in a calibration library. A
/// copy known only by its header is paired when it is imported.
///
/// `checked` remembers files read before that were not pairable, by stamp,
/// so a pass reads only what is new or changed. Files the catalog already
/// knows (lights by name, recorded copies) are dropped before any header is
/// read.
pub fn pair_files_on_disk(
    conn: &mut Connection,
    files: &[std::path::PathBuf],
    checked: &mut HashMap<std::path::PathBuf, FileStamp>,
) -> Result<PairingReport> {
    let files: Vec<std::path::PathBuf> = files
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(crate::image_io::name_suggests_derivative)
        })
        .cloned()
        .collect();
    if files.is_empty() {
        return Ok(PairingReport::default());
    }
    let known = crate::commands::import::known_files(conn, &files)?;
    let fresh: Vec<&std::path::PathBuf> = files
        .iter()
        .filter(|path| !known.contains(*path))
        .filter(|path| checked.get(*path) != Some(&file_fingerprint(path)))
        .take(MAX_HEADERS_PER_PASS)
        .collect();
    if fresh.is_empty() {
        return Ok(PairingReport::default());
    }
    let frames: Vec<crate::commands::import::headers::FrameMeta> = fresh
        .iter()
        .map(|path| crate::commands::import::headers::read_frame_meta(path))
        .collect();
    for frame in &frames {
        if !(frame.class.kind.is_derivative() && frame.is_light()) {
            checked.insert(frame.path.clone(), file_fingerprint(&frame.path));
        }
    }
    let copies: Vec<_> = frames
        .into_iter()
        .filter(|frame| frame.class.kind.is_derivative() && frame.is_light())
        .collect();
    if copies.is_empty() {
        return Ok(PairingReport::default());
    }
    let tx = conn.transaction()?;
    let lights = catalog_lights(&tx)?;
    let (report, unmatched) = pair_into_catalog(&tx, &copies, &lights)?;
    tx.commit()?;
    // Recorded copies drop out by name next time. The rest stay unpaired
    // until their light or their file changes.
    for index in unmatched {
        checked.insert(
            copies[index].path.clone(),
            file_fingerprint(&copies[index].path),
        );
    }
    Ok(report)
}

/// How many copies a catalog has paired, for Settings.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct PairingCounts {
    pub calibrated: usize,
    pub registered: usize,
    /// Lights whose own file is a calibrated copy (no raw frame yet).
    pub calibrated_lights: usize,
}

pub fn pairing_counts(conn: &Connection) -> Result<PairingCounts> {
    if !schema_exists(conn) {
        return Ok(PairingCounts::default());
    }
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(kind = 'calibrated'), 0), COALESCE(SUM(kind = 'registered'), 0),
                COALESCE(SUM(primary_source = 1), 0)
         FROM psf_guard_frame_derivative",
        [],
        |row| {
            Ok(PairingCounts {
                calibrated: row.get::<_, i64>(0)? as usize,
                registered: row.get::<_, i64>(1)? as usize,
                calibrated_lights: row.get::<_, i64>(2)? as usize,
            })
        },
    )?)
}

/// Guids of the lights whose own file is a calibrated copy.
pub fn primary_light_guids(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    if !schema_exists(conn) {
        return Ok(Default::default());
    }
    let mut statement = conn.prepare(
        "SELECT acquired_image_guid FROM psf_guard_frame_derivative WHERE primary_source = 1",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A light that took its raw file over: its calibrated record is no
/// longer the light's own file.
pub fn release_primary(conn: &Connection, acquired_image_guid: &str) -> Result<()> {
    if !schema_exists(conn) {
        return Ok(());
    }
    conn.execute(
        "UPDATE psf_guard_frame_derivative SET primary_source = 0, updated_at = ?2
         WHERE acquired_image_guid = ?1 AND primary_source = 1",
        params![acquired_image_guid, now_epoch()],
    )?;
    Ok(())
}

/// Point records at a light's new guid, after sync adopted a row PSF Guard
/// minted and gave it the telescope's guid.
pub fn rename_light_guid(conn: &Connection, from: &str, to: &str) -> Result<()> {
    if !schema_exists(conn) || from == to {
        return Ok(());
    }
    conn.execute(
        "UPDATE OR IGNORE psf_guard_frame_derivative SET acquired_image_guid = ?2
         WHERE acquired_image_guid = ?1",
        params![from, to],
    )?;
    Ok(())
}

/// Counts from carrying records between catalogs.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct DerivativeSyncCounts {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
    /// Records whose light did not come across, or whose light already has
    /// its own record of that file or option here.
    pub skipped: usize,
}

/// Carry pairing records from `source` to `destination` by
/// `derivative_uuid`. Lights match across catalogs by guid, so a record
/// keeps its light; one whose light is not in the destination is skipped.
/// Where the destination already paired the same file or option of that
/// light on its own, its record stays.
pub fn sync_records(source: &Connection, destination: &Connection) -> Result<DerivativeSyncCounts> {
    let mut counts = DerivativeSyncCounts::default();
    if !schema_exists(source) {
        return Ok(counts);
    }
    ensure_schema(destination)?;
    let mut statement = source.prepare(&format!(
        "SELECT {RECORD_COLUMNS} FROM psf_guard_frame_derivative"
    ))?;
    let records: Vec<DerivativeRecord> = statement
        .query_map([], record_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    for record in records {
        let light_here: bool = destination
            .query_row(
                "SELECT 1 FROM acquiredimage WHERE guid = ?1",
                [&record.acquired_image_guid],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !light_here {
            counts.skipped += 1;
            continue;
        }
        if let Some(mine) = record_by_uuid(destination, &record.derivative_uuid)? {
            if mine == record || mine.updated_at >= record.updated_at {
                counts.unchanged += 1;
            } else {
                replace_record(destination, &record.derivative_uuid, &record)?;
                counts.updated += 1;
            }
            continue;
        }
        // The destination paired this file, or this option, on its own.
        let paired_here = derivatives_for_light(destination, &record.acquired_image_guid)?
            .into_iter()
            .any(|mine| {
                mine.file_name == record.file_name || mine.option().same_as(record.option())
            });
        if paired_here {
            counts.skipped += 1;
        } else {
            record_pairing(destination, &record)?;
            counts.inserted += 1;
        }
    }
    Ok(counts)
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
            steps: steps_for(kind, includes_calibration),
        }
    }

    fn steps_for(kind: FrameKind, includes_calibration: bool) -> ProcessingSteps {
        let base = if kind == FrameKind::Registered {
            ProcessingSteps::REGISTERED
        } else {
            ProcessingSteps::CALIBRATED
        };
        if includes_calibration {
            base.with(ProcessingSteps::CALIBRATED)
        } else {
            base
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
            dimensions: None,
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
    fn every_distinct_copy_of_a_light_pairs() {
        // WBPP left both `_0115_r.xisf` and `_0115_c_r.xisf` for one light on
        // the C925 NGC 6543 run: different pixels, so two options.
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
                PairOutcome::Paired {
                    derivative: "frame_0115_r",
                    light: 1
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

    /// A minimal FITS light with the acquisition keywords N.I.N.A. writes.
    fn write_light(
        folder: &std::path::Path,
        name: &str,
        bitpix: i32,
        extra: &[&str],
    ) -> std::path::PathBuf {
        let mut cards: Vec<String> = vec![
            "SIMPLE  =                    T".into(),
            format!("BITPIX  = {bitpix:>20}"),
            "NAXIS   =                    2".into(),
            "NAXIS1  =                    4".into(),
            "NAXIS2  =                    3".into(),
            "IMAGETYP= 'LIGHT'".into(),
            "DATE-OBS= '2026-06-07T10:20:00.6712661'".into(),
            "EXPOSURE=                 75.0".into(),
            "FILTER  = 'B'".into(),
            "OBJECT  = 'NGC 6543'".into(),
            "INSTRUME= 'ZWO ASI2600MM Pro'".into(),
            "RA      =             269.6393".into(),
            "DEC     =              66.6332".into(),
        ];
        cards.extend(extra.iter().map(|card| card.to_string()));
        cards.push("END".into());
        let mut bytes: Vec<u8> = cards
            .iter()
            .flat_map(|card| format!("{card:<80}").into_bytes())
            .collect();
        bytes.resize(2880, b' ');
        bytes.resize(2880 * 2, 0);
        let path = folder.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn the_background_pass_pairs_copies_already_on_disk() {
        let folder = tempfile::tempdir().unwrap();
        let raw = write_light(folder.path(), "frame_0115.fits", 16, &[]);
        let calibrated = write_light(folder.path(), "frame_0115_c.fits", -32, &[]);
        // Named like a copy, but camera integers: an r-filter raw.
        let lookalike = write_light(folder.path(), "frame_0116_r.fits", 16, &[]);

        let mut conn = Connection::open_in_memory().unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        let frames = crate::commands::import::scan_frames(std::slice::from_ref(&raw));
        crate::commands::import::import_frames(
            &mut conn,
            frames,
            &crate::commands::import::ImportOptions::default(),
        )
        .unwrap();

        let files = vec![raw.clone(), calibrated.clone(), lookalike.clone()];
        let mut checked = HashMap::new();
        let report = pair_files_on_disk(&mut conn, &files, &mut checked).unwrap();
        assert_eq!(report.paired, 1);
        assert_eq!(checked.len(), 1, "the lookalike is remembered as read");
        assert!(checked.contains_key(&lookalike));
        let guid: String = conn
            .query_row("SELECT guid FROM acquiredimage", [], |row| row.get(0))
            .unwrap();
        let records = derivatives_for_light(&conn, &guid).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].file_name, "frame_0115_c.fits");
        assert_eq!(records[0].kind, FrameKind::Calibrated);
        assert!(records[0].size.is_some());

        // Nothing new on the next pass: no header is read again.
        let again = pair_files_on_disk(&mut conn, &files, &mut checked).unwrap();
        assert_eq!(again, PairingReport::default());
        let lights: i64 = conn
            .query_row("SELECT COUNT(*) FROM acquiredimage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(lights, 1, "the pass never adds a light");
    }

    fn record(guid: &str, kind: FrameKind, file_name: &str, uuid: &str) -> DerivativeRecord {
        DerivativeRecord {
            derivative_uuid: uuid.into(),
            acquired_image_guid: guid.into(),
            kind,
            steps: steps_for(kind, true),
            primary_source: false,
            file_name: file_name.into(),
            source_tail: Some("calibrated/Light_B".into()),
            size: Some(104_419_840),
            mtime: Some(1_750_000_000),
            width: None,
            height: None,
            producer: Producer::Pixinsight,
            evidence: KindEvidence::Header,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn every_copy_is_a_record_and_a_newer_file_keeps_its_identity() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(!schema_exists(&conn));
        assert!(derivatives_for_light(&conn, "light").unwrap().is_empty());
        ensure_schema(&conn).unwrap();
        ensure_schema(&conn).unwrap();

        let calibrated = record("light", FrameKind::Calibrated, "f_c.xisf", "u-c");
        let mut cosmetic = record("light", FrameKind::Calibrated, "f_c_cc.xisf", "u-cc");
        cosmetic.steps = ProcessingSteps::CALIBRATED.with(ProcessingSteps::COSMETIC);
        let registered = record("light", FrameKind::Registered, "f_c_r.xisf", "u-r");
        for record in [&registered, &cosmetic, &calibrated] {
            record_pairing(&conn, record).unwrap();
        }
        let stored = derivatives_for_light(&conn, "light").unwrap();
        let names: Vec<&str> = stored
            .iter()
            .map(|record| record.file_name.as_str())
            .collect();
        assert_eq!(names, ["f_c.xisf", "f_c_cc.xisf", "f_c_r.xisf"]);
        assert_eq!(stored[1].steps.label(), "Calibrated + cosmetic");

        // A newer file of one option takes its record over, uuid and all.
        let mut newer = record("light", FrameKind::Calibrated, "run2/f_c.xisf", "u-new");
        newer.updated_at = 2;
        replace_record(&conn, "u-c", &newer).unwrap();
        let replaced = record_by_uuid(&conn, "u-c").unwrap().unwrap();
        assert_eq!(replaced.file_name, "run2/f_c.xisf");
        assert_eq!(derivatives_for_light(&conn, "light").unwrap().len(), 3);

        assert!(forget(&conn, "u-r").unwrap());
        assert!(!forget(&conn, "u-r").unwrap());
        assert_eq!(derivatives_for_light(&conn, "light").unwrap().len(), 2);
        assert!(record_pairing(&conn, &record("light", FrameKind::Raw, "f.fits", "u")).is_err());
    }

    #[test]
    fn steps_round_trip_and_name_each_option() {
        let steps = ProcessingSteps::from_text("registered,calibrated");
        assert_eq!(steps.as_text(), "calibrated,registered");
        assert_eq!(steps.label(), "Calibrated + registered");
        assert_eq!(ProcessingSteps::REGISTERED.label(), "Registered");
        assert!(steps.changes_geometry());
        let key = |dimensions| OptionKey { steps, dimensions };
        assert!(
            key(Some((6248, 4176))).same_as(key(None)),
            "unknown size matches"
        );
        assert!(!key(Some((6248, 4176))).same_as(key(Some((6000, 4000)))));
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
