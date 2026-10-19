//! How well the library calibrates each project's lights.
//!
//! Every light of a project that is not rejected is matched against the
//! library the way a stack matches it, and what that match would give it is
//! judged on two levels:
//!
//! - **Poor**: the light cannot be calibrated properly. Nothing in the
//!   library matches its camera and settings; no flat, or no bias or dark,
//!   matches though the library holds them, or its flats are past the age
//!   limit (see [`super::calibration_gap`]); its dark master would be thin,
//!   or its bias master when no dark serves it; or its flats carry changing
//!   spots into their master.
//! - **Check**: worth a look. Its flats were shot more than
//!   [`FLAT_CHECK_AGE_DAYS`] from it; its flat master mixes nights; its
//!   masters use suspect frames; frames kept out by hand leave a set short;
//!   its bias master is thin while a dark serves it; or a changing spot sits
//!   in most of its flats without reaching the poor limit.
//!
//! A light takes the worst level among its findings, and a project the worst
//! among its lights. Each finding counts its lights by night and filter.
//! The work is one header read and one match per light, so the server runs
//! it in the background and keeps the result; see [`project_key`] and
//! [`project_file_digests`]. It keeps each light's header too
//! ([`KeptHeader`]), so a project judged again reads only the lights whose
//! file changed.

use super::quality::{self, StoredMeasurement};
use super::{
    calibration_gaps, coherent_master_subset, flat_session_at, rule_book, CalibrationFrame,
    CalibrationKind, Check, CheckMode, ExternalMasterPolicy, LibrarySnapshot, Rules,
    MIN_MASTER_FRAMES, THIN_MASTER_FRAMES,
};
use crate::commands::import::headers::FrameMeta;
use anyhow::{Context, Result};
use rusqlite::Connection;
use seiza_calibration::{FlatMeasurement, Grade, FLAT_QUALITY_ALGORITHM_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

/// Bump when the rules or what a result keeps change, so every kept result
/// is computed again. 2: each finding keeps its lights' ids. 3: measured
/// flat groups: mixed nights only when unmeasured, flats across a change.
/// 4: grouping by time checks mixed nights as version 2 did. 5: flats by
/// measured response family.
pub const HEALTH_ALGORITHM_VERSION: u32 = 5;

/// Flats further than this from a light are worth a look, though they are
/// within the age limit that makes a light one that cannot be calibrated,
/// unless a setting says otherwise ([`super::Rules::flat_check_age_days`]).
pub const FLAT_CHECK_AGE_DAYS: f64 = super::rules::DEFAULT_FLAT_CHECK_AGE_DAYS;

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthLevel {
    /// Worth a look; the light still calibrates.
    Check,
    /// The light cannot be calibrated properly.
    Poor,
}

/// Which rule a finding comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthCode {
    /// Nothing in the library matches the light's camera and settings.
    NoLibrary,
    /// One of the gap rules: no flat, no bias or dark, flats too old.
    CannotCalibrate,
    /// The dark master would use fewer than [`THIN_MASTER_FRAMES`] frames.
    ThinDark,
    /// The bias master would use fewer than [`THIN_MASTER_FRAMES`] frames.
    ThinBias,
    /// Changing spots sit in most of the flats, so their master keeps them.
    FlatSpots,
    /// The flats were shot more than [`FLAT_CHECK_AGE_DAYS`] from the light.
    OldFlats,
    /// The flat master takes flats from nights not measured against each
    /// other.
    MixedFlats,
    /// A measured change lies between the light and its flats, or the
    /// light was shot while it happened.
    FlatsDiffer,
    /// The masters use frames graded suspect.
    SuspectFrames,
    /// Frames a person kept out leave a set short.
    ShortAfterKeepOut,
}

/// One thing wrong with one light.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightFinding {
    pub code: HealthCode,
    pub level: HealthLevel,
    /// In plain words, with its numbers.
    pub reason: String,
    /// The kinds of calibration frame it is about, so a view of one light
    /// can point at them. Every kind when nothing matches the light.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<CalibrationKind>,
}

impl LightFinding {
    fn about(mut self, kinds: impl IntoIterator<Item = CalibrationKind>) -> Self {
        self.kinds = kinds.into_iter().collect();
        self
    }
}

/// How many lights of one night and filter a finding holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NightFilterLights {
    pub night: String,
    pub filter: String,
    pub lights: usize,
}

/// One finding across a project: its lights, by night and filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthFinding {
    pub code: HealthCode,
    pub level: HealthLevel,
    pub reason: String,
    pub lights: usize,
    /// Newest night first.
    pub nights: Vec<NightFilterLights>,
    /// The lights themselves, so the image views can mark and filter them.
    #[serde(default)]
    pub image_ids: Vec<i32>,
}

/// How well the library calibrates one project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHealth {
    pub project_id: i32,
    /// The worst finding's level; absent when nothing was found.
    pub level: Option<HealthLevel>,
    /// Lights read and matched.
    pub checked: usize,
    /// Lights whose file was not found or could not be read.
    pub missing_files: usize,
    /// Lights with at least one poor finding.
    pub poor_lights: usize,
    /// Lights whose worst finding is one to check.
    pub check_lights: usize,
    /// Poor findings first.
    pub findings: Vec<HealthFinding>,
}

/// A light to judge: where its file is, and the night and filter it is
/// counted under.
#[derive(Debug, Clone)]
pub struct ProjectLight {
    pub image_id: i32,
    pub path: PathBuf,
    pub night: String,
    pub filter: String,
}

/// What judging one light found.
#[derive(Debug, Clone)]
pub struct LightResult {
    pub image_id: i32,
    pub night: String,
    pub filter: String,
    /// `None` when the file could not be read.
    pub findings: Option<Vec<LightFinding>>,
}

use crate::light_headers::FileStamp;
/// A light's header as a pass read it, kept in the catalog so the next pass
/// reads it again only when the file changed: another place, size or
/// modification time.
pub use crate::light_headers::KeptHeader;

/// What judging one light came to, with its header for the next pass.
#[derive(Debug, Clone)]
pub struct JudgedLight {
    pub result: LightResult,
    /// The header read or kept; absent when the file could not be read.
    pub header: Option<KeptHeader>,
    /// Whether the header was read from the file this time.
    pub read: bool,
}

/// The library as judging needs it, read once for many lights.
pub struct HealthLibrary {
    snapshot: LibrarySnapshot,
    flats: Vec<CalibrationFrame>,
    /// Where each rig's calibration nights split, and the catalog's lights'.
    nights: super::Nights,
    has_frames: bool,
    suspect: HashSet<String>,
    by_hand: HashSet<String>,
    flat_measurements: HashMap<String, FlatMeasurement>,
    /// Seiza's verdict on each flat set already judged, by its frames and
    /// the limits it was judged by.
    flat_sets: Mutex<HashMap<String, Option<(Grade, usize)>>>,
    /// The settings in force when the library was read, so every light of
    /// one pass is judged by the same ones.
    book: std::sync::Arc<super::RuleBook>,
}

/// What the checks made of one light.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Judgement {
    /// What they flag, at the level its check's setting allows.
    pub findings: Vec<LightFinding>,
    /// What a check would have flagged had a setting not turned it off.
    pub waived: Vec<LightFinding>,
}

/// Findings as their checks' settings have them: flagged, made advisory, or
/// set aside as waived.
struct Gathered<'a> {
    rules: &'a Rules,
    judgement: Judgement,
}

impl<'a> Gathered<'a> {
    fn new(rules: &'a Rules) -> Self {
        Self {
            rules,
            judgement: Judgement::default(),
        }
    }

    /// Who set a check's mode, in words.
    fn set_by(&self, check: Check) -> String {
        self.rules
            .set_by(check.key())
            .unwrap_or_else(|| "set for this rig".into())
    }

    fn push(&mut self, check: Check, mut finding: LightFinding) {
        match self.rules.mode(check) {
            CheckMode::Flag => self.judgement.findings.push(finding),
            CheckMode::Advisory => {
                if finding.level == HealthLevel::Poor {
                    finding.level = HealthLevel::Check;
                    finding.reason =
                        format!("{} (advisory, {})", finding.reason, self.set_by(check));
                }
                self.judgement.findings.push(finding);
            }
            CheckMode::Off => {
                finding.reason = format!("{} (check off, {})", finding.reason, self.set_by(check));
                self.judgement.waived.push(finding);
            }
        }
    }
}

/// `" (fewer than 4, set globally)"` after a finding whose limit someone
/// set, or nothing for a default limit.
fn limit_note(rules: &Rules, key: &str, limit: impl FnOnce() -> String) -> String {
    rules
        .set_by(key)
        .map(|by| format!(" ({}, {by})", limit()))
        .unwrap_or_default()
}

/// The frames one master would take: a master from other software as it
/// is, or the coherent set of raw frames.
enum MasterInput {
    None,
    External,
    Raw(Vec<CalibrationFrame>),
}

impl MasterInput {
    /// The rule a stack build follows: an external master by the policy,
    /// otherwise the raw frames' coherent subset.
    fn of(kind: CalibrationKind, frames: &[CalibrationFrame], rules: &Rules) -> Self {
        if frames.is_empty() {
            return Self::None;
        }
        let raw: Vec<CalibrationFrame> = frames
            .iter()
            .filter(|frame| !frame.is_master)
            .cloned()
            .collect();
        let has_external = raw.len() < frames.len();
        let subset = coherent_master_subset(kind, &raw, rules);
        let external = match rules.external_masters {
            ExternalMasterPolicy::Prefer => has_external,
            ExternalMasterPolicy::Fallback => has_external && subset.len() < MIN_MASTER_FRAMES,
            ExternalMasterPolicy::Ignore => false,
        };
        if external {
            Self::External
        } else {
            Self::Raw(subset)
        }
    }

    /// Whether a master would apply at all.
    fn applies(&self) -> bool {
        match self {
            Self::None => false,
            Self::External => true,
            Self::Raw(frames) => frames.len() >= MIN_MASTER_FRAMES,
        }
    }

    fn raw(&self) -> &[CalibrationFrame] {
        match self {
            Self::Raw(frames) => frames,
            _ => &[],
        }
    }
}

fn poor(code: HealthCode, reason: impl Into<String>) -> LightFinding {
    LightFinding {
        code,
        level: HealthLevel::Poor,
        reason: reason.into(),
        kinds: Vec::new(),
    }
}

fn check(code: HealthCode, reason: impl Into<String>) -> LightFinding {
    LightFinding {
        code,
        level: HealthLevel::Check,
        reason: reason.into(),
        kinds: Vec::new(),
    }
}

fn frames(count: usize) -> String {
    format!("{count} frame{}", if count == 1 { "" } else { "s" })
}

impl HealthLibrary {
    pub fn load(conn: &Connection) -> Result<Self> {
        let snapshot = LibrarySnapshot::load(conn)?;
        let flats = snapshot
            .frames()
            .iter()
            .filter(|frame| frame.kind == CalibrationKind::Flat)
            .cloned()
            .collect();
        let has_frames = !snapshot.frames().is_empty();
        let mut suspect = HashSet::new();
        let mut by_hand = HashSet::new();
        let mut flat_measurements = HashMap::new();
        if quality::table_exists(conn) {
            let mut statement = conn.prepare(
                "SELECT q.frame_uuid, q.grade, q.override_choice,
                        CASE WHEN f.kind = 'flat' AND q.algorithm_version = ?1
                                  AND q.source_fingerprint IS f.source_fingerprint
                             THEN q.measurement_json END
                 FROM psf_guard_calibration_frame_quality q
                 JOIN psf_guard_calibration_frame f ON f.frame_uuid = q.frame_uuid",
            )?;
            let mut rows = statement.query([FLAT_QUALITY_ALGORITHM_VERSION])?;
            while let Some(row) = rows.next()? {
                let uuid: String = row.get(0)?;
                let choice = row.get::<_, Option<String>>(2)?;
                // A suspect frame a person chose to use anyway reads as ok.
                if row.get::<_, Option<String>>(1)?.as_deref() == Some("suspect")
                    && choice.as_deref() != Some("use")
                {
                    suspect.insert(uuid.clone());
                }
                if choice.as_deref() == Some("exclude") {
                    by_hand.insert(uuid.clone());
                }
                if let Some(json) = row.get::<_, Option<String>>(3)?
                    && let Ok(StoredMeasurement::Flat(flat)) = serde_json::from_str(&json)
                {
                    flat_measurements.insert(uuid, flat.measurement);
                }
            }
        }
        Ok(Self {
            snapshot,
            flats,
            nights: super::Nights::load(conn),
            has_frames,
            suspect,
            by_hand,
            flat_measurements,
            flat_sets: Mutex::new(HashMap::new()),
            book: rule_book(),
        })
    }

    /// Where this catalog's lights' nights split, by profile, as Images
    /// groups them at the server's `light_nights` setting.
    pub fn light_nights(&self, conn: &Connection) -> super::LightNights {
        super::LightNights::from_nights(conn, &self.nights, None)
    }

    /// Whether the library holds no frame at all. Then there is nothing to
    /// judge a light by, and no header needs reading.
    pub fn is_empty(&self) -> bool {
        !self.has_frames
    }

    /// Read one light's header and judge it.
    pub fn judge_light(&self, light: &ProjectLight) -> Result<LightResult> {
        Ok(self.judge_light_kept(light, None)?.result)
    }

    /// Judge one light, reading its header only when `kept` does not hold
    /// it as the file is now.
    pub fn judge_light_kept(
        &self,
        light: &ProjectLight,
        kept: Option<&KeptHeader>,
    ) -> Result<JudgedLight> {
        let result = |findings| LightResult {
            image_id: light.image_id,
            night: light.night.clone(),
            filter: light.filter.clone(),
            findings,
        };
        if self.is_empty() {
            return Ok(JudgedLight {
                result: result(Some(Vec::new())),
                header: kept.cloned(),
                read: false,
            });
        }
        let stamp = FileStamp::of(&light.path);
        let unchanged = kept.filter(|kept| kept.holds(&light.path, stamp));
        let (meta, read) = match unchanged {
            Some(kept) => (kept.meta.clone(), false),
            None => (
                crate::commands::import::headers::read_frame_meta(&light.path),
                true,
            ),
        };
        if !meta.readable {
            return Ok(JudgedLight {
                result: result(None),
                header: None,
                read,
            });
        }
        let findings = self.findings(&meta)?;
        Ok(JudgedLight {
            result: result(Some(findings)),
            header: stamp.map(|stamp| KeptHeader::new(&light.path, stamp, meta)),
            read,
        })
    }

    /// What the library would give this light, judged by its rig's rules.
    pub fn findings(&self, light: &FrameMeta) -> Result<Vec<LightFinding>> {
        let rules = self.book.for_light(light);
        Ok(self.judge(light, &rules)?.findings)
    }

    /// What the library would give this light, judged by `rules`: what the
    /// checks flag, and what a setting turned off.
    pub fn judge(&self, light: &FrameMeta, rules: &std::sync::Arc<Rules>) -> Result<Judgement> {
        let selected = self
            .snapshot
            .select_with(light, std::sync::Arc::clone(rules));
        let rules: &Rules = rules;
        let library = selected.library;
        let mut found = Gathered::new(rules);
        if self.has_frames
            && !library.bias
            && !library.dark
            && !library.flat
            && !library.unused_dark
        {
            found.push(
                Check::NoLibrary,
                poor(
                    HealthCode::NoLibrary,
                    "No calibration frames match its camera and settings",
                )
                .about([
                    CalibrationKind::Bias,
                    CalibrationKind::Dark,
                    CalibrationKind::DarkFlat,
                    CalibrationKind::Flat,
                ]),
            );
            return Ok(found.judgement);
        }
        let gaps = calibration_gaps(light, &selected, &self.nights, || Ok(self.flats.as_slice()))?;
        // One gap speaks for the light, in the gap rule's order (no flat,
        // then no bias or dark, then flats past the age limit): the first
        // flagged, else the first advisory. A gap whose check is off is
        // only noted.
        let speaks = gaps
            .iter()
            .find(|gap| gap.mode == CheckMode::Flag)
            .or_else(|| gaps.iter().find(|gap| gap.mode == CheckMode::Advisory));
        for gap in &gaps {
            let is_speaker = speaks.is_some_and(|speaker| std::ptr::eq(speaker, gap));
            if !is_speaker && gap.mode != CheckMode::Off {
                continue;
            }
            let kinds = if gap.check == Check::NoBiasOrDark {
                vec![CalibrationKind::Bias, CalibrationKind::Dark]
            } else {
                vec![CalibrationKind::Flat]
            };
            found.push(
                gap.check,
                poor(HealthCode::CannotCalibrate, gap.reason.clone()).about(kinds),
            );
        }
        let bias = MasterInput::of(CalibrationKind::Bias, &selected.bias, rules);
        let dark = MasterInput::of(CalibrationKind::Dark, &selected.dark, rules);
        let dark_flat = MasterInput::of(CalibrationKind::DarkFlat, &selected.dark_flat, rules);
        let flat = MasterInput::of(CalibrationKind::Flat, &selected.flat, rules);
        let thin = rules.thin_master_frames;
        let thin_note = || limit_note(rules, "thin_master_frames", || format!("fewer than {thin}"));

        if let MasterInput::Raw(darks) = &dark
            && !darks.is_empty()
            && darks.len() < thin
        {
            found.push(
                Check::ThinDark,
                poor(
                    HealthCode::ThinDark,
                    format!(
                        "Its dark master would use only {}{}",
                        frames(darks.len()),
                        thin_note()
                    ),
                )
                .about([CalibrationKind::Dark]),
            );
        }
        if let MasterInput::Raw(biases) = &bias
            && !biases.is_empty()
            && biases.len() < thin
        {
            // A dark carries the bias for the light; then a thin bias only
            // touches the flats.
            let reason = format!(
                "Its bias master would use only {}{}",
                frames(biases.len()),
                thin_note()
            );
            found.push(
                Check::ThinBias,
                if dark.applies() {
                    check(HealthCode::ThinBias, reason)
                } else {
                    poor(HealthCode::ThinBias, reason)
                }
                .about([CalibrationKind::Bias]),
            );
        }
        if let Some((grade, carried)) = self.flat_set_verdict(flat.raw(), rules) {
            let reason = format!(
                "Its flats carry {carried} changing spot{} into their master{}",
                if carried == 1 { "" } else { "s" },
                limit_note(rules, "grade_flat_carried_spots", || "spot limits".into())
            );
            match grade {
                Grade::Poor => found.push(
                    Check::FlatSpots,
                    poor(HealthCode::FlatSpots, reason).about([CalibrationKind::Flat]),
                ),
                Grade::Suspect => found.push(
                    Check::FlatSpots,
                    check(HealthCode::FlatSpots, reason).about([CalibrationKind::Flat]),
                ),
                Grade::Ok => {}
            }
        }
        if let (Some(flats_at), Some(light_at)) = (flat_session_at(&selected), light.timestamp) {
            let age = flats_at.abs_diff(light_at) as f64 / 86_400.0;
            // Past the limit, the gap rule has already spoken, unless its
            // check is off.
            let stale =
                age > rules.flat_max_age_days && rules.mode(Check::StaleFlats) != CheckMode::Off;
            if age > rules.flat_check_age_days && !stale {
                let note = limit_note(rules, "flat_check_age_days", || {
                    format!("more than {:.0} days", rules.flat_check_age_days)
                });
                found.push(
                    Check::OldFlats,
                    check(
                        HealthCode::OldFlats,
                        format!("Its flats are {age:.0} days from it{note}"),
                    )
                    .about([CalibrationKind::Flat]),
                );
            }
        }
        if let Some(finding) = self.mixed_flats(flat.raw(), rules) {
            found.push(Check::MixedFlats, finding);
        }
        // A change it cannot be placed against, or a family its own lights
        // point away from that nothing confirms.
        let flag = super::flat_groups::describe_flag(&selected.flat_basis, rules).or_else(|| {
            selected
                .flat_family
                .as_ref()
                .and_then(super::flat_groups::describe_family_flag)
        });
        if let Some(reason) = flag {
            found.push(
                Check::FlatsDiffer,
                check(HealthCode::FlatsDiffer, reason).about([CalibrationKind::Flat]),
            );
        }
        let mut suspect = 0;
        let mut suspect_kinds = Vec::new();
        for (kind, input) in [
            (CalibrationKind::Bias, &bias),
            (CalibrationKind::Dark, &dark),
            (CalibrationKind::DarkFlat, &dark_flat),
            (CalibrationKind::Flat, &flat),
        ] {
            let count = input
                .raw()
                .iter()
                .filter(|frame| self.suspect.contains(&frame.frame_uuid))
                .count();
            if count > 0 {
                suspect += count;
                suspect_kinds.push(kind);
            }
        }
        if suspect > 0 {
            found.push(
                Check::SuspectFrames,
                check(
                    HealthCode::SuspectFrames,
                    format!(
                        "Its masters use {suspect} suspect frame{}",
                        if suspect == 1 { "" } else { "s" }
                    ),
                )
                .about(suspect_kinds),
            );
        }
        for (kind, input, name) in [
            (CalibrationKind::Bias, &bias, "bias"),
            (CalibrationKind::Dark, &dark, "darks"),
            (CalibrationKind::Flat, &flat, "flats"),
        ] {
            let kept = selected
                .kept_out
                .iter()
                .any(|frame| frame.kind == kind && self.by_hand.contains(&frame.frame_uuid));
            let left = input.raw().len();
            if kept && !matches!(input, MasterInput::External) && left < thin {
                found.push(
                    Check::ShortAfterKeepOut,
                    check(
                        HealthCode::ShortAfterKeepOut,
                        format!(
                            "Frames kept out by hand leave its {name} at {}{}",
                            frames(left),
                            thin_note()
                        ),
                    )
                    .about([kind]),
                );
            }
        }
        Ok(found.judgement)
    }

    /// The mixed-flats check ([`Check::MixedFlats`], `check_mixed_flats`):
    /// the flats one master would take, counted by the night each was shot.
    /// More than one night is worth a look. The check's mode decides what
    /// [`Self::judge`] makes of the finding; this decides only whether
    /// there is one, so a rule about flats that measure alike can replace it
    /// here alone.
    fn mixed_flats(&self, flats: &[CalibrationFrame], rules: &Rules) -> Option<LightFinding> {
        let nights: BTreeSet<String> = flats
            .iter()
            .filter_map(|frame| {
                Some(self.nights.night_of_frame(
                    &frame.rig_uuid,
                    &frame.frame_uuid,
                    frame.captured_at?,
                ))
            })
            .collect();
        if nights.len() <= 1 {
            return None;
        }
        let finding =
            |text: String| Some(check(HealthCode::MixedFlats, text).about([CalibrationKind::Flat]));
        // Grouping by time: the check as it always was.
        if rules.flat_grouping != super::rules::FlatGrouping::Measured {
            return finding(format!(
                "Its flat master mixes flats from {} nights",
                nights.len()
            ));
        }
        // Nights whose flats were measured against each other and found
        // alike mix without a word.
        let book = self.snapshot.flat_book();
        let sessions: Option<BTreeSet<usize>> = flats
            .iter()
            .map(|frame| book.session_of_frame(&frame.frame_uuid))
            .collect();
        let sessions: Vec<usize> = sessions
            .map(|s| s.into_iter().collect())
            .unwrap_or_default();
        if !sessions.is_empty() && book.measured_alike(&sessions) {
            return None;
        }
        let compared = !sessions.is_empty()
            && sessions.iter().enumerate().all(|(i, a)| {
                sessions[i + 1..]
                    .iter()
                    .all(|b| book.comparison(*a, *b).is_some())
            });
        finding(if compared {
            format!(
                "Its flat master mixes flats from {} nights that measure unlike each other",
                nights.len()
            )
        } else {
            format!(
                "Its flat master mixes flats from {} nights not measured against each other",
                nights.len()
            )
        })
    }

    /// Seiza's master verdict on a flat set and how many changing spots it
    /// carries, from the stored measurements of the frames measured, by the
    /// light's flat limits; `None` with fewer than three, since the set
    /// check needs peers.
    fn flat_set_verdict(
        &self,
        flats: &[CalibrationFrame],
        rules: &Rules,
    ) -> Option<(Grade, usize)> {
        let mut uuids: Vec<&str> = flats
            .iter()
            .map(|frame| frame.frame_uuid.as_str())
            .filter(|uuid| self.flat_measurements.contains_key(*uuid))
            .collect();
        if uuids.len() < 3 {
            return None;
        }
        uuids.sort_unstable();
        let key = format!("{}|{:?}", uuids.join(","), rules.flat);
        if let Some(known) = self
            .flat_sets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key)
        {
            return *known;
        }
        let measurements: Vec<FlatMeasurement> = uuids
            .iter()
            .filter_map(|uuid| self.flat_measurements.get(*uuid).cloned())
            .collect();
        let verdict = seiza_calibration::screen_flats(&measurements, &rules.flat)
            .ok()
            .map(|report| (report.master.grade, report.carried_features));
        self.flat_sets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key, verdict);
        verdict
    }
}

/// The lights of a project that are not rejected, with their files found
/// in the image folders, and how many were not found.
pub fn project_lights(
    conn: &Connection,
    project_id: i32,
    directory_tree: &crate::directory_tree::DirectoryTree,
    nights: &super::LightNights,
) -> Result<(Vec<ProjectLight>, usize)> {
    let rows = crate::db::Database::new(conn)
        .query_images_scoped(None, Some(project_id), None, None, 0)
        .context("querying project lights")?;
    let mut lights = Vec::new();
    let mut missing = 0;
    for (image, _project, _target) in rows {
        if image.grading_status == 2 {
            continue;
        }
        let Some(path) = crate::utils::extract_filename(&image.metadata)
            .and_then(|basename| directory_tree.find_file_first(&basename).cloned())
        else {
            missing += 1;
            continue;
        };
        lights.push(ProjectLight {
            image_id: image.id,
            path,
            night: image
                .acquired_date
                .map(|at| nights.night_of(image.profile_id.as_deref(), at))
                .unwrap_or_else(|| "unknown".into()),
            filter: image.filter_name.clone(),
        });
    }
    Ok((lights, missing))
}

/// Gather judged lights into a project's health.
pub fn summarize(project_id: i32, results: Vec<LightResult>, missing: usize) -> ProjectHealth {
    type Group = (BTreeSet<i32>, BTreeMap<(String, String), usize>);
    let mut groups: BTreeMap<(std::cmp::Reverse<HealthLevel>, HealthCode, String), Group> =
        BTreeMap::new();
    let (mut checked, mut unreadable, mut poor_lights, mut check_lights) = (0, 0, 0, 0);
    for result in results {
        let Some(findings) = result.findings else {
            unreadable += 1;
            continue;
        };
        checked += 1;
        match findings.iter().map(|finding| finding.level).max() {
            Some(HealthLevel::Poor) => poor_lights += 1,
            Some(HealthLevel::Check) => check_lights += 1,
            None => {}
        }
        // A light counts once per finding, however often a rule fired.
        let distinct: BTreeSet<(std::cmp::Reverse<HealthLevel>, HealthCode, String)> = findings
            .into_iter()
            .map(|finding| {
                (
                    std::cmp::Reverse(finding.level),
                    finding.code,
                    finding.reason,
                )
            })
            .collect();
        for key in distinct {
            let group = groups.entry(key).or_default();
            group.0.insert(result.image_id);
            *group
                .1
                .entry((result.night.clone(), result.filter.clone()))
                .or_default() += 1;
        }
    }
    let findings = groups
        .into_iter()
        .map(|((level, code, reason), (image_ids, nights))| {
            let mut nights: Vec<NightFilterLights> = nights
                .into_iter()
                .map(|((night, filter), lights)| NightFilterLights {
                    night,
                    filter,
                    lights,
                })
                .collect();
            nights.sort_by(|left, right| {
                right
                    .night
                    .cmp(&left.night)
                    .then_with(|| left.filter.cmp(&right.filter))
            });
            HealthFinding {
                code,
                level: level.0,
                reason,
                lights: image_ids.len(),
                nights,
                image_ids: image_ids.into_iter().collect(),
            }
        })
        .collect();
    ProjectHealth {
        project_id,
        level: if poor_lights > 0 {
            Some(HealthLevel::Poor)
        } else if check_lights > 0 {
            Some(HealthLevel::Check)
        } else {
            None
        },
        checked,
        missing_files: missing + unreadable,
        poor_lights,
        check_lights,
        findings,
    }
}

/// Judge one project's lights one after another.
pub fn project_health(
    conn: &Connection,
    project_id: i32,
    directory_tree: &crate::directory_tree::DirectoryTree,
) -> Result<ProjectHealth> {
    let library = HealthLibrary::load(conn)?;
    let (lights, missing) = project_lights(
        conn,
        project_id,
        directory_tree,
        &library.light_nights(conn),
    )?;
    let results = lights
        .iter()
        .map(|light| library.judge_light(light))
        .collect::<Result<Vec<_>>>()?;
    Ok(summarize(project_id, results, missing))
}

fn hex(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn feed(hasher: &mut Sha256, value: rusqlite::types::ValueRef<'_>) {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => hasher.update(b"\x00"),
        ValueRef::Integer(number) => hasher.update(number.to_le_bytes()),
        ValueRef::Real(number) => hasher.update(number.to_le_bytes()),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => hasher.update(bytes),
    }
    hasher.update(b"\x1f");
}

/// Every row of `sql`, column by column, into one digest.
fn digest_rows(conn: &Connection, sql: &str, hasher: &mut Sha256) -> Result<()> {
    let mut statement = conn.prepare(sql)?;
    let columns = statement.column_count();
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        for index in 0..columns {
            feed(hasher, row.get_ref(index)?);
        }
        hasher.update(b"\x1e");
    }
    Ok(())
}

/// What the library holds and how it is graded: changes whenever a frame
/// is added, forgotten, moved, marked, graded, or chosen for or against.
pub fn library_digest(conn: &Connection) -> Result<String> {
    let mut hasher = Sha256::new();
    if !super::schema_exists(conn) {
        return Ok("none".into());
    }
    digest_rows(
        conn,
        "SELECT frame_uuid, source_fingerprint, updated_at, valid_direction, stray_light,
                is_master
         FROM psf_guard_calibration_frame ORDER BY frame_uuid",
        &mut hasher,
    )?;
    hasher.update(b"\x1d");
    if quality::table_exists(conn) {
        digest_rows(
            conn,
            "SELECT frame_uuid, grade, findings_json, override_choice, override_at, measured_at
             FROM psf_guard_calibration_frame_quality ORDER BY frame_uuid",
            &mut hasher,
        )?;
    }
    hasher.update(b"\x1d");
    // What flat grouping measured decides which flats a light takes.
    super::flat_groups::store::digest_rows(conn, &mut hasher)?;
    // So does which family the lights' own pixels chose.
    super::flat_groups::family_store::digest(conn, &mut hasher)?;
    // And what a person chose: splits, joins, and a light's side or group.
    super::flat_groups::override_store::digest(conn, &mut hasher)?;
    Ok(hex(hasher))
}

/// The settings that change what matches a light, with where nights split.
pub fn settings_digest(nights: &super::Nights) -> String {
    let boundary = nights.digest();
    let book = rule_book();
    let global = book.global();
    let mut digest = format!(
        "reach={};complete={};choice={:?};flat_age={};rotation={};external={:?};night={boundary};\
         flat_check={FLAT_CHECK_AGE_DAYS};thin={THIN_MASTER_FRAMES}",
        global.dark_reach_days,
        global.complete_dark_frames,
        global.dark_choice,
        global.flat_max_age_days,
        global.rotation_deg,
        global.external_masters,
    );
    // Every setting, a rig's included; nothing when none is set, so a
    // catalog nobody tuned keeps the keys it had.
    if let Some(rules) = book.digest() {
        digest.push_str(&format!(";rules={rules}"));
    }
    digest
}

/// Each project's lights as health reads them: which, whether rejected,
/// when, with what filter and file. One query for the whole catalog.
/// Accepting a light, or setting it back to pending, leaves the key alone:
/// only a rejected light leaves the lights judged.
pub fn project_light_digests(conn: &Connection) -> Result<BTreeMap<i32, String>> {
    let mut statement = conn.prepare(
        "SELECT ai.projectId, ai.Id, ai.gradingStatus = 2, ai.acquireddate, ai.filtername,
                CASE WHEN json_valid(ai.metadata) THEN json_extract(ai.metadata, '$.FileName') END
         FROM acquiredimage ai
         JOIN project p ON ai.projectId = p.Id
         JOIN target t ON ai.targetId = t.Id
         ORDER BY ai.projectId, ai.Id",
    )?;
    let mut rows = statement.query([])?;
    let mut hashers: BTreeMap<i32, Sha256> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let project: i32 = row.get(0)?;
        let hasher = hashers.entry(project).or_default();
        for index in 1..6 {
            feed(hasher, row.get_ref(index)?);
        }
        hasher.update(b"\x1e");
    }
    Ok(hashers
        .into_iter()
        .map(|(project, hasher)| (project, hex(hasher)))
        .collect())
}

/// Where each project's lights were found, with the image folders searched:
/// one changes when a light's file turns up, goes or moves, or the folders
/// change. Only the background pass compares it, since it needs the folder
/// tree; the views compare [`current_keys`], which needs only the catalog.
/// A project whose lights are all rejected has none.
pub fn project_file_digests(
    conn: &Connection,
    folders: &[String],
    tree: &crate::directory_tree::DirectoryTree,
) -> Result<BTreeMap<i32, String>> {
    let mut statement = conn.prepare(
        "SELECT ai.projectId, ai.Id,
                CASE WHEN json_valid(ai.metadata) THEN json_extract(ai.metadata, '$.FileName') END
         FROM acquiredimage ai
         JOIN project p ON ai.projectId = p.Id
         JOIN target t ON ai.targetId = t.Id
         WHERE ai.gradingStatus IS NOT 2
         ORDER BY ai.projectId, ai.Id",
    )?;
    let mut rows = statement.query([])?;
    let mut hashers: BTreeMap<i32, Sha256> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let project: i32 = row.get(0)?;
        let hasher = hashers.entry(project).or_insert_with(|| {
            let mut hasher = Sha256::new();
            for folder in folders {
                hasher.update(folder.as_bytes());
                hasher.update(b"\x1f");
            }
            hasher.update(b"\x1d");
            hasher
        });
        hasher.update(row.get::<_, i64>(1)?.to_le_bytes());
        hasher.update(b"\x1f");
        let found = row.get::<_, Option<String>>(2)?.and_then(|name| {
            tree.find_file_first(crate::utils::file_name_of(&name))
                .cloned()
        });
        match found {
            Some(path) => hasher.update(path.to_string_lossy().as_bytes()),
            None => hasher.update(b"\x00"),
        }
        hasher.update(b"\x1e");
    }
    Ok(hashers
        .into_iter()
        .map(|(project, hasher)| (project, hex(hasher)))
        .collect())
}

/// The key one project's kept result is filed under: the rules, the
/// library, the settings and the project's lights.
pub fn project_key(library: &str, settings: &str, lights: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!(
        "health-v{HEALTH_ALGORITHM_VERSION}\x1f{library}\x1f{settings}\x1f{lights}"
    ));
    hex(hasher)
}

/// The current key of every project in the catalog.
pub fn current_keys(conn: &Connection) -> Result<BTreeMap<i32, String>> {
    let library = library_digest(conn)?;
    let settings = settings_digest(&super::Nights::load(conn));
    Ok(project_light_digests(conn)?
        .into_iter()
        .map(|(project, lights)| (project, project_key(&library, &settings, &lights)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::quality::tests::{noise, write_frame};
    use super::super::quality::{measure_and_grade, set_overrides, QualityOverride};
    use super::super::tests::POLICY_LOCK;
    use super::super::{configure_flat_max_age, import_calibration_frames};
    use super::*;
    use crate::commands::import::headers::read_frame_meta;
    use std::path::Path;

    const SIZE: (usize, usize) = (128, 128);

    /// A Target Scheduler catalog with one project and one target, and a
    /// folder for its lights and calibration frames.
    struct Fixture {
        temp: tempfile::TempDir,
        conn: Connection,
        /// The telescope the next frames are shot with: five letters, to
        /// patch the header in place. Another telescope is another rig.
        scope: std::cell::Cell<&'static str>,
    }

    /// Write `to` over the first `from` in a file's header.
    fn patch(path: &Path, from: &[u8], to: &[u8]) {
        assert_eq!(from.len(), to.len());
        let mut bytes = std::fs::read(path).unwrap();
        let at = bytes
            .windows(from.len())
            .position(|window| window == from)
            .unwrap();
        bytes[at..at + to.len()].copy_from_slice(to);
        std::fs::write(path, bytes).unwrap();
    }

    fn timestamp(captured: &str) -> i64 {
        chrono::NaiveDateTime::parse_from_str(captured, "%Y-%m-%dT%H:%M:%S")
            .unwrap()
            .and_utc()
            .timestamp()
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let conn = Connection::open(temp.path().join("catalog.sqlite")).unwrap();
            crate::ts_schema::apply_schema(&conn).unwrap();
            conn.execute_batch(
                "INSERT INTO project (Id, profileId, name, guid) VALUES (1, 'p', 'M42', 'pg');
                 INSERT INTO project (Id, profileId, name, guid) VALUES (2, 'p', 'M31', 'pg2');
                 INSERT INTO target (Id, name, active, epochcode, projectId, guid)
                     VALUES (1, 'M42', 1, 0, 1, 'tg');
                 INSERT INTO target (Id, name, active, epochcode, projectId, guid)
                     VALUES (2, 'M31', 1, 0, 2, 'tg2');",
            )
            .unwrap();
            std::fs::create_dir_all(temp.path().join("frames")).unwrap();
            Self {
                temp,
                conn,
                scope: std::cell::Cell::new("Scope"),
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.temp.path().join("frames").join(name)
        }

        /// Put a frame on the fixture's current telescope.
        fn on_scope(&self, path: &Path) {
            let scope = self.scope.get();
            if scope != "Scope" {
                patch(
                    path,
                    b"TELESCOP= 'Scope'",
                    format!("TELESCOP= '{scope}'").as_bytes(),
                );
            }
        }

        /// A light of project 1 (or `project`), shot by `camera`.
        fn light(&self, id: i32, project: i32, captured: &str, camera: &str) {
            let name = format!("light-{id}.fits");
            let path = self.path(&name);
            write_frame(&path, "LIGHT", SIZE, captured, |_, _| 1_100.0);
            if camera != "Camera" {
                assert_eq!(camera.len(), "Camera".len());
                let bytes = std::fs::read(&path).unwrap();
                let from = b"INSTRUME= 'Camera'";
                let at = bytes
                    .windows(from.len())
                    .position(|window| window == from)
                    .unwrap();
                let mut bytes = bytes;
                bytes[at + 11..at + 17].copy_from_slice(camera.as_bytes());
                std::fs::write(&path, bytes).unwrap();
            }
            self.on_scope(&path);
            self.conn
                .execute(
                    "INSERT INTO acquiredimage
                        (Id, projectId, targetId, acquireddate, filtername, gradingStatus,
                         metadata, profileId)
                     VALUES (?1, ?2, ?2, ?3, 'Ha', 0, ?4, 'p')",
                    rusqlite::params![
                        id,
                        project,
                        timestamp(captured),
                        format!(r#"{{"FileName": "C:\\Lights\\{name}"}}"#),
                    ],
                )
                .unwrap();
        }

        /// `count` frames of `kind` at `level`, with a little noise, shot
        /// one minute apart from `captured`.
        fn frames(&mut self, kind: &str, count: usize, captured: &str, level: f64) -> Vec<String> {
            self.frames_with(kind, count, captured, |_, x, y| {
                level + 5.0 * noise(x, y, 1)
            })
        }

        fn frames_with(
            &mut self,
            kind: &str,
            count: usize,
            captured: &str,
            pixel: impl Fn(usize, usize, usize) -> f64,
        ) -> Vec<String> {
            let start = timestamp(captured);
            let mut metas = Vec::new();
            let mut names = Vec::new();
            for index in 0..count {
                let at = chrono::DateTime::from_timestamp(start + 60 * index as i64, 0)
                    .unwrap()
                    .format("%Y-%m-%dT%H:%M:%S")
                    .to_string();
                // Windows refuses `:` in a file name, so the name takes the
                // time with dashes; the header keeps the real one.
                let name = format!(
                    "{}-{}-{index}.fits",
                    kind.to_ascii_lowercase(),
                    at.replace(':', "-")
                );
                let path = self.path(&name);
                write_frame(&path, kind, SIZE, &at, |x, y| {
                    pixel(index, x, y) + 5.0 * noise(x, y, index as u64 + 7)
                });
                self.on_scope(&path);
                metas.push(read_frame_meta(&path));
                names.push(name);
            }
            let tx = self.conn.transaction().unwrap();
            import_calibration_frames(&tx, &metas, Some("profile")).unwrap();
            tx.commit().unwrap();
            names
        }

        fn uuid(&self, name: &str) -> String {
            self.conn
                .query_row(
                    "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE source_path LIKE ?1",
                    [format!("%{name}")],
                    |row| row.get(0),
                )
                .unwrap()
        }

        fn health(&self, project: i32) -> ProjectHealth {
            let tree = crate::directory_tree::DirectoryTree::build(self.temp.path()).unwrap();
            project_health(&self.conn, project, &tree).unwrap()
        }

        /// What a view of one light is told about its calibration.
        fn explain(&self, id: i32) -> super::super::CalibrationExplanation {
            let light = read_frame_meta(&self.path(&format!("light-{id}.fits")));
            super::super::explain_calibration(&self.conn, &light).unwrap()
        }
    }

    /// The kinds each of a light's findings is about, by code.
    fn finding_kinds(
        explained: &super::super::CalibrationExplanation,
    ) -> Vec<(HealthCode, Vec<CalibrationKind>)> {
        explained
            .findings
            .iter()
            .map(|finding| (finding.code, finding.kinds.clone()))
            .collect()
    }

    fn codes(health: &ProjectHealth) -> Vec<(HealthCode, HealthLevel)> {
        health
            .findings
            .iter()
            .map(|finding| (finding.code, finding.level))
            .collect()
    }

    fn reason(health: &ProjectHealth, code: HealthCode) -> String {
        health
            .findings
            .iter()
            .find(|finding| finding.code == code)
            .map(|finding| finding.reason.clone())
            .unwrap_or_default()
    }

    const LIGHT_AT: &str = "2026-09-10T04:00:00";

    #[test]
    fn a_camera_the_library_has_nothing_for_is_poor() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.light(2, 2, LIGHT_AT, "ASCOM1");
        // With no library at all, nothing is said about anyone.
        let health = fixture.health(2);
        assert_eq!(health.level, None);
        assert_eq!(health.checked, 1);

        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 10, "2026-09-09T13:00:00", 503.0);
        let health = fixture.health(2);
        assert_eq!(health.level, Some(HealthLevel::Poor));
        assert_eq!(codes(&health), [(HealthCode::NoLibrary, HealthLevel::Poor)]);
        assert_eq!(health.poor_lights, 1);
        let finding = &health.findings[0];
        assert_eq!(finding.lights, 1);
        assert_eq!(finding.nights.len(), 1);
        assert_eq!(finding.nights[0].filter, "Ha");
        // The camera the library holds frames for is fine.
        assert_eq!(fixture.health(1).level, None);
    }

    #[test]
    fn thin_masters_are_poor_and_a_thin_bias_beside_a_dark_is_to_check() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 3, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 3, "2026-09-09T13:00:00", 503.0);
        let health = fixture.health(1);
        assert_eq!(
            codes(&health),
            [
                (HealthCode::ThinDark, HealthLevel::Poor),
                (HealthCode::ThinBias, HealthLevel::Check),
            ]
        );
        assert_eq!(
            reason(&health, HealthCode::ThinDark),
            "Its dark master would use only 3 frames"
        );
        assert_eq!(
            finding_kinds(&fixture.explain(1)),
            [
                (HealthCode::ThinDark, vec![CalibrationKind::Dark]),
                (HealthCode::ThinBias, vec![CalibrationKind::Bias]),
            ]
        );

        // Enough darks: the bias alone is thin, and only worth a look.
        fixture.frames("DARK", 7, "2026-09-09T14:00:00", 503.0);
        let health = fixture.health(1);
        assert_eq!(codes(&health), [(HealthCode::ThinBias, HealthLevel::Check)]);
        assert_eq!(health.level, Some(HealthLevel::Check));
        assert_eq!((health.poor_lights, health.check_lights), (0, 1));
    }

    #[test]
    fn a_thin_bias_with_no_dark_is_poor() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 3, "2026-09-09T12:00:00", 500.0);
        assert_eq!(
            codes(&fixture.health(1)),
            [(HealthCode::ThinBias, HealthLevel::Poor)]
        );
    }

    #[test]
    fn flats_too_old_are_poor_and_old_ones_are_to_check() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        configure_flat_max_age(None);
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 10, "2026-09-09T13:00:00", 503.0);
        fixture.frames("FLAT", 5, "2026-07-02T02:00:00", 30_000.0);
        let health = fixture.health(1);
        assert_eq!(
            codes(&health),
            [(HealthCode::CannotCalibrate, HealthLevel::Poor)]
        );
        assert_eq!(
            reason(&health, HealthCode::CannotCalibrate),
            "Its flats are 70 days from it, past the 60-day limit"
        );
        assert_eq!(
            finding_kinds(&fixture.explain(1)),
            [(HealthCode::CannotCalibrate, vec![CalibrationKind::Flat])]
        );
        // Lifting the limit leaves them only old.
        configure_flat_max_age(Some(0.0));
        let health = fixture.health(1);
        configure_flat_max_age(None);
        assert_eq!(codes(&health), [(HealthCode::OldFlats, HealthLevel::Check)]);
        assert_eq!(
            reason(&health, HealthCode::OldFlats),
            "Its flats are 70 days from it"
        );
    }

    #[test]
    fn a_flat_master_from_two_nights_is_to_check() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 10, "2026-09-09T13:00:00", 503.0);
        // The lights' quiet hours put the night split at 16:30 UTC.
        fixture.frames("FLAT", 3, "2026-09-10T15:00:00", 30_000.0);
        fixture.frames("FLAT", 3, "2026-09-10T18:00:00", 30_000.0);
        let health = fixture.health(1);
        assert_eq!(
            codes(&health),
            [(HealthCode::MixedFlats, HealthLevel::Check)]
        );
        assert_eq!(
            reason(&health, HealthCode::MixedFlats),
            "Its flat master mixes flats from 2 nights not measured against each other"
        );

        // A view of the light names the finding, its kind, and both nights.
        let explained = fixture.explain(1);
        assert_eq!(
            finding_kinds(&explained),
            [(HealthCode::MixedFlats, vec![CalibrationKind::Flat])]
        );
        let flat = explained
            .kinds
            .iter()
            .find(|kind| kind.kind == "flat")
            .unwrap();
        assert_eq!(
            flat.used
                .iter()
                .map(|night| (night.night.as_str(), night.frames))
                .collect::<Vec<_>>(),
            [("2026-09-10", 3), ("2026-09-09", 3)]
        );

        // Measured, the two nights' flats are alike: they mix without a word.
        crate::calibration::flat_groups::store::add_tables(&fixture.conn).unwrap();
        let responses = fixture.temp.path().join("responses");
        let outcome =
            crate::calibration::flat_groups::job::run(crate::calibration::flat_groups::job::Pass {
                conn: &fixture.conn,
                dir: &responses,
                scope: crate::calibration::flat_groups::job::Scope::everything(),
                keep_going: &|| true,
                yield_now: &|| {},
                progress: &mut |_| {},
                goes_first: &|_| false,
                find_light: &|_| None,
                pool: None,
            })
            .unwrap();
        assert_eq!((outcome.measured, outcome.compared), (2, 1));
        assert_eq!(fixture.health(1).level, None);
        let alike = |explained: &crate::calibration::CalibrationExplanation| {
            explained
                .kinds
                .iter()
                .find(|kind| kind.kind == "flat")
                .and_then(|kind| kind.alike.as_ref().map(|alike| alike.nights))
        };
        assert_eq!(alike(&fixture.explain(1)), Some(2));

        // Grouping by time, the check is as it always was, measured or not,
        // and the light's account says nothing of measurements.
        let _rules = crate::calibration::tests::RulesInForce::global(&[(
            "flat_grouping",
            serde_json::json!("time"),
        )]);
        assert_eq!(
            reason(&fixture.health(1), HealthCode::MixedFlats),
            "Its flat master mixes flats from 2 nights"
        );
        assert_eq!(alike(&fixture.explain(1)), None);
    }

    #[test]
    fn keeping_darks_out_by_hand_can_leave_the_set_short() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        let darks = fixture.frames("DARK", 6, "2026-09-09T13:00:00", 503.0);
        assert_eq!(fixture.health(1).level, None);
        let kept: Vec<String> = darks[..2].iter().map(|name| fixture.uuid(name)).collect();
        set_overrides(&fixture.conn, &kept, Some(QualityOverride::Exclude)).unwrap();
        let health = fixture.health(1);
        assert_eq!(
            codes(&health),
            [
                (HealthCode::ThinDark, HealthLevel::Poor),
                (HealthCode::ShortAfterKeepOut, HealthLevel::Check),
            ]
        );
        assert_eq!(
            reason(&health, HealthCode::ShortAfterKeepOut),
            "Frames kept out by hand leave its darks at 4 frames"
        );
    }

    #[test]
    fn flats_carrying_changing_spots_and_suspect_frames_are_reported() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 10, "2026-09-09T13:00:00", 503.0);
        // Twelve spots in every flat, fading from 16% to 3%: too few in one
        // frame to make it poor, but most frames hold them, so a master
        // would keep them.
        let depths = [0.16, 0.10, 0.05, 0.03];
        fixture.frames_with("FLAT", 4, "2026-09-10T02:00:00", |index, x, y| {
            let spot = (16..112).contains(&x)
                && (16..88).contains(&y)
                && (x - 16) % 24 < 4
                && (y - 16) % 24 < 4;
            30_000.0 * if spot { 1.0 - depths[index] } else { 1.0 }
        });
        let graded = measure_and_grade(&fixture.conn).unwrap().1;
        assert_eq!(graded.poor, 0, "{graded:?}");
        assert!(graded.suspect >= 3, "{graded:?}");
        let health = fixture.health(1);
        assert_eq!(health.level, Some(HealthLevel::Poor), "{health:?}");
        assert!(codes(&health).contains(&(HealthCode::FlatSpots, HealthLevel::Poor)));
        assert!(codes(&health).contains(&(HealthCode::SuspectFrames, HealthLevel::Check)));
        assert!(
            reason(&health, HealthCode::FlatSpots).starts_with("Its flats carry 12 changing spots"),
            "{health:?}"
        );

        // A view of the light names the suspect flats the master takes.
        let explained = fixture.explain(1);
        let kinds = finding_kinds(&explained);
        assert!(kinds.contains(&(HealthCode::FlatSpots, vec![CalibrationKind::Flat])));
        assert!(kinds.contains(&(HealthCode::SuspectFrames, vec![CalibrationKind::Flat])));
        assert_eq!(explained.findings[0].level, HealthLevel::Poor, "poor first");
        let flat = explained
            .kinds
            .iter()
            .find(|kind| kind.kind == "flat")
            .unwrap();
        assert_eq!(flat.suspect.len(), graded.suspect);
        assert!(flat
            .suspect
            .iter()
            .all(|frame| frame.file.starts_with("flat-") && !frame.reason.is_empty()));
    }

    #[test]
    fn a_suspect_frame_kept_out_by_hand_no_longer_flags_the_lights() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        let darks = fixture.frames("DARK", 10, "2026-09-09T13:00:00", 503.0);
        measure_and_grade(&fixture.conn).unwrap();
        let uuid = fixture.uuid(&darks[0]);
        fixture
            .conn
            .execute(
                "UPDATE psf_guard_calibration_frame_quality SET grade = 'suspect' WHERE frame_uuid = ?1",
                [&uuid],
            )
            .unwrap();
        assert!(
            codes(&fixture.health(1)).contains(&(HealthCode::SuspectFrames, HealthLevel::Check))
        );

        // Kept out, the master no longer takes it, and nine darks are plenty.
        set_overrides(&fixture.conn, &[uuid], Some(QualityOverride::Exclude)).unwrap();
        let health = fixture.health(1);
        assert_eq!(codes(&health), Vec::new(), "{health:?}");
        assert_eq!(health.level, None);
    }

    /// A light whose dark master takes a suspect frame shows the suspect
    /// Check. Once a person uses that frame anyway, the light reads exactly
    /// as it would with the frame graded ok: no Check, no flag, nothing
    /// that says a grade was overruled.
    #[test]
    fn a_suspect_frame_used_anyway_scores_as_an_ok_one() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        let darks = fixture.frames("DARK", 10, "2026-09-09T13:00:00", 503.0);
        measure_and_grade(&fixture.conn).unwrap();
        let uuid = fixture.uuid(&darks[0]);
        let ok = fixture.health(1);
        assert_eq!(ok.level, None, "{ok:?}");
        let ok_findings = fixture.explain(1).findings;
        assert!(ok_findings.is_empty(), "{ok_findings:?}");

        fixture
            .conn
            .execute(
                "UPDATE psf_guard_calibration_frame_quality
                 SET grade = 'suspect', reason = 'noise' WHERE frame_uuid = ?1",
                [&uuid],
            )
            .unwrap();
        let flagged = fixture.health(1);
        assert_eq!(flagged.level, Some(HealthLevel::Check));
        assert_eq!(flagged.check_lights, 1);
        assert_eq!(
            codes(&flagged),
            [(HealthCode::SuspectFrames, HealthLevel::Check)]
        );

        set_overrides(
            &fixture.conn,
            std::slice::from_ref(&uuid),
            Some(QualityOverride::Use),
        )
        .unwrap();
        let used = fixture.health(1);
        assert_eq!(used, ok, "scores as the ok frame did");
        assert_eq!((used.check_lights, used.poor_lights), (0, 0));
        assert!(used
            .findings
            .iter()
            .all(|finding| finding.image_ids.is_empty()));
        assert_eq!(fixture.explain(1).findings, ok_findings);
    }

    /// Using suspect flats anyway overrules their grade: the light loses
    /// its suspect finding and its view stops listing them, while the
    /// masters, which took them already, keep their keys. A regrade keeps
    /// the choice; clearing it brings the finding back.
    #[test]
    fn suspect_frames_used_anyway_no_longer_flag_the_lights() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 10, "2026-09-09T13:00:00", 503.0);
        // The spotted flats of the test above: suspect, none poor.
        let depths = [0.16, 0.10, 0.05, 0.03];
        fixture.frames_with("FLAT", 4, "2026-09-10T02:00:00", |index, x, y| {
            let spot = (16..112).contains(&x)
                && (16..88).contains(&y)
                && (x - 16) % 24 < 4
                && (y - 16) % 24 < 4;
            30_000.0 * if spot { 1.0 - depths[index] } else { 1.0 }
        });
        let graded = measure_and_grade(&fixture.conn).unwrap().1;
        assert_eq!(graded.poor, 0, "{graded:?}");
        let suspect: Vec<String> = fixture
            .conn
            .prepare(
                "SELECT frame_uuid FROM psf_guard_calibration_frame_quality
                 WHERE grade = 'suspect' ORDER BY frame_uuid",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(suspect.len() >= 3, "{graded:?}");
        let flagged = |fixture: &Fixture| {
            codes(&fixture.health(1)).contains(&(HealthCode::SuspectFrames, HealthLevel::Check))
        };
        let listed = |fixture: &Fixture| {
            fixture
                .explain(1)
                .kinds
                .iter()
                .find(|kind| kind.kind == "flat")
                .unwrap()
                .suspect
                .len()
        };
        let light = fixture.path("light-1.fits");
        let cache = fixture.temp.path().join("cache");
        let build = |fixture: &Fixture| {
            super::super::resolve_or_build_masters(
                &fixture.conn,
                &cache,
                std::slice::from_ref(&light),
                None,
                None,
                super::super::CalibrationMode::Auto,
            )
            .unwrap()
            .1
        };
        assert!(flagged(&fixture));
        assert_eq!(listed(&fixture), suspect.len());
        let before = build(&fixture);
        assert_eq!(before.flat_frames, 4, "a suspect flat stays in");
        let scope = super::super::project_scope::scope_revision(&fixture.conn, 1).unwrap();
        let keys = current_keys(&fixture.conn).unwrap();

        assert_eq!(
            set_overrides(&fixture.conn, &suspect, Some(QualityOverride::Use)).unwrap(),
            suspect.len()
        );
        assert!(!flagged(&fixture));
        assert_eq!(listed(&fixture), 0);
        // The spots the set carries into its master are the set's finding,
        // not a frame's grade, so they still show.
        assert!(codes(&fixture.health(1)).contains(&(HealthCode::FlatSpots, HealthLevel::Poor)));
        let counts = quality::quality_counts(&fixture.conn).unwrap();
        assert_eq!(
            (counts.suspect, counts.used_anyway),
            (0, suspect.len()),
            "{counts:?}"
        );
        // Health is judged again, but nothing a master or a light's scope
        // reads has moved.
        assert_ne!(current_keys(&fixture.conn).unwrap(), keys);
        assert_eq!(
            super::super::project_scope::scope_revision(&fixture.conn, 1).unwrap(),
            scope
        );
        let after = build(&fixture);
        assert_eq!(
            (
                &after.bias_master,
                &after.dark_master,
                &after.flat_master,
                &after.masters_signature
            ),
            (
                &before.bias_master,
                &before.dark_master,
                &before.flat_master,
                &before.masters_signature
            )
        );

        // A regrade leaves the grade suspect and the choice standing.
        let regraded = quality::grade_library(&fixture.conn).unwrap();
        assert_eq!(regraded.suspect, suspect.len(), "{regraded:?}");
        assert!(!flagged(&fixture));

        // Cleared, the grade decides again.
        set_overrides(&fixture.conn, &suspect, None).unwrap();
        assert!(flagged(&fixture));
        assert_eq!(listed(&fixture), suspect.len());
        assert_eq!(
            quality::quality_counts(&fixture.conn).unwrap().used_anyway,
            0
        );
    }

    #[test]
    fn findings_count_lights_by_night_and_filter_and_skip_rejects() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 2, "2026-09-10T04:00:00", "ASCOM1");
        fixture.light(2, 2, "2026-09-10T05:00:00", "ASCOM1");
        fixture.light(3, 2, "2026-09-12T04:00:00", "ASCOM1");
        fixture.light(4, 2, "2026-09-12T04:30:00", "ASCOM1");
        fixture
            .conn
            .execute(
                "UPDATE acquiredimage SET gradingStatus = 2 WHERE Id = 4",
                [],
            )
            .unwrap();
        std::fs::remove_file(fixture.path("light-3.fits")).unwrap();
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        let health = fixture.health(2);
        assert_eq!((health.checked, health.missing_files), (2, 1));
        let finding = &health.findings[0];
        assert_eq!(finding.lights, 2);
        assert_eq!(finding.image_ids, vec![1, 2]);
        assert_eq!(
            finding.nights,
            [NightFilterLights {
                night: "2026-09-09".into(),
                filter: "Ha".into(),
                lights: 2
            }]
        );
    }

    #[test]
    fn a_project_key_follows_its_lights_the_library_and_the_settings() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        configure_flat_max_age(None);
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.light(2, 2, LIGHT_AT, "Camera");
        let darks = fixture.frames("DARK", 6, "2026-09-09T13:00:00", 503.0);
        let keys = current_keys(&fixture.conn).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(current_keys(&fixture.conn).unwrap(), keys, "stable");

        // Accepting a light leaves the key alone: only a rejected light
        // leaves the lights judged.
        fixture
            .conn
            .execute(
                "UPDATE acquiredimage SET gradingStatus = 1 WHERE Id = 1",
                [],
            )
            .unwrap();
        assert_eq!(current_keys(&fixture.conn).unwrap(), keys);

        // A light rejected in one project changes that project's key only.
        fixture
            .conn
            .execute(
                "UPDATE acquiredimage SET gradingStatus = 2 WHERE Id = 1",
                [],
            )
            .unwrap();
        let after = current_keys(&fixture.conn).unwrap();
        assert_ne!(after[&1], keys[&1]);
        assert_eq!(after[&2], keys[&2]);

        // A frame kept out by hand changes every key.
        let uuid = fixture.uuid(&darks[0]);
        set_overrides(&fixture.conn, &[uuid], Some(QualityOverride::Exclude)).unwrap();
        let kept_out = current_keys(&fixture.conn).unwrap();
        assert_ne!(kept_out[&2], after[&2]);

        // So do a new frame, a grade and a setting.
        fixture.frames("BIAS", 1, "2026-09-09T12:00:00", 500.0);
        let more = current_keys(&fixture.conn).unwrap();
        assert_ne!(more[&2], kept_out[&2]);
        measure_and_grade(&fixture.conn).unwrap();
        let graded = current_keys(&fixture.conn).unwrap();
        assert_ne!(graded[&2], more[&2]);
        configure_flat_max_age(Some(90.0));
        let settings = current_keys(&fixture.conn).unwrap();
        configure_flat_max_age(None);
        assert_ne!(settings[&2], graded[&2]);
        assert_eq!(current_keys(&fixture.conn).unwrap(), graded);
    }

    #[test]
    fn file_digests_follow_where_the_lights_are_found() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.light(2, 1, LIGHT_AT, "Camera");
        fixture.light(3, 2, LIGHT_AT, "Camera");
        let folders = vec![fixture.temp.path().display().to_string()];
        let digests = |folders: &[String]| {
            let tree = crate::directory_tree::DirectoryTree::build(fixture.temp.path()).unwrap();
            project_file_digests(&fixture.conn, folders, &tree).unwrap()
        };
        let found = digests(&folders);
        assert_eq!(found.len(), 2);
        assert_eq!(digests(&folders), found, "stable");

        // A light's file goes: its project's digest changes, the other's
        // does not; it comes back, and the digest with it.
        let aside = fixture.temp.path().join("light-1.aside");
        std::fs::rename(fixture.path("light-1.fits"), &aside).unwrap();
        let missing = digests(&folders);
        assert_ne!(missing[&1], found[&1]);
        assert_eq!(missing[&2], found[&2]);
        std::fs::rename(&aside, fixture.path("light-1.fits")).unwrap();
        assert_eq!(digests(&folders), found);

        // Other image folders change every project's digest.
        let other = digests(&[folders[0].clone(), "/elsewhere".into()]);
        assert_ne!(other[&1], found[&1]);
        assert_ne!(other[&2], found[&2]);

        // A rejected light is not judged, so where it is does not count.
        fixture
            .conn
            .execute(
                "UPDATE acquiredimage SET gradingStatus = 2 WHERE Id = 3",
                [],
            )
            .unwrap();
        assert!(!digests(&folders).contains_key(&2));
    }

    #[test]
    fn a_kept_header_is_read_again_only_when_its_file_changes() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        let library = HealthLibrary::load(&fixture.conn).unwrap();
        let light = ProjectLight {
            image_id: 1,
            path: fixture.path("light-1.fits"),
            night: "2026-09-09".into(),
            filter: "Ha".into(),
        };
        let first = library.judge_light_kept(&light, None).unwrap();
        assert!(first.read);
        let header = first
            .header
            .clone()
            .expect("a readable light keeps its header");
        assert!(header.meta.readable);

        // The header survives a trip through JSON, as the catalog keeps it.
        let header: KeptHeader =
            serde_json::from_slice(&serde_json::to_vec(&header).unwrap()).unwrap();
        let again = library.judge_light_kept(&light, Some(&header)).unwrap();
        assert!(!again.read, "an unchanged file is not read again");
        assert_eq!(again.result.findings, first.result.findings);

        // A file written again is read again.
        let bytes = std::fs::read(&light.path).unwrap();
        std::fs::write(&light.path, [bytes.as_slice(), &[0; 2880]].concat()).unwrap();
        assert!(
            library
                .judge_light_kept(&light, Some(&header))
                .unwrap()
                .read
        );

        // So is one found somewhere else.
        let moved = ProjectLight {
            path: fixture.path("light-1-moved.fits"),
            ..light.clone()
        };
        std::fs::copy(&light.path, &moved.path).unwrap();
        assert!(
            library
                .judge_light_kept(&moved, Some(&header))
                .unwrap()
                .read
        );
    }

    #[test]
    fn an_unreadable_light_counts_as_missing() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        std::fs::write(fixture.path("light-1.fits"), b"not a fits file").unwrap();
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        let health = fixture.health(1);
        assert_eq!((health.checked, health.missing_files), (0, 1));
        assert_eq!(health.level, None);
    }

    // ---------- Tunable checks ----------

    use super::super::tests::RulesInForce;
    use super::super::{rules, RuleSource};
    use serde_json::json;

    fn levels(findings: &[LightFinding]) -> Vec<(HealthCode, HealthLevel)> {
        findings
            .iter()
            .map(|finding| (finding.code, finding.level))
            .collect()
    }

    #[test]
    fn a_thin_master_is_flagged_below_the_limit_and_not_at_or_above_it() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 4, "2026-09-09T13:00:00", 503.0);
        // The default limit is five: four darks are thin.
        assert_eq!(
            codes(&fixture.health(1)),
            [(HealthCode::ThinDark, HealthLevel::Poor)]
        );
        for (limit, flagged) in [(3, false), (4, false), (5, true), (6, true)] {
            let _rules = RulesInForce::global(&[("thin_master_frames", json!(limit))]);
            let health = fixture.health(1);
            assert_eq!(
                !health.findings.is_empty(),
                flagged,
                "four darks against a limit of {limit}"
            );
        }
        let _rules = RulesInForce::global(&[("thin_master_frames", json!(6))]);
        assert_eq!(
            reason(&fixture.health(1), HealthCode::ThinDark),
            "Its dark master would use only 4 frames (fewer than 6, set globally)"
        );
    }

    #[test]
    fn a_check_can_be_made_advisory_or_turned_off() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 3, "2026-09-09T13:00:00", 503.0);
        assert_eq!(
            codes(&fixture.health(1)),
            [(HealthCode::ThinDark, HealthLevel::Poor)]
        );
        {
            let _rules = RulesInForce::global(&[("check_thin_dark", json!("advisory"))]);
            let health = fixture.health(1);
            assert_eq!(codes(&health), [(HealthCode::ThinDark, HealthLevel::Check)]);
            assert_eq!(
                reason(&health, HealthCode::ThinDark),
                "Its dark master would use only 3 frames (advisory, set globally)"
            );
        }
        let _rules = RulesInForce::global(&[("check_thin_dark", json!("off"))]);
        assert_eq!(fixture.health(1).level, None);
        // The light's own account says what was not flagged, and why.
        let explained = fixture.explain(1);
        assert!(explained.findings.is_empty());
        assert_eq!(
            explained
                .waived
                .iter()
                .map(|finding| finding.reason.as_str())
                .collect::<Vec<_>>(),
            ["Its dark master would use only 3 frames (check off, set globally)"]
        );
    }

    #[test]
    fn an_advisory_gap_is_shown_but_never_leaves_a_light_out_of_a_stack() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 10, "2026-09-09T13:00:00", 503.0);
        fixture.frames("FLAT", 5, "2026-07-02T02:00:00", 30_000.0);
        let light = read_frame_meta(&fixture.path("light-1.fits"));
        let gap = || {
            let selected = super::super::select_for_light(&fixture.conn, &light).unwrap();
            super::super::calibration_gap(
                &fixture.conn,
                &light,
                &selected,
                &super::super::Nights::fixed(0),
            )
            .unwrap()
        };
        assert_eq!(
            gap().as_deref(),
            Some("Its flats are 70 days from it, past the 60-day limit")
        );
        {
            let _rules = RulesInForce::global(&[("check_stale_flats", json!("advisory"))]);
            assert_eq!(gap(), None, "a stack keeps it");
            let health = fixture.health(1);
            assert_eq!(
                codes(&health),
                [(HealthCode::CannotCalibrate, HealthLevel::Check)]
            );
            assert_eq!(
                reason(&health, HealthCode::CannotCalibrate),
                "Its flats are 70 days from it, past the 60-day limit (advisory, set globally)"
            );
        }
        // Off, the age is still worth a look: past 30 days.
        let _rules = RulesInForce::global(&[("check_stale_flats", json!("off"))]);
        assert_eq!(gap(), None);
        assert_eq!(
            codes(&fixture.health(1)),
            [(HealthCode::OldFlats, HealthLevel::Check)]
        );
    }

    #[test]
    fn flats_are_worth_a_look_past_the_check_age_and_not_at_or_before_it() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 10, "2026-09-09T13:00:00", 503.0);
        // 40 days before the light.
        fixture.frames("FLAT", 5, "2026-08-01T04:00:00", 30_000.0);
        assert_eq!(
            codes(&fixture.health(1)),
            [(HealthCode::OldFlats, HealthLevel::Check)]
        );
        for (days, flagged) in [(39, true), (40, false), (41, false), (0, false)] {
            let _rules = RulesInForce::global(&[("flat_check_age_days", json!(days))]);
            assert_eq!(
                !fixture.health(1).findings.is_empty(),
                flagged,
                "40-day-old flats against {days} days"
            );
        }
        let _rules = RulesInForce::global(&[("flat_check_age_days", json!(39))]);
        assert_eq!(
            reason(&fixture.health(1), HealthCode::OldFlats),
            "Its flats are 40 days from it (more than 39 days, set globally)"
        );
    }

    #[test]
    fn one_rigs_settings_change_only_its_own_lights() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        // Two rigs on one camera, each with flats 70 days before its light.
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 10, "2026-09-09T13:00:00", 503.0);
        fixture.frames("FLAT", 5, "2026-07-02T02:00:00", 30_000.0);
        fixture.scope.set("Other");
        fixture.light(2, 2, LIGHT_AT, "Camera");
        fixture.frames("FLAT", 5, "2026-07-02T03:00:00", 30_000.0);
        let scope = rules::light_rig_uuid(&read_frame_meta(&fixture.path("light-1.fits")));
        let other = rules::light_rig_uuid(&read_frame_meta(&fixture.path("light-2.fits")));
        assert_ne!(scope, other);
        let stale = [(HealthCode::CannotCalibrate, HealthLevel::Poor)];
        assert_eq!(codes(&fixture.health(1)), stale);
        assert_eq!(codes(&fixture.health(2)), stale);

        // A longer limit for the first rig: its lights are only worth a
        // look; the other rig's stay poor.
        let _rules = RulesInForce::with(
            &[],
            &[(
                scope.as_str(),
                "Scope · Camera",
                &[("flat_max_age_days", json!(90))],
            )],
        );
        assert_eq!(
            codes(&fixture.health(1)),
            [(HealthCode::OldFlats, HealthLevel::Check)]
        );
        assert_eq!(codes(&fixture.health(2)), stale);
        let explained = fixture.explain(1);
        assert!(explained.rules.rig_has_settings);
        assert_eq!(
            explained
                .rules
                .notes
                .iter()
                .map(|note| (note.key, note.text.as_str(), note.source))
                .collect::<Vec<_>>(),
            [("flat_max_age_days", "90 days", RuleSource::Rig)]
        );
        assert!(!fixture.explain(2).rules.rig_has_settings);
    }

    #[test]
    fn a_rig_without_darks_is_not_flagged_for_lacking_them() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        // Three darks shot uncapped are all the library holds for it.
        fixture.frames("DARK", 3, "2026-09-09T13:00:00", 503.0);
        assert_eq!(
            codes(&fixture.health(1)),
            [(HealthCode::ThinDark, HealthLevel::Poor)]
        );
        let light = read_frame_meta(&fixture.path("light-1.fits"));
        let _rules = RulesInForce::with(
            &[],
            &[(
                rules::light_rig_uuid(&light).as_str(),
                "Scope · Camera",
                &[("use_darks", json!(false))],
            )],
        );
        // Not thin, not lacking a bias or dark, and not a camera the
        // library holds nothing for.
        let health = fixture.health(1);
        assert_eq!(
            health.level,
            None,
            "{:?}",
            levels(&fixture.explain(1).findings)
        );
    }

    #[test]
    fn a_rigs_flat_grade_limits_decide_its_spots_and_suspect_flats() {
        let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut fixture = Fixture::new();
        fixture.light(1, 1, LIGHT_AT, "Camera");
        fixture.frames("BIAS", 10, "2026-09-09T12:00:00", 500.0);
        fixture.frames("DARK", 10, "2026-09-09T13:00:00", 503.0);
        // Twelve spots in every flat, fading from 16% to 3%, as above.
        let depths = [0.16, 0.10, 0.05, 0.03];
        fixture.frames_with("FLAT", 4, "2026-09-10T02:00:00", |index, x, y| {
            let spot = (16..112).contains(&x)
                && (16..88).contains(&y)
                && (x - 16) % 24 < 4
                && (y - 16) % 24 < 4;
            30_000.0 * if spot { 1.0 - depths[index] } else { 1.0 }
        });
        measure_and_grade(&fixture.conn).unwrap();
        let health = fixture.health(1);
        assert!(codes(&health).contains(&(HealthCode::FlatSpots, HealthLevel::Poor)));
        assert!(codes(&health).contains(&(HealthCode::SuspectFrames, HealthLevel::Check)));
        let rig = rules::light_rig_uuid(&read_frame_meta(&fixture.path("light-1.fits")));
        let spots = |limits: serde_json::Value| {
            let _rules = RulesInForce::with(
                &[],
                &[(
                    rig.as_str(),
                    "Scope · Camera",
                    &[("grade_flat_carried_spots", limits)],
                )],
            );
            fixture
                .health(1)
                .findings
                .iter()
                .find(|finding| finding.code == HealthCode::FlatSpots)
                .map(|finding| finding.level)
        };
        // Twelve carried spots against the limits: at the poor limit, at the
        // suspect limit, and below both.
        assert_eq!(
            spots(json!({"suspect": 1, "poor": 12})),
            Some(HealthLevel::Poor)
        );
        assert_eq!(
            spots(json!({"suspect": 12, "poor": 13})),
            Some(HealthLevel::Check)
        );
        assert_eq!(spots(json!({"suspect": 13, "poor": null})), None);

        // The flats' own grades follow the rig's limits once grading runs.
        let _rules = RulesInForce::with(
            &[],
            &[(
                rig.as_str(),
                "Scope · Camera",
                &[(
                    "grade_flat_changing_spots",
                    json!({"suspect": 50, "poor": null}),
                )],
            )],
        );
        let graded = measure_and_grade(&fixture.conn).unwrap().1;
        assert_eq!((graded.suspect, graded.poor), (0, 0), "{graded:?}");
        assert!(
            !codes(&fixture.health(1)).contains(&(HealthCode::SuspectFrames, HealthLevel::Check))
        );
    }
}
