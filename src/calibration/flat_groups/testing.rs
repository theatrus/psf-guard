//! Measurements made up for tests: a catalog's flat sessions as the pass
//! would record them, with the differences a test names, and no pixels.

use super::metric::{Comparison, Spot, SpotKind};
use super::model::Book;
use super::store::{self, ResponseRow};
use crate::calibration::{catalog_night_boundary, LibrarySnapshot};
use rusqlite::Connection;

/// Record every flat session of the library as measured, and compare every
/// pair the grouping asks for with `dust`: a fraction of the background,
/// shown as one new shadow at (100, 100) when it is over half a percent.
pub(crate) fn measure_as(conn: &Connection, dust: impl Fn(&ResponseRow, &ResponseRow) -> f64) {
    store::add_tables(conn).unwrap();
    let library = LibrarySnapshot::load(conn).unwrap();
    let plans = super::job::plan_sessions(library.frames(), catalog_night_boundary(conn));
    for plan in plans
        .iter()
        .filter(|plan| plan.frames.len() >= super::metric::MIN_FRAMES)
    {
        store::upsert_response(
            conn,
            &ResponseRow {
                response_key: plan.response_key(),
                session_key: plan.session_key.clone(),
                rig_uuid: plan.rig_uuid.clone(),
                family_key: plan.family_key.clone(),
                night: plan.night.clone(),
                filter: plan.filter.clone(),
                rotation: plan.rotation,
                frame_uuids: plan.frames.iter().map(|f| f.frame_uuid.clone()).collect(),
                frames: plan.frames.len(),
                captured_first: plan.frames.first().and_then(|f| f.captured_at),
                captured_last: plan.frames.last().and_then(|f| f.captured_at),
                level_adu: Some(30_000.0),
                pedestal_adu: None,
                noise: Some(0.0004),
                width: Some(100),
                height: Some(100),
                bin: Some(1),
                algorithm_version: super::metric::RESPONSE_ALGORITHM_VERSION,
                error: None,
            },
        )
        .unwrap();
    }
    // A catalog on disk gets a plane for each response, flat, beside it,
    // so a change's map can be made as after a real pass.
    if let Some(catalog) = conn.path().filter(|path| !path.is_empty()) {
        let dir = std::path::PathBuf::from(format!("{catalog}.responses"));
        std::fs::create_dir_all(&dir).unwrap();
        for row in store::load_responses(conn).unwrap() {
            let (width, height) = (row.width.unwrap_or(1), row.height.unwrap_or(1));
            let plane = super::metric::Plane {
                width,
                height,
                data: vec![1.0; width * height],
            };
            std::fs::write(
                super::response_path(&dir, &row.response_key),
                super::metric::encode_plane(&plane),
            )
            .unwrap();
        }
        super::register_response_dir(std::path::Path::new(catalog), dir);
    }
    let book = Book::build(
        store::load_responses(conn).unwrap(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        super::rig_limits,
    );
    for (a, b) in book.pairs_to_compare() {
        let (earlier, later) = (&book.sessions[a].row, &book.sessions[b].row);
        let value = dust(earlier, later);
        let spots = if value > 0.005 {
            vec![Spot {
                kind: SpotKind::New,
                darker: true,
                x: 100.0,
                y: 100.0,
                radius: 40.0,
                area: 5_000.0,
                peak: value as f32,
            }]
        } else {
            Vec::new()
        };
        let comparison = Comparison {
            algorithm_version: super::metric::COMPARE_ALGORITHM_VERSION,
            broad: 0.001,
            broad_x: 0.5,
            broad_y: 0.5,
            dust: value as f32,
            sigma: 0.0002,
            spots,
            star_fraction: 0.0,
        };
        store::insert_comparison(
            conn,
            &earlier.response_key,
            &later.response_key,
            &comparison,
        )
        .unwrap();
    }
}
