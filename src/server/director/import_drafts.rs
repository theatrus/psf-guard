//! Take a Target Scheduler project's targets and exposure plans in as the
//! Director's own framing and plan drafts, once, when the project has none.
//! What N.I.N.A. already knows about a project is its plan; the operator
//! should open the workspace to find it there, not retype it.

use super::plan::read_templates;
use super::*;
use psf_guard_director_core::{
    framing::{Mosaic, PanelSize, TangentPlane},
    visibility::IcrsPosition,
};
use psf_guard_director_meta::{
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
}

struct SourceTarget {
    name: String,
    center: IcrsPosition,
    rotation_degrees: f64,
}

fn has_column(connection: &Connection, table: &str, column: &str) -> bool {
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
        let (_id, name, ra_hours, dec, rotation) = row?;
        let (Some(ra_hours), Some(dec)) = (ra_hours, dec) else {
            continue;
        };
        let Some((ra_degrees, dec_degrees)) =
            crate::astrometry::target_scheduler_coordinates(ra_hours, dec)
        else {
            continue;
        };
        targets.push(SourceTarget {
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

/// The mosaic grid the targets form, seen along the camera axes: rows by
/// columns when every target sits on one, else a single panel.
fn infer_layout(targets: &[SourceTarget], panel: Option<PanelSize>) -> (IcrsPosition, Mosaic) {
    let first = &targets[0];
    let single = (
        first.center,
        Mosaic {
            rows: 1,
            columns: 1,
            overlap_percent: 20,
        },
    );
    if targets.len() < 2 {
        return single;
    }
    let Ok(plane) = TangentPlane::at(first.center) else {
        return single;
    };
    let mut offsets = Vec::with_capacity(targets.len());
    for target in targets {
        let Some(offset) = plane.project(target.center) else {
            return single;
        };
        offsets.push(offset);
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
    let Ok(plane) = TangentPlane::at(center) else {
        return single;
    };
    let mut offsets = Vec::with_capacity(targets.len());
    for target in targets {
        let Some(offset) = plane.project(target.center) else {
            return single;
        };
        offsets.push(offset);
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
        return single;
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
    (
        center,
        Mosaic {
            rows: rows_n,
            columns: columns_n,
            overlap_percent: overlap,
        },
    )
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

/// Import once: a framing draft from the targets and a plan draft from the
/// exposure plans, each only when the project has none yet.
pub(super) fn import_from_catalog(
    store: &mut MetaStore,
    connection: &Connection,
    rig: Uuid,
    project_id: Uuid,
    project_row: i64,
    project_name: &str,
    now_ms: u64,
) -> Result<Imported, StoreError> {
    let mut imported = Imported::default();
    let need_framing = store.framing_draft(project_id)?.is_none();
    let need_plan = store.plan_draft(project_id)?.is_none();
    if !need_framing && !need_plan {
        return Ok(imported);
    }
    let targets = source_targets(connection, project_row)?;
    if targets.is_empty() {
        return Ok(imported);
    }
    let panel = store
        .rig_profile(rig)?
        .and_then(|profile| profile.optics)
        .and_then(|optics| optics.value.field_of_view().ok())
        .map(|fov| PanelSize {
            width_degrees: fov.width_degrees,
            height_degrees: fov.height_degrees,
        });
    if need_framing {
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
        let draft = FramingDraft {
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
        };
        store.save_framing_draft(&draft, 0)?;
        imported.framing = true;
    }
    if need_plan {
        let priority = source_priority(connection, project_row)?;
        let templates = read_templates(connection).map_err(|_| StoreError::InvalidInput)?;
        let enabled = if has_column(connection, "exposureplan", "enabled") {
            "COALESCE(e.enabled, 1) = 1"
        } else {
            "1 = 1"
        };
        let mut statement = connection.prepare(&format!(
            "SELECT e.exposureTemplateId, e.exposure, e.desired FROM exposureplan e
             JOIN target t ON t.Id = e.targetid
             WHERE t.projectid = ?1 AND {enabled} ORDER BY e.Id LIMIT 1024"
        ))?;
        let rows = statement
            .query_map([project_row], |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<f64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        // One objective per bandpass; the frames a panel wants is the most
        // any of its plans asks for, since Director counts per panel.
        let mut by_bandpass: BTreeMap<String, (usize, f64, u32)> = BTreeMap::new();
        for (template_id, exposure, desired) in rows {
            let Some(index) = template_id.and_then(|id| templates.iter().position(|t| t.id == id))
            else {
                continue;
            };
            let template = &templates[index];
            let exposure = exposure
                .filter(|e| e.is_finite() && *e > 0.0)
                .unwrap_or(template.default_exposure)
                .max(0.001);
            let desired = desired.unwrap_or(0).clamp(0, 100_000) as u32;
            let entry = by_bandpass
                .entry(template.bandpass.id.clone())
                .or_insert((index, exposure, 0));
            entry.2 = entry.2.max(desired);
        }
        let mut objectives = Vec::new();
        let mut contributions = Vec::new();
        for (bandpass_id, (index, exposure, desired)) in by_bandpass {
            if desired == 0 {
                continue;
            }
            let template = &templates[index];
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
            });
            objectives.push(objective);
        }
        if !objectives.is_empty() {
            store.save_plan_draft(
                &PlanDraft {
                    project_id,
                    revision: 0,
                    objectives,
                    contributions,
                    updated_at_ms: now_ms,
                },
                0,
            )?;
            imported.plan = true;
        }
    }
    Ok(imported)
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
