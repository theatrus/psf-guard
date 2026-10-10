//! Take a Target Scheduler project's targets and exposure plans in as the
//! Director's own framing and plan drafts, once, when the project has none.
//! What N.I.N.A. already knows about a project is its plan; the operator
//! should open the workspace to find it there, not retype it.

use super::plan::Templates;
use super::*;
use psf_guard_director_core::{
    framing::{Mosaic, PanelSize, TangentPlane},
    visibility::IcrsPosition,
};
use psf_guard_director_meta::{
    draft_import::DraftImport,
    framing::FramingDraft,
    plan::{Contribution, Goal, Objective, PlanDraft, TemplateChoice},
};
use rusqlite::Connection;
use std::collections::BTreeMap;

const DEFAULT_SURVEY: &str = "dss2_color";
const MAX_GRID: u32 = 16;

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Imported {
    pub framing: bool,
    pub plan: bool,
    /// How many separate targets the project held when they were not one
    /// framing; the drafts then frame the first alone. Zero otherwise.
    pub separate_targets: usize,
}

struct SourceTarget {
    id: i64,
    name: String,
    center: IcrsPosition,
    rotation_degrees: f64,
}

pub(super) fn has_column(connection: &Connection, table: &str, column: &str) -> bool {
    connection
        .prepare(&format!("SELECT {column} FROM {table} LIMIT 0"))
        .is_ok()
}

fn source_priority(connection: &Connection, project_row: i64) -> Result<u32, StoreError> {
    // TS Low/Normal/High are 0/1/2, with higher values preferred. Do not
    // introduce a filter priority from alphabetical bandpass ordering.
    if !has_column(connection, "project", "priority") {
        return Ok(1);
    }
    let priority = connection.query_row(
        "SELECT priority FROM project WHERE Id=?1",
        [project_row],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    Ok(priority.filter(|p| (0..=2).contains(p)).unwrap_or(1) as u32)
}

fn source_targets(
    connection: &Connection,
    project_row: i64,
) -> Result<Vec<SourceTarget>, StoreError> {
    let rotation = if has_column(connection, "target", "rotation") {
        "rotation"
    } else {
        "NULL"
    };
    let mut statement = connection.prepare(&format!(
        "SELECT Id, name, ra, dec, {rotation} FROM target WHERE projectid = ?1 ORDER BY Id LIMIT 256"
    ))?;
    let rows = statement.query_map([project_row], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<f64>>(2)?,
            row.get::<_, Option<f64>>(3)?,
            row.get::<_, Option<f64>>(4)?,
        ))
    })?;
    let mut targets = Vec::new();
    for row in rows {
        let (id, name, ra_hours, dec, rotation) = row?;
        let (Some(ra_hours), Some(dec)) = (ra_hours, dec) else {
            continue;
        };
        let Some((ra_degrees, dec_degrees)) =
            crate::astrometry::target_scheduler_coordinates(ra_hours, dec)
        else {
            continue;
        };
        targets.push(SourceTarget {
            id,
            name: name.unwrap_or_default().trim().to_owned(),
            center: IcrsPosition {
                ra_degrees,
                dec_degrees,
            },
            rotation_degrees: rotation
                .filter(|r| r.is_finite())
                .unwrap_or(0.0)
                .rem_euclid(360.0),
        });
    }
    Ok(targets)
}

/// One band of a plan taken in from Target Scheduler, so far: its main
/// plan's template and exposure, and the most frames any of its plans asks
/// for.
struct Band {
    template: usize,
    exposure: f64,
    desired: u32,
    /// What makes a plan the main one: a second or more, then frames asked
    /// for, then length.
    rank: (bool, u32, u64),
}

/// Distinct positions along one camera axis, within `tolerance`, in order.
fn clusters(values: &[f64], tolerance: f64) -> Vec<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mut out: Vec<f64> = Vec::new();
    for value in sorted {
        match out.last() {
            Some(last) if (value - last).abs() <= tolerance => {}
            _ => out.push(value),
        }
    }
    out
}

/// A grid the targets form along the camera axes, with each target's cell.
struct Grid {
    center: IcrsPosition,
    mosaic: Mosaic,
    /// Per target, in input order: zero-based row from the top and column
    /// from the left, as Director numbers panels.
    cells: Vec<(u32, u32)>,
}

/// The index of the cluster `value` falls in. `starts` holds each cluster's
/// smallest member, ascending, as `clusters` returns them.
fn cluster_index(starts: &[f64], value: f64) -> usize {
    starts
        .iter()
        .filter(|start| **start <= value)
        .count()
        .saturating_sub(1)
}

/// The mosaic grid the targets form, seen along the camera axes, when every
/// target sits on one; `None` for a lone target or scattered ones.
fn grid_layout(targets: &[SourceTarget], panel: Option<PanelSize>) -> Option<Grid> {
    if targets.len() < 2 {
        return None;
    }
    let first = &targets[0];
    let plane = TangentPlane::at(first.center).ok()?;
    let mut offsets = Vec::with_capacity(targets.len());
    for target in targets {
        offsets.push(plane.project(target.center)?);
    }
    let mean = [
        offsets.iter().map(|o| o[0]).sum::<f64>() / offsets.len() as f64,
        offsets.iter().map(|o| o[1]).sum::<f64>() / offsets.len() as f64,
    ];
    let center = plane.deproject(mean);
    // Measure the grid in the plane of the mosaic's centre, where N.I.N.A.
    // lays panels out. Seen from the first panel's plane instead, a grid far
    // from the equator bends: at +61° a 2×2 of 1.4° panels is off by 0.03°,
    // more than the tolerance, and would read as scattered targets.
    let plane = TangentPlane::at(center).ok()?;
    let mut offsets = Vec::with_capacity(targets.len());
    for target in targets {
        offsets.push(plane.project(target.center)?);
    }
    // Camera frame: up along the position angle, right a quarter turn on.
    let (sin, cos) = first.rotation_degrees.to_radians().sin_cos();
    let along_x: Vec<f64> = offsets.iter().map(|o| -o[0] * cos + o[1] * sin).collect();
    let along_y: Vec<f64> = offsets.iter().map(|o| o[0] * sin + o[1] * cos).collect();
    let tolerance = panel
        .map(|p| (p.width_degrees.min(p.height_degrees) * 0.1).max(0.02))
        .unwrap_or(0.02);
    let columns = clusters(&along_x, tolerance);
    let rows = clusters(&along_y, tolerance);
    let (rows_n, columns_n) = (rows.len() as u32, columns.len() as u32);
    if rows_n * columns_n != targets.len() as u32 || rows_n > MAX_GRID || columns_n > MAX_GRID {
        return None;
    }
    let cells: Vec<(u32, u32)> = along_y
        .iter()
        .zip(&along_x)
        .map(|(y, x)| {
            // Row 1 is the top: the largest distance up.
            let row = rows.len() - 1 - cluster_index(&rows, *y);
            (row as u32, cluster_index(&columns, *x) as u32)
        })
        .collect();
    // Clusters can count right yet leave a cell empty and another doubled.
    let mut seen = std::collections::HashSet::new();
    if !cells.iter().all(|cell| seen.insert(*cell)) {
        return None;
    }
    let overlap = match panel {
        Some(panel) => {
            let mut shares = Vec::new();
            if columns_n > 1 {
                let step = (columns.last().unwrap() - columns[0]) / f64::from(columns_n - 1);
                shares.push(1.0 - step / panel.width_degrees);
            }
            if rows_n > 1 {
                let step = (rows.last().unwrap() - rows[0]) / f64::from(rows_n - 1);
                shares.push(1.0 - step / panel.height_degrees);
            }
            shares
                .into_iter()
                .fold(None, |acc: Option<f64>, s| {
                    Some(acc.map_or(s, |a| a.min(s)))
                })
                .map(|s| (s * 100.0).round().clamp(0.0, 90.0) as u32)
                .unwrap_or(20)
        }
        None => 0,
    };
    Some(Grid {
        center,
        mosaic: Mosaic {
            rows: rows_n,
            columns: columns_n,
            overlap_percent: overlap,
        },
        cells,
    })
}

/// The mosaic grid the targets form, seen along the camera axes: rows by
/// columns when every target sits on one, else a single panel.
fn infer_layout(targets: &[SourceTarget], panel: Option<PanelSize>) -> (IcrsPosition, Mosaic) {
    match grid_layout(targets, panel) {
        Some(grid) => (grid.center, grid.mosaic),
        None => (
            targets[0].center,
            Mosaic {
                rows: 1,
                columns: 1,
                overlap_percent: 20,
            },
        ),
    }
}

/// One panel of a mosaic a project's targets form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InferredPanel {
    pub target_id: i64,
    /// One-based, row 1 at the top and column 1 at the left.
    pub row: u32,
    pub column: u32,
}

/// The panels of the mosaic grid a Target Scheduler project's targets form,
/// read from their coordinates and rotation the way a framing draft is
/// imported. `None` when the targets are one or form no grid.
pub(crate) fn inferred_mosaic(
    connection: &Connection,
    project_row: i64,
) -> Result<Option<Vec<InferredPanel>>, StoreError> {
    let targets = source_targets(connection, project_row)?;
    let Some(grid) = grid_layout(&targets, None) else {
        return Ok(None);
    };
    Ok(Some(
        targets
            .iter()
            .zip(grid.cells)
            .map(|(target, (row, column))| InferredPanel {
                target_id: target.id,
                row: row + 1,
                column: column + 1,
            })
            .collect(),
    ))
}

/// Whether the targets are one framing: a single target, or panels of one
/// mosaic grid. Separate targets are not, and Director plans one framing per
/// project.
fn forms_one_framing(targets: &[SourceTarget], panel: Option<PanelSize>) -> bool {
    if targets.len() < 2 {
        return true;
    }
    let (_, mosaic) = infer_layout(targets, panel);
    mosaic.rows * mosaic.columns == targets.len() as u32
}

/// A name for the whole: what the targets' names share, else the first.
fn shared_name(targets: &[SourceTarget], project_name: &str) -> String {
    let names: Vec<&str> = targets
        .iter()
        .map(|t| t.name.as_str())
        .filter(|n| !n.is_empty())
        .collect();
    let candidate = match names.first() {
        None => project_name.to_owned(),
        Some(first) if names.len() == 1 => (*first).to_owned(),
        Some(first) => {
            let mut prefix = first.len();
            for name in &names[1..] {
                prefix = first
                    .char_indices()
                    .zip(name.chars())
                    .take_while(|((_, a), b)| a == b)
                    .last()
                    .map(|((i, c), _)| i + c.len_utf8())
                    .unwrap_or(0)
                    .min(prefix);
            }
            // Panels share whole words, not a cut one: "Heart r1c1" and
            // "Heart r1c2" name "Heart", not "Heart r1c".
            let shared = if prefix < first.len() {
                first[..prefix]
                    .rfind([' ', '-', '_', ':', '#'])
                    .map(|at| &first[..at])
                    .unwrap_or("")
            } else {
                first
            };
            let shared = shared.trim_end_matches([' ', '-', '_', ':', '#']).trim();
            if shared.len() >= 3 {
                shared.to_owned()
            } else {
                project_name.to_owned()
            }
        }
    };
    candidate
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A Target Scheduler project's targets and exposure plans read as drafts,
/// before the store sees them. Reading needs no store gate.
#[derive(Default)]
pub(super) struct Drafts {
    pub framing: Option<FramingDraft>,
    pub plan: Option<PlanDraft>,
    /// As in [`Imported`].
    pub separate_targets: usize,
    /// Exposure plans left out, and why.
    pub warnings: Vec<String>,
}

/// Which drafts a project has none of yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Missing {
    pub framing: bool,
    pub plan: bool,
}

/// Read the drafts a project would take in: a framing draft from the
/// targets and a plan draft from the exposure plans, each only when missing.
/// `panel` is the rig's field, when its optics are known; `templates` are
/// the database's, read once for all its projects. Without them no plan
/// draft is read.
#[allow(clippy::too_many_arguments)]
pub(super) fn read_drafts(
    connection: &Connection,
    templates: Option<&Templates>,
    rig: Uuid,
    project_id: Uuid,
    project_row: i64,
    project_name: &str,
    panel: Option<PanelSize>,
    missing: Missing,
) -> Result<Drafts, StoreError> {
    let mut drafts = Drafts::default();
    let missing = Missing {
        plan: missing.plan && templates.is_some(),
        ..missing
    };
    if !missing.framing && !missing.plan {
        return Ok(drafts);
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut targets = source_targets(connection, project_row)?;
    if targets.is_empty() {
        return Ok(drafts);
    }
    // Separate targets that are not one mosaic: Director plans one framing
    // per project, so the draft is the first target alone. Merging the
    // others' exposure plans into it would raise its desired counts.
    let one_framing = forms_one_framing(&targets, panel);
    if !one_framing {
        drafts.separate_targets = targets.len();
        targets.truncate(1);
    }
    if missing.framing {
        let (center, mosaic) = infer_layout(&targets, panel);
        let extent = panel.map(|p| {
            let keep = 1.0 - f64::from(mosaic.overlap_percent) / 100.0;
            (
                p.width_degrees + p.width_degrees * keep * f64::from(mosaic.columns - 1),
                p.height_degrees + p.height_degrees * keep * f64::from(mosaic.rows - 1),
            )
        });
        let view_fov = extent
            .map(|(w, h)| (w.max(h * 4.0 / 3.0) * 1.6).clamp(0.5, 40.0))
            .unwrap_or(4.0);
        drafts.framing = Some(FramingDraft {
            project_id,
            revision: 0,
            target_name: shared_name(&targets, project_name),
            center,
            position_angle_degrees: targets[0].rotation_degrees,
            mosaic,
            panel_rig_id: panel.is_some().then_some(rig),
            panel,
            shown_rig_ids: vec![],
            survey_id: DEFAULT_SURVEY.to_owned(),
            view_fov_degrees: view_fov,
            updated_at_ms: now_ms,
            rig_framings: vec![],
            layout_revision: 0,
        });
    }
    if let Some(templates) = templates.filter(|_| missing.plan) {
        let priority = source_priority(connection, project_row)?;
        let enabled = if has_column(connection, "exposureplan", "enabled") {
            "COALESCE(e.enabled, 1) = 1"
        } else {
            "1 = 1"
        };
        // With separate targets, only the first target's plans: the draft
        // frames that target alone.
        let only_target = if one_framing {
            None
        } else {
            Some(targets[0].id)
        };
        let mut statement = connection.prepare(&format!(
            "SELECT e.exposureTemplateId, e.exposure, e.desired FROM exposureplan e
             JOIN target t ON t.Id = e.targetid
             WHERE t.projectid = ?1 AND (?2 IS NULL OR t.Id = ?2) AND {enabled} ORDER BY e.Id LIMIT 1024"
        ))?;
        let rows = statement
            .query_map(rusqlite::params![project_row, only_target], |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<f64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        // One objective per bandpass; the frames a panel wants is the most
        // any of its plans asks for, since Director counts per panel. Its
        // template and exposure come from the band's main plan: one of at
        // least a second, then the one asking for most frames. A short plan
        // made first (flats or test frames taken as lights) never sets them.
        let main = |exposure: f64, desired: u32| (exposure >= 1.0, desired, exposure.to_bits());
        let mut by_bandpass: BTreeMap<String, Band> = BTreeMap::new();
        let mut left_out = std::collections::BTreeSet::new();
        for (template_id, exposure, desired) in rows {
            let Some(index) =
                template_id.and_then(|id| templates.usable.iter().position(|t| t.id == id))
            else {
                if let Some(unusable) =
                    template_id.and_then(|id| templates.unusable.iter().find(|t| t.id == id))
                    && left_out.insert(unusable.id)
                {
                    drafts.warnings.push(format!(
                        "{project_name}: its exposure plans with template {} were left out of its plan; that template's Moon avoidance settings are outside what Director plans with.",
                        unusable.name
                    ));
                }
                continue;
            };
            let template = &templates.usable[index];
            let exposure = exposure
                .filter(|e| e.is_finite() && *e > 0.0)
                .unwrap_or(template.default_exposure)
                .max(0.001);
            let desired = desired.unwrap_or(0).clamp(0, 100_000) as u32;
            let rank = main(exposure, desired);
            let band = by_bandpass
                .entry(template.bandpass.id.clone())
                .or_insert(Band {
                    template: index,
                    exposure,
                    desired: 0,
                    rank,
                });
            if rank > band.rank {
                band.template = index;
                band.exposure = exposure;
                band.rank = rank;
            }
            band.desired = band.desired.max(desired);
        }
        let mut objectives = Vec::new();
        let mut contributions = Vec::new();
        for (bandpass_id, band) in by_bandpass {
            let (index, exposure, desired) = (band.template, band.exposure, band.desired);
            if desired == 0 {
                continue;
            }
            let template = &templates.usable[index];
            let objective = Objective {
                id: Uuid::new_v4(),
                bandpass_id,
                purpose: "faint_detail".to_owned(),
                goal: Goal::Frames { value: desired },
                priority,
            };
            contributions.push(Contribution {
                id: Uuid::new_v4(),
                objective_id: objective.id,
                rig_id: rig,
                template: TemplateChoice {
                    template_guid: template.guid,
                    template_id: Some(template.id),
                    name: template.name.clone(),
                    filter_name: template.filter_name.clone(),
                    gain: template.gain,
                    offset: template.offset,
                    bin: template.bin,
                    readout_mode: template.readout_mode,
                    moon: Some(template.moon.clone()),
                },
                exposure_seconds: exposure,
                panel_ids: vec![],
                enabled: true,
                goal: None,
            });
            objectives.push(objective);
        }
        if !objectives.is_empty() {
            drafts.plan = Some(PlanDraft {
                project_id,
                revision: 0,
                objectives,
                contributions,
                updated_at_ms: now_ms,
            });
        }
    }
    Ok(drafts)
}

/// Import once: save each draft under `project_id` only while the project
/// has none, so a draft the operator saved meanwhile is never replaced.
/// `source` names the project read; drafts the import made alone are
/// recorded against it, so the plan follows it until someone edits it.
pub(super) fn save_drafts(
    store: &mut MetaStore,
    project_id: Uuid,
    drafts: Drafts,
    source: Option<(Uuid, Uuid)>,
) -> Result<Imported, StoreError> {
    let mut imported = Imported::default();
    let mut revisions = (0, 0);
    if let Some(mut framing) = drafts.framing {
        framing.project_id = project_id;
        match store.save_framing_draft(&framing, 0) {
            Ok(saved) => {
                imported.framing = true;
                revisions.0 = saved.layout_revision;
            }
            Err(StoreError::Conflict) => {}
            Err(error) => return Err(error),
        }
    }
    if let Some(mut plan) = drafts.plan {
        plan.project_id = project_id;
        match store.save_plan_draft(&plan, 0) {
            Ok(saved) => {
                imported.plan = true;
                revisions.1 = saved.revision;
            }
            Err(StoreError::Conflict) => {}
            Err(error) => return Err(error),
        }
    }
    if let Some((catalog_id, source_project_guid)) = source
        && (imported.framing || imported.plan)
    {
        // A draft saved here before the import is the operator's: then the
        // plan does not follow.
        let theirs = (!imported.framing && store.framing_draft(project_id)?.is_some())
            || (!imported.plan && store.plan_draft(project_id)?.is_some());
        if !theirs {
            store.record_draft_import(&DraftImport {
                project_id,
                catalog_id,
                source_project_guid,
                framing_revision: revisions.0,
                plan_revision: revisions.1,
            })?;
        }
    }
    if imported.framing || imported.plan {
        imported.separate_targets = drafts.separate_targets;
    }
    Ok(imported)
}

/// A plan that follows its Target Scheduler project: the import's record
/// and the drafts it saved, untouched since. A framing's revision counts by
/// its layout: choosing another survey or view width is no edit.
pub(super) struct Follow {
    record: DraftImport,
    framing: Option<FramingDraft>,
    plan: Option<PlanDraft>,
}

/// Whether a linked plan follows this source project: imported from it,
/// never activated, and its drafts still at the revisions the import saved.
/// Once someone saves a draft here or activates the plan, it is theirs.
pub(super) fn following(
    store: &MetaStore,
    project: Uuid,
    catalog: Uuid,
    source: Uuid,
) -> Option<Follow> {
    let record = store.draft_import(project).ok()??;
    if record.catalog_id != catalog || record.source_project_guid != source {
        return None;
    }
    if !matches!(store.activation(project), Ok(None)) {
        return None;
    }
    let framing = store.framing_draft(project).ok()?;
    let plan = store.plan_draft(project).ok()?;
    (framing.as_ref().map_or(0, |f| f.layout_revision) == record.framing_revision
        && plan.as_ref().map_or(0, |p| p.revision) == record.plan_revision)
        .then_some(Follow {
            record,
            framing,
            plan,
        })
}

/// Give a plan read again the ids its last import had, band by band, so an
/// unchanged plan reads the same and a changed one keeps its objectives.
fn keep_ids(stored: Option<&PlanDraft>, plan: &mut PlanDraft) {
    let Some(stored) = stored else {
        return;
    };
    for objective in &mut plan.objectives {
        let Some(old) = stored
            .objectives
            .iter()
            .find(|o| o.bandpass_id == objective.bandpass_id)
        else {
            continue;
        };
        let read = objective.id;
        objective.id = old.id;
        for contribution in plan
            .contributions
            .iter_mut()
            .filter(|c| c.objective_id == read)
        {
            contribution.objective_id = old.id;
            if let Some(was) = stored
                .contributions
                .iter()
                .find(|c| c.objective_id == old.id && c.rig_id == contribution.rig_id)
            {
                contribution.id = was.id;
            }
        }
    }
}

/// Take a followed plan's project in again. Each draft saves under the
/// revision the import left; one that reads the same is not saved again. A
/// save made here meanwhile wins, and the plan stops following. Answers
/// whether a draft changed.
pub(super) fn refresh_drafts(
    store: &mut MetaStore,
    follow: Follow,
    drafts: Drafts,
) -> Result<bool, StoreError> {
    let Follow {
        mut record,
        framing: stored_framing,
        plan: stored_plan,
    } = follow;
    let project_id = record.project_id;
    let mut changed = false;
    if let Some(mut framing) = drafts.framing {
        framing.project_id = project_id;
        // How the person looks at it stays theirs.
        if let Some(stored) = &stored_framing {
            framing.survey_id = stored.survey_id.clone();
            framing.view_fov_degrees = stored.view_fov_degrees;
            framing.shown_rig_ids = stored.shown_rig_ids.clone();
        }
        let expected = stored_framing.as_ref().map_or(0, |stored| stored.revision);
        match store.save_framing_draft(&framing, expected) {
            Ok(saved) => {
                changed |= saved.revision != expected;
                record.framing_revision = saved.layout_revision;
            }
            Err(StoreError::Conflict) => {
                store.forget_draft_import(project_id)?;
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
    }
    if let Some(mut plan) = drafts.plan {
        plan.project_id = project_id;
        keep_ids(stored_plan.as_ref(), &mut plan);
        match store.save_plan_draft(&plan, record.plan_revision) {
            Ok(saved) => {
                changed |= saved.revision != record.plan_revision;
                record.plan_revision = saved.revision;
            }
            Err(StoreError::Conflict) => {
                store.forget_draft_import(project_id)?;
                return Ok(changed);
            }
            Err(error) => return Err(error),
        }
    }
    if changed {
        store.record_draft_import(&record)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_ts_priority_preserves_low_normal_high_without_inventing_values() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE project(Id INTEGER PRIMARY KEY); INSERT INTO project VALUES(1)",
        )
        .unwrap();
        assert_eq!(source_priority(&db, 1).unwrap(), 1);
        db.execute_batch("ALTER TABLE project ADD COLUMN priority INTEGER")
            .unwrap();
        assert_eq!(source_priority(&db, 1).unwrap(), 1);
        for (priority, expected) in [(0, 0), (1, 1), (2, 2), (-1, 1), (999, 1)] {
            db.execute("UPDATE project SET priority=?1", [priority])
                .unwrap();
            assert_eq!(source_priority(&db, 1).unwrap(), expected);
        }
    }

    fn target(name: &str, ra: f64, dec: f64, rotation: f64) -> SourceTarget {
        SourceTarget {
            id: 0,
            name: name.into(),
            center: IcrsPosition {
                ra_degrees: ra,
                dec_degrees: dec,
            },
            rotation_degrees: rotation,
        }
    }

    #[test]
    fn a_mosaic_far_from_the_equator_still_reads_as_its_grid() {
        // Askar107PHQ's Heart Mosaic, as N.I.N.A. laid it out: a 2×2 at +61°
        // (RA in Target Scheduler hours, times 15). From the first panel's
        // plane the columns bend by 0.03°; from the centre's they line up.
        let targets = [
            target(
                "Heart and Soul Nebula Panel 1",
                2.645_499_681_489_58 * 15.0,
                61.832_231_223_272_4,
                0.0,
            ),
            target(
                "Heart and Soul Nebula Panel 2",
                2.442_450_009_872_69 * 15.0,
                61.832_231_223_272_4,
                0.0,
            ),
            target(
                "Heart and Soul Nebula Panel 3",
                2.642_430_289_441_8 * 15.0,
                60.871_626_084_609_4,
                0.0,
            ),
            target(
                "Heart and Soul Nebula Panel 4",
                2.445_519_401_920_46 * 15.0,
                60.871_626_084_609_4,
                0.0,
            ),
        ];
        let (center, mosaic) = infer_layout(&targets, None);
        assert_eq!((mosaic.rows, mosaic.columns), (2, 2));
        assert!((center.dec_degrees - 61.35).abs() < 0.05, "{center:?}");
    }

    #[test]
    fn a_row_of_targets_reads_as_a_one_by_n_mosaic_with_its_overlap() {
        // Two panels 2° wide stepping 1.6° along RA at the equator: 20% overlap.
        let targets = [
            target("Heart r1c1", 38.0, 0.0, 0.0),
            target("Heart r1c2", 36.4, 0.0, 0.0),
        ];
        let panel = PanelSize {
            width_degrees: 2.0,
            height_degrees: 1.5,
        };
        let (center, mosaic) = infer_layout(&targets, Some(panel));
        assert_eq!(
            (mosaic.rows, mosaic.columns, mosaic.overlap_percent),
            (1, 2, 20)
        );
        assert!((center.ra_degrees - 37.2).abs() < 1e-3, "{center:?}");
        assert_eq!(shared_name(&targets, "Heart Nebula"), "Heart");
        // Without a panel size the grid still counts, the overlap does not.
        let (_, loose) = infer_layout(&targets, None);
        assert_eq!(
            (loose.rows, loose.columns, loose.overlap_percent),
            (1, 2, 0)
        );
        // Targets that form no grid fall back to one panel at the first.
        let scattered = [
            target("A", 10.0, 0.0, 0.0),
            target("B", 12.0, 3.0, 0.0),
            target("C", 20.0, -5.0, 0.0),
        ];
        let (first, single) = infer_layout(&scattered, Some(panel));
        assert_eq!((single.rows, single.columns), (1, 1));
        assert_eq!(first.ra_degrees, 10.0);
        assert_eq!(shared_name(&scattered, "Scattered"), "Scattered");
        assert_eq!(shared_name(&scattered[..1], "X"), "A");
    }
}
