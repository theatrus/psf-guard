//! Group scanned frames into a Target Scheduler project/target/exposure plan.
//!
//! Grouping rules (see CREATE_IMPORT_PLAN.md §2):
//! - Frames never share a project across different equipment signatures
//!   (telescope, camera, focal length, binning).
//! - Each distinct OBJECT becomes its own project and target by default.
//! - Targets with a shared panel-style name, nearby coordinates, and nearby
//!   capture dates share one project as a likely mosaic.
//! - Within a target, each distinct (filter, gain, offset, binning, readout,
//!   exposure) is one exposure plan referencing a shared exposure template.
//!
//! The plan is deterministic for a given frame set, so `--dry-run` previews
//! exactly what a real run inserts.

use super::headers::FrameMeta;
use std::collections::BTreeMap;

/// Equipment that must not be mixed within one project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct EquipmentSignature {
    pub telescope: String,
    pub camera: String,
    /// Focal length rounded to whole mm (0 when unknown).
    pub focal_mm: i64,
    pub binning: (i64, i64),
}

impl EquipmentSignature {
    pub fn of(meta: &FrameMeta) -> Self {
        Self {
            telescope: meta.telescope.clone().unwrap_or_default(),
            camera: meta.camera.clone().unwrap_or_default(),
            focal_mm: meta.focal_length_mm.map(|f| f.round() as i64).unwrap_or(0),
            binning: (meta.binning_x.unwrap_or(1), meta.binning_y.unwrap_or(1)),
        }
    }

    /// Short human-readable rig description for auto-generated project names.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.telescope.is_empty() {
            parts.push(self.telescope.clone());
        } else if !self.camera.is_empty() {
            parts.push(self.camera.clone());
        }
        if self.focal_mm > 0 {
            parts.push(format!("{}mm", self.focal_mm));
        }
        if self.binning != (1, 1) {
            parts.push(format!("{}x{}", self.binning.0, self.binning.1));
        }
        if parts.is_empty() {
            "Imported".to_string()
        } else {
            parts.join(" ")
        }
    }
}

/// One exposure template: TS shares these per profile across projects.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TemplateKey {
    pub filter: String,
    pub gain: i64,    // -1 = camera default (TS convention)
    pub offset: i64,  // -1 = camera default
    pub binning: i64, // X binning
    pub readout: i64, // -1 = default
}

impl TemplateKey {
    pub fn of(meta: &FrameMeta) -> Self {
        Self {
            filter: meta.filter.clone().unwrap_or_else(|| "NONE".to_string()),
            gain: meta.gain.unwrap_or(-1),
            offset: meta.offset.unwrap_or(-1),
            binning: meta.binning_x.unwrap_or(1),
            readout: meta.readout_mode.unwrap_or(-1),
        }
    }

    /// Template display name, e.g. `Ha G100 O30 1x1`.
    pub fn name(&self) -> String {
        let mut name = self.filter.clone();
        if self.gain >= 0 {
            name.push_str(&format!(" G{}", self.gain));
        }
        if self.offset >= 0 {
            name.push_str(&format!(" O{}", self.offset));
        }
        name.push_str(&format!(" {0}x{0}", self.binning));
        name
    }
}

/// One exposure plan: frames of one template + exposure length on one target.
#[derive(Debug)]
pub struct PlannedExposure {
    pub template: TemplateKey,
    /// Exposure length in seconds (0 when EXPTIME was missing).
    pub exposure_s: f64,
    /// Indices into the caller's frame slice.
    pub frames: Vec<usize>,
}

#[derive(Debug)]
pub struct PlannedTarget {
    pub name: String,
    /// Median frame RA, converted to TS's storage unit (decimal HOURS).
    pub ra_hours: Option<f64>,
    /// Median frame Dec in degrees.
    pub dec_deg: Option<f64>,
    pub exposures: Vec<PlannedExposure>,
}

impl PlannedTarget {
    pub fn frame_count(&self) -> usize {
        self.exposures.iter().map(|e| e.frames.len()).sum()
    }
}

#[derive(Debug)]
pub struct PlannedProject {
    pub name: String,
    pub signature: EquipmentSignature,
    pub start_ts: Option<i64>,
    pub end_ts: Option<i64>,
    pub targets: Vec<PlannedTarget>,
}

impl PlannedProject {
    pub fn frame_count(&self) -> usize {
        self.targets.iter().map(|t| t.frame_count()).sum()
    }
}

#[derive(Debug, Default)]
pub struct ImportPlan {
    pub projects: Vec<PlannedProject>,
}

impl ImportPlan {
    pub fn frame_count(&self) -> usize {
        self.projects.iter().map(|p| p.frame_count()).sum()
    }

    /// Every distinct template the plan references, in stable order.
    pub fn template_keys(&self) -> Vec<TemplateKey> {
        let mut keys: Vec<TemplateKey> = self
            .projects
            .iter()
            .flat_map(|p| p.targets.iter())
            .flat_map(|t| t.exposures.iter())
            .map(|e| e.template.clone())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }
}

pub const DEFAULT_TIME_GAP_DAYS: f64 = 14.0;
pub const DEFAULT_MOSAIC_RADIUS_DEG: f64 = 5.0;

#[derive(Debug)]
struct TargetGroup {
    name: String,
    frames: Vec<usize>,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
    ra_deg: Option<f64>,
    dec_deg: Option<f64>,
    mosaic_root: Option<(String, String)>,
}

/// Build the import plan. `frames` should already be filtered to readable
/// light frames; indices in the plan refer into this slice.
pub fn build_plan(frames: &[FrameMeta], time_gap_days: f64) -> ImportPlan {
    // 1. Partition by equipment signature (BTreeMap for deterministic order).
    let mut by_signature: BTreeMap<EquipmentSignature, Vec<usize>> = BTreeMap::new();
    for (idx, meta) in frames.iter().enumerate() {
        by_signature
            .entry(EquipmentSignature::of(meta))
            .or_default()
            .push(idx);
    }

    let mosaic_gap_secs = (time_gap_days.max(0.0) * 86_400.0) as i64;
    let mut projects = Vec::new();

    for (signature, indices) in by_signature {
        // Build one target group per exact OBJECT across the full archive.
        // A target remains one scheduler project even when captures span
        // multiple sessions; session splitting belongs to later analysis.
        let mut by_object: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for idx in indices {
            let object = frames[idx]
                .object
                .clone()
                .unwrap_or_else(|| "Unknown Target".to_string());
            by_object.entry(object).or_default().push(idx);
        }

        let target_groups = by_object
            .into_iter()
            .map(|(name, mut target_frames)| {
                target_frames.sort_by_key(|&i| (frames[i].timestamp.unwrap_or(i64::MIN), i));
                TargetGroup {
                    start_ts: target_frames
                        .iter()
                        .filter_map(|&i| frames[i].timestamp)
                        .min(),
                    end_ts: target_frames
                        .iter()
                        .filter_map(|&i| frames[i].timestamp)
                        .max(),
                    ra_deg: median(target_frames.iter().filter_map(|&i| frames[i].ra_deg)),
                    dec_deg: median(target_frames.iter().filter_map(|&i| frames[i].dec_deg)),
                    mosaic_root: mosaic_root(&name),
                    name,
                    frames: target_frames,
                }
            })
            .collect::<Vec<_>>();

        // Start with one project per target, then combine only strong mosaic
        // candidates: a shared panel-style root, nearby centers, and nearby
        // capture ranges. The name clue keeps unrelated neighboring objects
        // from being merged merely because they share a wide field.
        let mut project_groups: Vec<Vec<TargetGroup>> = Vec::new();
        for target in target_groups {
            let matching_project = project_groups.iter().position(|group| {
                group
                    .iter()
                    .any(|other| likely_mosaic_pair(other, &target, mosaic_gap_secs))
            });
            if let Some(index) = matching_project {
                project_groups[index].push(target);
            } else {
                project_groups.push(vec![target]);
            }
        }

        for group in project_groups {
            let start_ts = group.iter().filter_map(|target| target.start_ts).min();
            let end_ts = group.iter().filter_map(|target| target.end_ts).max();
            let name = if group.len() > 1 {
                group[0]
                    .mosaic_root
                    .as_ref()
                    .map(|(_, display)| display.clone())
                    .unwrap_or_else(|| group[0].name.clone())
            } else {
                group[0].name.clone()
            };
            let targets = group
                .into_iter()
                .map(|target| build_target(frames, target.name, target.frames))
                .collect();
            projects.push(PlannedProject {
                name,
                signature: signature.clone(),
                start_ts,
                end_ts,
                targets,
            });
        }
    }

    // Disambiguate identical project names (same rig, same month after a
    // long mid-month gap) with a numeric suffix.
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for project in &mut projects {
        let count = seen.entry(project.name.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            project.name = format!("{} ({})", project.name, count);
        }
    }

    ImportPlan { projects }
}

fn mosaic_root(name: &str) -> Option<(String, String)> {
    let tokens = name
        .split(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '(' | ')' | '[' | ']'))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.len() < 2 {
        return None;
    }

    let panel_index = tokens.iter().rposition(|token| {
        matches!(
            token.to_ascii_lowercase().as_str(),
            "panel" | "pane" | "tile"
        )
    });
    let root_len = if let Some(index) = panel_index {
        let suffix = &tokens[index + 1..];
        if index == 0
            || suffix.is_empty()
            || !suffix
                .iter()
                .all(|part| part.chars().all(|c| c.is_ascii_digit()))
        {
            return None;
        }
        index
    } else {
        let last = tokens.last().unwrap().to_ascii_lowercase();
        if last.len() > 1 && last.starts_with('p') && last[1..].chars().all(|c| c.is_ascii_digit())
        {
            tokens.len() - 1
        } else {
            return None;
        }
    };

    let display = tokens[..root_len].join(" ");
    Some((display.to_ascii_lowercase(), display))
}

fn likely_mosaic_pair(a: &TargetGroup, b: &TargetGroup, max_time_gap_secs: i64) -> bool {
    let (Some((a_root, _)), Some((b_root, _))) = (&a.mosaic_root, &b.mosaic_root) else {
        return false;
    };
    if a_root != b_root || !capture_ranges_near(a, b, max_time_gap_secs) {
        return false;
    }
    let (Some(a_ra), Some(a_dec), Some(b_ra), Some(b_dec)) =
        (a.ra_deg, a.dec_deg, b.ra_deg, b.dec_deg)
    else {
        return false;
    };
    angular_distance_deg(a_ra, a_dec, b_ra, b_dec) <= DEFAULT_MOSAIC_RADIUS_DEG
}

fn capture_ranges_near(a: &TargetGroup, b: &TargetGroup, max_gap: i64) -> bool {
    let (Some(a_start), Some(a_end), Some(b_start), Some(b_end)) =
        (a.start_ts, a.end_ts, b.start_ts, b.end_ts)
    else {
        return false;
    };
    if a_start <= b_end && b_start <= a_end {
        return true;
    }
    let gap = if a_end < b_start {
        b_start - a_end
    } else {
        a_start - b_end
    };
    gap <= max_gap
}

fn angular_distance_deg(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let (ra1, dec1, ra2, dec2) = (
        ra1.to_radians(),
        dec1.to_radians(),
        ra2.to_radians(),
        dec2.to_radians(),
    );
    let cosine =
        (dec1.sin() * dec2.sin() + dec1.cos() * dec2.cos() * (ra1 - ra2).cos()).clamp(-1.0, 1.0);
    cosine.acos().to_degrees()
}

fn build_target(frames: &[FrameMeta], name: String, indices: Vec<usize>) -> PlannedTarget {
    let ra_hours = median(indices.iter().filter_map(|&i| frames[i].ra_deg)).map(|deg| deg / 15.0);
    let dec_deg = median(indices.iter().filter_map(|&i| frames[i].dec_deg));

    // Exposure plans: template + exposure rounded to milliseconds.
    let mut by_plan: BTreeMap<(TemplateKey, i64), Vec<usize>> = BTreeMap::new();
    for idx in indices {
        let key = TemplateKey::of(&frames[idx]);
        let exp_ms = frames[idx]
            .exposure_s
            .map(|e| (e * 1000.0).round() as i64)
            .unwrap_or(0);
        by_plan.entry((key, exp_ms)).or_default().push(idx);
    }

    let exposures = by_plan
        .into_iter()
        .map(|((template, exp_ms), mut plan_frames)| {
            plan_frames.sort_by_key(|&i| (frames[i].timestamp.unwrap_or(i64::MIN), i));
            PlannedExposure {
                template,
                exposure_s: exp_ms as f64 / 1000.0,
                frames: plan_frames,
            }
        })
        .collect();

    PlannedTarget {
        name,
        ra_hours,
        dec_deg,
        exposures,
    }
}

fn median(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut values: Vec<f64> = values.filter(|v| v.is_finite()).collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(values[values.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn frame(
        object: &str,
        filter: &str,
        ts: i64,
        telescope: &str,
        focal: f64,
        exposure: f64,
    ) -> FrameMeta {
        FrameMeta {
            path: PathBuf::from(format!("{object}-{ts}.fits")),
            readable: true,
            object: Some(object.to_string()),
            filter: Some(filter.to_string()),
            timestamp: Some(ts),
            exposure_s: Some(exposure),
            gain: Some(100),
            offset: Some(30),
            binning_x: Some(1),
            binning_y: Some(1),
            ra_deg: Some(83.8),
            dec_deg: Some(-5.4),
            telescope: Some(telescope.to_string()),
            focal_length_mm: Some(focal),
            ..Default::default()
        }
    }

    const DAY: i64 = 86_400;

    #[test]
    fn maps_each_object_to_its_own_project_by_default() {
        let frames = vec![
            frame("M31", "Ha", 1_000, "EdgeHD", 1960.0, 300.0),
            frame("M31", "OIII", 2_000, "EdgeHD", 1960.0, 300.0),
            frame("M33", "Ha", 3_000, "EdgeHD", 1960.0, 300.0),
        ];
        let plan = build_plan(&frames, 14.0);
        assert_eq!(plan.projects.len(), 2);
        assert!(plan
            .projects
            .iter()
            .all(|project| project.targets.len() == 1));
        assert_eq!(plan.frame_count(), 3);
        // Two templates (Ha + OIII), three plans total across two targets.
        assert_eq!(plan.template_keys().len(), 2);
    }

    #[test]
    fn different_equipment_splits_projects() {
        let frames = vec![
            frame("M31", "Ha", 1_000, "EdgeHD", 1960.0, 300.0),
            frame("M31", "Ha", 2_000, "RedCat", 250.0, 300.0),
        ];
        let plan = build_plan(&frames, 14.0);
        assert_eq!(plan.projects.len(), 2);
    }

    #[test]
    fn one_target_remains_one_project_across_session_gaps() {
        let frames = vec![
            frame("M31", "Ha", 0, "EdgeHD", 1960.0, 300.0),
            frame("M31", "Ha", 5 * DAY, "EdgeHD", 1960.0, 300.0),
            // 60 days later: new project.
            frame("M31", "Ha", 65 * DAY, "EdgeHD", 1960.0, 300.0),
        ];
        let plan = build_plan(&frames, 14.0);
        assert_eq!(plan.projects.len(), 1);
        assert_eq!(plan.projects[0].frame_count(), 3);
    }

    #[test]
    fn ra_stored_in_hours() {
        let frames = vec![frame("M42", "L", 1_000, "EdgeHD", 1960.0, 60.0)];
        let plan = build_plan(&frames, 14.0);
        let target = &plan.projects[0].targets[0];
        // 83.8 degrees == 5.5866… hours
        let ra = target.ra_hours.unwrap();
        assert!((ra - 83.8 / 15.0).abs() < 1e-9, "ra_hours = {ra}");
    }

    #[test]
    fn distinct_exposures_become_distinct_plans() {
        let frames = vec![
            frame("M31", "Ha", 1_000, "EdgeHD", 1960.0, 300.0),
            frame("M31", "Ha", 2_000, "EdgeHD", 1960.0, 60.0),
        ];
        let plan = build_plan(&frames, 14.0);
        let target = &plan.projects[0].targets[0];
        assert_eq!(target.exposures.len(), 2);
        // One shared template — exposure length is plan-level, not template.
        assert_eq!(plan.template_keys().len(), 1);
    }

    #[test]
    fn timestampless_frames_join_earliest_session_without_splitting() {
        let mut untimed = frame("M31", "Ha", 0, "EdgeHD", 1960.0, 300.0);
        untimed.timestamp = None;
        let frames = vec![
            untimed,
            frame("M31", "Ha", 100 * DAY, "EdgeHD", 1960.0, 300.0),
        ];
        let plan = build_plan(&frames, 14.0);
        // The None-timestamp frame must not manufacture a 100-day gap.
        assert_eq!(plan.projects.len(), 1);
    }

    #[test]
    fn nearby_panel_targets_share_a_mosaic_project() {
        let mut panel_1 = frame("NGC 7000 - Panel 1", "Ha", DAY, "RedCat", 250.0, 300.0);
        panel_1.ra_deg = Some(312.0);
        panel_1.dec_deg = Some(44.0);
        let mut panel_2 = frame(
            "NGC 7000 - Panel 2",
            "OIII",
            2 * DAY,
            "RedCat",
            250.0,
            300.0,
        );
        panel_2.ra_deg = Some(314.0);
        panel_2.dec_deg = Some(44.5);

        let plan = build_plan(&[panel_1, panel_2], 14.0);
        assert_eq!(plan.projects.len(), 1);
        assert_eq!(plan.projects[0].name, "NGC 7000");
        assert_eq!(plan.projects[0].targets.len(), 2);
    }

    #[test]
    fn panel_names_do_not_merge_when_coordinates_or_dates_are_distant() {
        let mut near = frame("Veil Panel 1", "Ha", DAY, "RedCat", 250.0, 300.0);
        near.ra_deg = Some(312.0);
        near.dec_deg = Some(30.0);
        let mut distant_sky = frame("Veil Panel 2", "Ha", 2 * DAY, "RedCat", 250.0, 300.0);
        distant_sky.ra_deg = Some(40.0);
        distant_sky.dec_deg = Some(-20.0);
        let mut distant_time = frame("Veil Panel 3", "Ha", 40 * DAY, "RedCat", 250.0, 300.0);
        distant_time.ra_deg = Some(313.0);
        distant_time.dec_deg = Some(30.5);

        let plan = build_plan(&[near, distant_sky, distant_time], 14.0);
        assert_eq!(plan.projects.len(), 3);
    }
}
