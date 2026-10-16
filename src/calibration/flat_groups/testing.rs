//! Measurements made up for tests: a catalog's flat sessions as the pass
//! would record them, with the differences a test names, and no pixels.

use super::metric::{Comparison, Spot, SpotKind};
use super::model::Book;
use super::store::{self, ResponseRow};
use crate::calibration::{LibrarySnapshot, Nights};
use rusqlite::Connection;

/// Record every flat session of the library as measured, and compare every
/// pair the grouping asks for with `dust`: a fraction of the background,
/// shown as one new shadow at (100, 100) when it is over half a percent.
pub(crate) fn measure_as(conn: &Connection, dust: impl Fn(&ResponseRow, &ResponseRow) -> f64) {
    store::add_tables(conn).unwrap();
    let library = LibrarySnapshot::load(conn).unwrap();
    let plans = super::job::plan_sessions(library.frames(), &Nights::load(conn));
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

/// A deterministic, roughly normal number with unit spread.
pub(crate) fn noise(x: usize, y: usize, seed: u64) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ seed.wrapping_mul(0x1656_67B1_9E37_79F9);
    let mut sum = 0f32;
    for _ in 0..4 {
        h ^= h >> 33;
        h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        h ^= h >> 33;
        sum += (h >> 40) as f32 / (1u64 << 24) as f32;
    }
    (sum - 2.0) * 1.732
}

/// A synthetic flat response for test frames: radial vignetting, dust
/// shadows `(x, y, radius, depth)` in plane pixels, a linear gradient, stars
/// `(x, y, height)`, and noise of `noise_level`.
pub(crate) fn synthetic(
    width: usize,
    height: usize,
    dust: &[(f32, f32, f32, f32)],
    gradient: f32,
    stars: &[(f32, f32, f32)],
    noise_level: f32,
    seed: u64,
) -> super::metric::Plane {
    let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
    let r_max = (cx * cx + cy * cy).sqrt();
    let mut data = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            let r = (dx * dx + dy * dy).sqrt() / r_max;
            let mut value = 1.0 - 0.3 * r * r;
            value *= 1.0 + gradient * (x as f32 / width as f32 - 0.5);
            for (sx, sy, radius, depth) in dust {
                let d = ((x as f32 - sx).powi(2) + (y as f32 - sy).powi(2)).sqrt();
                let edge = ((radius - d) / 2.0).clamp(0.0, 1.0);
                value *= 1.0 - depth * edge;
            }
            for (sx, sy, peak) in stars {
                let d2 = (x as f32 - sx).powi(2) + (y as f32 - sy).powi(2);
                value += peak * (-d2 / 2.0).exp();
            }
            value *= 1.0 + noise_level * noise(x, y, seed);
            data.push(value);
        }
    }
    super::metric::Plane {
        width,
        height,
        data,
    }
}

/// Record that the lights of `light`'s imaging session were read and
/// follow the family at its recorded angle, as the measuring job would
/// store it: the light is added to the catalog's lights (one imaging
/// session of its own), and the answer is kept under the key its
/// candidates give now.
pub(crate) fn lights_confirm_their_family(
    conn: &Connection,
    light: &crate::commands::import::headers::FrameMeta,
) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER,
            targetId INTEGER, acquireddate INTEGER, filtername TEXT, gradingStatus INTEGER,
            metadata TEXT);",
    )
    .unwrap();
    let at = light.timestamp.expect("the light's time");
    conn.execute(
        "INSERT INTO acquiredimage (projectId, targetId, acquireddate, filtername, gradingStatus,
         metadata) VALUES (1, 1, ?1, ?2, 0, '{}')",
        rusqlite::params![at, light.filter],
    )
    .unwrap();
    super::family_store::add_table(conn).unwrap();
    let rules = crate::calibration::rules::rules_for_light(light);
    let rig = crate::calibration::rules::light_rig_uuid(light);
    let angle = light.rotator_position.expect("the light's angle");
    let library = LibrarySnapshot::load(conn).unwrap();
    let book = library.flat_book();
    let compatible = library.flat_family_compatible(light, &rules);
    let candidates: Vec<usize> = book
        .family_candidates(&rig, Some(at), angle, &compatible)
        .into_iter()
        .map(|(family, _)| family)
        .collect();
    assert!(
        candidates.len() >= 2,
        "the lights are asked between families"
    );
    let chosen = book
        .at_recorded(&candidates, angle, rules.rotation_deg)
        .into_iter()
        .next()
        .expect("a family at the light's angle");
    let session = book.imaging_session(&rig, at).expect("its imaging session");
    super::family_store::upsert(
        conn,
        &super::measure::rows::FamilyEvidenceRow {
            session_key: session.key.clone(),
            filter: super::model::canonical_filter(light.filter.as_deref().unwrap_or("")),
            target: String::new(),
            angle,
            angle_low: angle,
            angle_high: angle,
            start: session.start,
            end: session.end,
            key: book.family_evidence_key(&candidates, &rig, session.start, angle),
            candidates: candidates
                .iter()
                .map(|f| book.families[*f].key.clone())
                .collect(),
            chosen: Some(book.families[chosen].key.clone()),
            lights: 1,
            targets: Vec::new(),
            pairs: Vec::new(),
        },
    )
    .unwrap();
}
