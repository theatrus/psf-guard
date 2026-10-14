//! A project's mosaic panels as one scope, so the Library can show a
//! Target Scheduler mosaic's targets together. Grouping is display only:
//! grading, calibration and stacking stay per panel.
//!
//! The panels come from Director's records when it activated the project,
//! else from the grid the targets' coordinates form. Target names such as
//! "r1c2" are never read for this. Director's panels need not form a grid:
//! panels placed one by one, or a panel moved off its old target, are
//! listed in the rows and columns their targets fall on.

use super::api::ApiResponse;
use super::extract::DbContext;
use super::handlers::AppError;
use super::sky_coverage::{previews_for, SkyPreview};
use axum::{extract::Path, Json};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;

/// Where a mosaic's panel layout came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MosaicSource {
    /// Director's activation record names each target's panel.
    Director,
    /// The targets' coordinates and rotation form the grid.
    Inferred,
}

#[derive(Debug, Clone, Serialize)]
pub struct MosaicPanel {
    pub target_id: i32,
    pub target_name: String,
    /// Director's panel id (`r{row}c{column}` for a grid's panel, `p{n}`
    /// for one placed since), or `r{row}c{column}` for an inferred grid.
    pub panel_id: String,
    /// One-based, row 1 at the top and column 1 at the left. For panels
    /// that form no full grid, the sparse grid their targets fall on.
    pub row: u32,
    pub column: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<SkyPreview>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectMosaic {
    pub project_id: i32,
    pub name: String,
    pub source: MosaicSource,
    pub rows: u32,
    pub columns: u32,
    /// Row by row from the top, left to right.
    pub panels: Vec<MosaicPanel>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectMosaicResponse {
    /// `None` when the project is not a mosaic.
    pub mosaic: Option<ProjectMosaic>,
}

/// GET /api/db/{db}/projects/{project_id}/mosaic
pub async fn get_project_mosaic(
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<ProjectMosaicResponse>>, AppError> {
    let mosaic = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        project_mosaic(&conn, project_id)?
    };
    let Some(mut mosaic) = mosaic else {
        return Ok(Json(ApiResponse::success(ProjectMosaicResponse {
            mosaic: None,
        })));
    };
    let context = ctx.0.clone();
    let previews = tokio::task::spawn_blocking(move || previews_for(&context))
        .await
        .map_err(|error| AppError::InternalError(format!("mosaic preview task failed: {error}")))?;
    attach_previews(&mut mosaic, previews);
    Ok(Json(ApiResponse::success(ProjectMosaicResponse {
        mosaic: Some(mosaic),
    })))
}

fn attach_previews(mosaic: &mut ProjectMosaic, mut previews: HashMap<i32, SkyPreview>) {
    for panel in &mut mosaic.panels {
        panel.preview = previews.remove(&panel.target_id);
    }
}

/// The project's mosaic, Director's record first, then the inferred grid.
pub(crate) fn project_mosaic(
    conn: &Connection,
    project_id: i32,
) -> Result<Option<ProjectMosaic>, AppError> {
    let Some(project_name) = conn
        .query_row(
            "SELECT name FROM project WHERE Id=?1",
            [project_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(AppError::db)?
    else {
        return Ok(None);
    };
    let names = target_names(conn, project_id).map_err(AppError::db)?;
    let recorded = director_cells(conn, project_id).map_err(AppError::db)?;
    let (source, cells) = match recorded {
        Some(cells) => (MosaicSource::Director, Some(cells)),
        None => {
            let inferred = crate::server::director::inferred_mosaic(conn, i64::from(project_id))
                .map_err(|error| AppError::InternalError(error.to_string()))?;
            (
                MosaicSource::Inferred,
                inferred.map(|panels| {
                    panels
                        .into_iter()
                        .map(|panel| Cell {
                            target_id: panel.target_id as i32,
                            panel_id: format!("r{}c{}", panel.row, panel.column),
                            row: panel.row,
                            column: panel.column,
                        })
                        .collect()
                }),
            )
        }
    };
    let Some(mut cells) = cells else {
        return Ok(None);
    };
    if cells.len() < 2 {
        return Ok(None);
    }
    cells.sort_by(|a, b| (a.row, a.column, &a.panel_id).cmp(&(b.row, b.column, &b.panel_id)));
    let rows = cells.iter().map(|cell| cell.row).max().unwrap_or(1);
    let columns = cells.iter().map(|cell| cell.column).max().unwrap_or(1);
    let panels = cells
        .into_iter()
        .map(|cell| MosaicPanel {
            target_id: cell.target_id,
            target_name: names.get(&cell.target_id).cloned().unwrap_or_default(),
            panel_id: cell.panel_id,
            row: cell.row,
            column: cell.column,
            preview: None,
        })
        .collect();
    Ok(Some(ProjectMosaic {
        project_id,
        name: project_name.unwrap_or_default().trim().to_owned(),
        source,
        rows,
        columns,
        panels,
    }))
}

fn target_names(conn: &Connection, project_id: i32) -> rusqlite::Result<HashMap<i32, String>> {
    let mut statement = conn.prepare("SELECT Id, name FROM target WHERE projectid=?1")?;
    let rows = statement.query_map([project_id], |row| {
        Ok((
            row.get::<_, i32>(0)?,
            row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        ))
    })?;
    rows.collect()
}

/// A target's place in the mosaic.
#[derive(Debug, Clone, PartialEq)]
struct Cell {
    target_id: i32,
    panel_id: String,
    row: u32,
    column: u32,
}

/// `r{row}c{column}`, one-based, as Director writes a grid's panel IDs.
fn parse_panel_id(id: &str) -> Option<(u32, u32)> {
    psf_guard_director_core::framing::layout::grid_cell(id)
}

/// The cells Director recorded when it activated this project, when it did:
/// every recorded target of one Director project, with the panel ID it
/// wrote. A grid's IDs are its cells; otherwise the targets' coordinates
/// place them. `None` when the database has no such record.
fn director_cells(conn: &Connection, project_id: i32) -> rusqlite::Result<Option<Vec<Cell>>> {
    let table: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='psf_guard_director_target'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if table.is_none() || conn.prepare("SELECT guid FROM target LIMIT 0").is_err() {
        return Ok(None);
    }
    let mut statement = conn.prepare(
        "SELECT t.Id, d.project_guid, d.panel_id
         FROM target t JOIN psf_guard_director_target d ON d.target_guid = t.guid
         WHERE t.projectid = ?1
         ORDER BY t.Id",
    )?;
    let rows: Vec<(i32, String, String)> = statement
        .query_map(params![project_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let Some((_, project_guid, _)) = rows.first() else {
        return Ok(None);
    };
    // One Director project per Target Scheduler project; a mix is not a
    // record to trust.
    if rows.iter().any(|(_, guid, _)| guid != project_guid) {
        return Ok(None);
    }
    let grid: Option<Vec<Cell>> = rows
        .iter()
        .map(|(target_id, _, panel_id)| {
            parse_panel_id(panel_id).map(|(row, column)| Cell {
                target_id: *target_id,
                panel_id: panel_id.clone(),
                row,
                column,
            })
        })
        .collect();
    if let Some(grid) = grid {
        let mut seen = std::collections::HashSet::new();
        if grid.iter().all(|cell| seen.insert((cell.row, cell.column))) {
            return Ok(Some(grid));
        }
    }
    placed_cells(conn, &rows).map(Some)
}

/// Rows and columns for panels that form no full grid, from where their
/// targets point: each target's center on the plane at their mean, along
/// the first target's camera axes, banded by a quarter of the closest two
/// targets' spacing. Row 1 is the top and column 1 the camera's left (east
/// at angle zero). A cell two targets share puts the later one in a row of
/// its own below, so nothing hides.
fn placed_cells(conn: &Connection, rows: &[(i32, String, String)]) -> rusqlite::Result<Vec<Cell>> {
    use psf_guard_director_core::{framing::TangentPlane, visibility::IcrsPosition};
    let rotation = if conn.prepare("SELECT rotation FROM target LIMIT 0").is_ok() {
        "rotation"
    } else {
        "NULL"
    };
    let mut places = Vec::with_capacity(rows.len());
    for (target_id, _, panel_id) in rows {
        let (ra_hours, dec, angle): (Option<f64>, Option<f64>, Option<f64>) = conn.query_row(
            &format!("SELECT ra, dec, {rotation} FROM target WHERE Id=?1"),
            [target_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let center = ra_hours
            .zip(dec)
            .and_then(|(ra, dec)| crate::astrometry::target_scheduler_coordinates(ra, dec))
            .map(|(ra_degrees, dec_degrees)| IcrsPosition {
                ra_degrees,
                dec_degrees,
            });
        places.push((*target_id, panel_id.clone(), center, angle.unwrap_or(0.0)));
    }
    let located: Vec<_> = places.iter().filter_map(|p| p.2).collect();
    // The plane at the targets' mean, seen from the first target's plane.
    let offsets = located.first().and_then(|first| {
        let plane = TangentPlane::at(*first).ok()?;
        let projected: Option<Vec<[f64; 2]>> = located.iter().map(|c| plane.project(*c)).collect();
        let projected = projected?;
        let n = projected.len() as f64;
        let mean = [
            projected.iter().map(|o| o[0]).sum::<f64>() / n,
            projected.iter().map(|o| o[1]).sum::<f64>() / n,
        ];
        TangentPlane::at(plane.deproject(mean)).ok()
    });
    let angle = places.first().map_or(0.0, |p| p.3);
    let (sin, cos) = angle.to_radians().sin_cos();
    let camera: Vec<Option<(f64, f64)>> = places
        .iter()
        .map(|(_, _, center, _)| {
            let [xi, eta] = offsets.as_ref()?.project((*center)?)?;
            Some((-xi * cos + eta * sin, xi * sin + eta * cos))
        })
        .collect();
    let known: Vec<(f64, f64)> = camera.iter().flatten().copied().collect();
    let mut closest = f64::INFINITY;
    for (i, a) in known.iter().enumerate() {
        for b in &known[i + 1..] {
            let apart = (a.0 - b.0).hypot(a.1 - b.1);
            if apart > 1e-9 {
                closest = closest.min(apart);
            }
        }
    }
    let tolerance = if closest.is_finite() {
        closest / 4.0
    } else {
        1e-6
    };
    let band = |values: Vec<f64>| -> Vec<u32> {
        let mut order: Vec<usize> = (0..values.len()).collect();
        order.sort_by(|a, b| values[*a].total_cmp(&values[*b]));
        let mut out = vec![0; values.len()];
        let (mut current, mut last) = (0, None::<f64>);
        for index in order {
            if last.is_none_or(|last| values[index] - last > tolerance) {
                current += 1;
            }
            last = Some(values[index]);
            out[index] = current;
        }
        out
    };
    // A target with no coordinates sits after the others.
    let columns = band(camera.iter().map(|p| p.map_or(f64::MAX, |p| p.0)).collect());
    let rows_of = band(
        camera
            .iter()
            .map(|p| p.map_or(f64::MAX, |p| -p.1))
            .collect(),
    );
    let mut cells: Vec<Cell> = Vec::with_capacity(places.len());
    let mut extra = rows_of.iter().copied().max().unwrap_or(0);
    let mut taken = std::collections::HashSet::new();
    for (index, (target_id, panel_id, _, _)) in places.into_iter().enumerate() {
        let mut row = rows_of[index];
        if !taken.insert((row, columns[index])) {
            extra += 1;
            row = extra;
            taken.insert((row, columns[index]));
        }
        cells.push(Cell {
            target_id,
            panel_id,
            row,
            column: columns[index],
        });
    }
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO project (Id, profileId, name, state, guid) VALUES
                (1, 'p', 'Heart', 1, 'heart'),
                (2, 'p', 'M31', 1, 'm31'),
                (3, 'p', 'M81', 1, 'm81'),
                (4, 'p', 'Pairs', 1, 'pairs');",
        )
        .unwrap();
        conn
    }

    fn add_target(conn: &Connection, id: i32, project: i32, name: &str, ra_hours: f64, dec: f64) {
        conn.execute(
            "INSERT INTO target (Id, name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
             VALUES (?1, ?2, 1, ?3, ?4, 2, 0, 100, ?5, ?6)",
            params![id, name, ra_hours, dec, project, format!("target-{id}")],
        )
        .unwrap();
    }

    fn cells(mosaic: &ProjectMosaic) -> Vec<(i32, &str)> {
        mosaic
            .panels
            .iter()
            .map(|panel| (panel.target_id, panel.panel_id.as_str()))
            .collect()
    }

    #[test]
    fn a_grid_of_targets_reads_as_its_mosaic_in_row_order() {
        // Askar107PHQ's Heart Mosaic as N.I.N.A. laid it out, stored out of
        // order and with names that say nothing about the cell.
        let conn = catalog();
        add_target(
            &conn,
            10,
            1,
            "Heart d",
            2.445_519_401_920_46,
            60.871_626_084_609_4,
        );
        add_target(
            &conn,
            11,
            1,
            "Heart a",
            2.645_499_681_489_58,
            61.832_231_223_272_4,
        );
        add_target(
            &conn,
            12,
            1,
            "Heart c",
            2.642_430_289_441_8,
            60.871_626_084_609_4,
        );
        add_target(
            &conn,
            13,
            1,
            "Heart b",
            2.442_450_009_872_69,
            61.832_231_223_272_4,
        );
        let mosaic = project_mosaic(&conn, 1).unwrap().expect("a 2×2 mosaic");
        assert_eq!(mosaic.source, MosaicSource::Inferred);
        assert_eq!((mosaic.rows, mosaic.columns), (2, 2));
        // East is left, so the larger right ascension is column 1.
        assert_eq!(
            cells(&mosaic),
            vec![(11, "r1c1"), (13, "r1c2"), (12, "r2c1"), (10, "r2c2")]
        );
        assert_eq!(mosaic.panels[0].target_name, "Heart a");
    }

    #[test]
    fn director_records_name_the_panels_ahead_of_the_coordinates() {
        let conn = catalog();
        // Coordinates that form no grid: only Director's record makes these
        // one mosaic.
        add_target(&conn, 20, 2, "M31 north", 0.70, 42.0);
        add_target(&conn, 21, 2, "M31 south", 0.75, 40.0);
        add_target(&conn, 22, 2, "M31 west", 0.60, 41.5);
        crate::server::director::create_rig_tables_for_tests(&conn);
        conn.execute_batch(
            "INSERT INTO psf_guard_director_target (target_guid, project_guid, panel_id, framing_revision) VALUES
                ('target-20', 'director-m31', 'r1c2', 3),
                ('target-21', 'director-m31', 'r2c1', 3),
                ('target-22', 'director-m31', 'r1c1', 3);",
        )
        .unwrap();
        let mosaic = project_mosaic(&conn, 2)
            .unwrap()
            .expect("Director's mosaic");
        assert_eq!(mosaic.source, MosaicSource::Director);
        assert_eq!((mosaic.rows, mosaic.columns), (2, 2));
        assert_eq!(
            cells(&mosaic),
            vec![(22, "r1c1"), (20, "r1c2"), (21, "r2c1")]
        );
    }

    #[test]
    fn director_panels_placed_one_by_one_read_as_one_mosaic() {
        // A framed target kept in place, one panel added east of it and one
        // north: an L, which no full grid holds.
        let conn = catalog();
        add_target(&conn, 50, 3, "M81", 9.9, 69.0);
        add_target(
            &conn,
            51,
            3,
            "M81 p2",
            9.9 + 1.6 / 15.0 / 69f64.to_radians().cos(),
            69.0,
        );
        add_target(&conn, 52, 3, "M81 p3", 9.9, 70.2);
        crate::server::director::create_rig_tables_for_tests(&conn);
        conn.execute_batch(
            "INSERT INTO psf_guard_director_target (target_guid, project_guid, panel_id, framing_revision) VALUES
                ('target-50', 'director-m81', 'r1c1', 1),
                ('target-51', 'director-m81', 'p2', 4),
                ('target-52', 'director-m81', 'p3', 4);",
        )
        .unwrap();
        let mosaic = project_mosaic(&conn, 3).unwrap().expect("one mosaic");
        assert_eq!(mosaic.source, MosaicSource::Director);
        assert_eq!((mosaic.rows, mosaic.columns), (2, 2));
        // North is the top row; east is the left column.
        assert_eq!(
            mosaic
                .panels
                .iter()
                .map(|p| (p.panel_id.as_str(), p.row, p.column))
                .collect::<Vec<_>>(),
            vec![("p3", 1, 2), ("p2", 2, 1), ("r1c1", 2, 2)]
        );
        assert_eq!(mosaic.panels[2].target_name, "M81");
    }

    #[test]
    fn a_panel_moved_off_its_old_target_keeps_both_in_the_mosaic() {
        let conn = catalog();
        add_target(&conn, 60, 2, "M31 r1c1", 0.70, 41.0);
        add_target(&conn, 61, 2, "M31 r1c2", 0.62, 41.0);
        add_target(&conn, 62, 2, "M31 r1c2", 0.54, 41.0);
        crate::server::director::create_rig_tables_for_tests(&conn);
        conn.execute_batch(
            "INSERT INTO psf_guard_director_target (target_guid, project_guid, panel_id, framing_revision) VALUES
                ('target-60', 'director-m31', 'r1c1', 2),
                ('target-61', 'director-m31', 'r1c2~2', 2),
                ('target-62', 'director-m31', 'r1c2', 3);",
        )
        .unwrap();
        let mosaic = project_mosaic(&conn, 2).unwrap().expect("one mosaic");
        assert_eq!(mosaic.source, MosaicSource::Director);
        assert_eq!(
            mosaic
                .panels
                .iter()
                .map(|p| (p.panel_id.as_str(), p.row, p.column))
                .collect::<Vec<_>>(),
            vec![("r1c1", 1, 1), ("r1c2~2", 1, 2), ("r1c2", 1, 3)]
        );
    }

    #[test]
    fn a_project_that_is_not_a_mosaic_has_none() {
        let conn = catalog();
        add_target(&conn, 30, 3, "M81", 9.9, 69.0);
        assert!(project_mosaic(&conn, 3).unwrap().is_none());
        // Names that read like panels are not a grid.
        add_target(&conn, 40, 4, "Pair r1c1", 5.0, 10.0);
        add_target(&conn, 41, 4, "Pair r1c2", 7.0, -20.0);
        add_target(&conn, 42, 4, "Pair r2c1", 12.0, 45.0);
        assert!(project_mosaic(&conn, 4).unwrap().is_none());
        // A project with no targets, and one that does not exist.
        assert!(project_mosaic(&conn, 99).unwrap().is_none());
    }

    #[test]
    fn panel_ids_parse_only_as_director_writes_them() {
        assert_eq!(parse_panel_id("r2c3"), Some((2, 3)));
        assert_eq!(parse_panel_id("r0c1"), None);
        assert_eq!(parse_panel_id("panel 1"), None);
        assert_eq!(parse_panel_id("r1"), None);
    }
}
