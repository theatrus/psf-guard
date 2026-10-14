//! Where a shot panel's frames really point, so the framing draws a locked
//! panel as it was shot and not with the camera the plan uses now. Fresh
//! pixel-derived plate solves come first; without them, the frames' FITS
//! headers or rotator readings; without those, the target as planned. The
//! source travels with the numbers, so a header or the plan is never shown
//! as a solve.

use crate::server::database_context::DatabaseContext;
use psf_guard_director_core::{
    framing::{
        measured::{median_geometry, FrameSample},
        MeasuredGeometry, MeasuredSource, PanelSize, TangentPlane,
    },
    visibility::IcrsPosition,
};
use rusqlite::{Connection, OptionalExtension};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

/// Solves read per panel, newest first: plenty for a steady median.
const MAX_SOLVED_FRAMES: usize = 200;
/// Headers read per panel when no frame is solved: headers on network
/// storage are slow, and frames of one panel agree.
const MAX_HEADER_FRAMES: usize = 8;

/// A light of the target, newest first.
struct Frame {
    id: i32,
    metadata: String,
}

/// What a light's header says about where it was shot.
#[derive(Clone, Copy)]
struct HeaderSample {
    center: Option<IcrsPosition>,
    position_angle_degrees: Option<f64>,
    size: Option<PanelSize>,
}

fn header_cache() -> &'static Mutex<HashMap<PathBuf, Option<HeaderSample>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<HeaderSample>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The camera's up, east of north, from a CD matrix: the field rotation the
/// sequence analysis reads, turned to sky handedness. The same angle Target
/// Scheduler stores as a target's rotation.
fn position_angle_from_cd(cd: [[f64; 2]; 2]) -> Option<f64> {
    let (rotation, mirrored) = crate::sequence_analysis::field_rotation_from_cd(cd)?;
    let angle = if mirrored { -rotation } else { rotation };
    angle.is_finite().then(|| angle.rem_euclid(360.0) % 360.0)
}

fn position(ra_degrees: f64, dec_degrees: f64) -> Option<IcrsPosition> {
    let position = IcrsPosition {
        ra_degrees: ra_degrees.rem_euclid(360.0) % 360.0,
        dec_degrees,
    };
    (ra_degrees.is_finite() && dec_degrees.is_finite() && (-90.0..=90.0).contains(&dec_degrees))
        .then_some(position)
}

fn field(width_pixels: f64, height_pixels: f64, arcsec_per_pixel: f64) -> Option<PanelSize> {
    let size = PanelSize {
        width_degrees: width_pixels * arcsec_per_pixel / 3600.0,
        height_degrees: height_pixels * arcsec_per_pixel / 3600.0,
    };
    size.validate().is_ok().then_some(size)
}

/// One frame's fresh pixel-derived solve, when the quality scan made one
/// and the frame's file has not changed since.
fn solved_sample(context: &DatabaseContext, frame: &Frame) -> Option<FrameSample> {
    let analysis =
        context
            .astrometry_evidence
            .evidence_for_source(&context.cache_dir_path, frame.id, None)?;
    let metrics = crate::sequence_analysis::astrometry_metrics_from_analysis(&analysis)?;
    if !metrics.pixel_solved {
        return None;
    }
    let solution = analysis.solution?;
    Some(FrameSample {
        center: position(solution.center_ra_deg, solution.center_dec_deg)?,
        position_angle_degrees: position_angle_from_cd(solution.wcs.cd),
        size: field(
            f64::from(solution.image_width),
            f64::from(solution.image_height),
            solution.pixel_scale_arcsec_per_pixel,
        ),
    })
}

/// What a frame's header says, read once per file for the process.
fn header_sample(path: &Path) -> Option<HeaderSample> {
    if let Some(known) = header_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(path).copied())
    {
        return known;
    }
    let sample = crate::astrometry_headers::FitsAstrometryHeaders::from_path(path)
        .ok()
        .map(|headers| {
            let width = headers.width.as_ref().map(|w| f64::from(w.value));
            let height = headers.height.as_ref().map(|h| f64::from(h.value));
            let wcs = headers.embedded_wcs.as_ref().map(|wcs| &wcs.value);
            // The embedded solution's image center, else the mount's
            // pointing as the header records it.
            let wcs_center = wcs.zip(width.zip(height)).and_then(|(wcs, (w, h))| {
                let [[cd11, cd12], [cd21, cd22]] = wcs.cd;
                let dx = (w - 1.0) / 2.0 - wcs.crpix[0];
                let dy = (h - 1.0) / 2.0 - wcs.crpix[1];
                let plane = TangentPlane::at(position(wcs.crval[0], wcs.crval[1])?).ok()?;
                Some(plane.deproject([cd11 * dx + cd12 * dy, cd21 * dx + cd22 * dy]))
            });
            let center = wcs_center
                .or_else(|| position(headers.center_ra_deg?.value, headers.center_dec_deg?.value));
            HeaderSample {
                center,
                position_angle_degrees: wcs.and_then(|wcs| position_angle_from_cd(wcs.cd)),
                size: width.zip(height).and_then(|(w, h)| {
                    field(w, h, headers.pixel_scale_arcsec_per_pixel.as_ref()?.value)
                }),
            }
        });
    if let Ok(mut cache) = header_cache().lock() {
        cache.insert(path.to_path_buf(), sample);
    }
    sample
}

/// The target as Target Scheduler holds it: center and planned rotation.
fn planned(
    connection: &Connection,
    target_id: i64,
) -> rusqlite::Result<Option<(Option<IcrsPosition>, Option<f64>)>> {
    let rotation = if super::import_drafts::has_column(connection, "target", "rotation") {
        "rotation"
    } else {
        "NULL"
    };
    connection
        .query_row(
            &format!("SELECT ra, dec, {rotation} FROM target WHERE Id = ?1"),
            [target_id],
            |row| {
                let ra: Option<f64> = row.get(0)?;
                let dec: Option<f64> = row.get(1)?;
                let rotation: Option<f64> = row.get(2)?;
                // Target Scheduler stores right ascension in hours.
                Ok((
                    ra.zip(dec).and_then(|(ra, dec)| position(ra * 15.0, dec)),
                    rotation.filter(|value| value.is_finite()),
                ))
            },
        )
        .optional()
}

fn frames(connection: &Connection, target_id: i64) -> rusqlite::Result<Vec<Frame>> {
    // Rejected frames are not where the panel was shot well enough to keep.
    let mut statement = connection.prepare(
        "SELECT Id, IFNULL(metadata, '') FROM acquiredimage
         WHERE targetId = ?1 AND IFNULL(gradingStatus, 0) <> 2
         ORDER BY acquireddate DESC, Id DESC",
    )?;
    let rows = statement.query_map([target_id], |row| {
        Ok(Frame {
            id: row.get(0)?,
            metadata: row.get(1)?,
        })
    })?;
    rows.collect()
}

/// Where a target's frames were shot. `size` stands in for the field when
/// no solve or header gives one: the size the panel was measured with
/// before, else the plan's panel. `None` when the target has no frames
/// that are not rejected.
pub(super) fn measure_target(
    context: &DatabaseContext,
    connection: &Connection,
    target_id: i64,
    size: Option<PanelSize>,
) -> rusqlite::Result<Option<MeasuredGeometry>> {
    let frames = frames(connection, target_id)?;
    if frames.is_empty() {
        return Ok(None);
    }
    let Some((plan_center, plan_rotation)) = planned(connection, target_id)? else {
        return Ok(None);
    };
    let reference = plan_rotation.unwrap_or(0.0);
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);

    // Fresh plate solves of the pixels: evidence of where they point.
    let solved: Vec<FrameSample> = frames
        .iter()
        .take(MAX_SOLVED_FRAMES)
        .filter_map(|frame| solved_sample(context, frame))
        .collect();
    if let Some((center, angle, measured_size)) = median_geometry(&solved, reference) {
        let (size, size_source) = match (measured_size, size) {
            (Some(measured), _) => (measured, MeasuredSource::Solved),
            (None, Some(size)) => (size, MeasuredSource::Plan),
            (None, None) => return Ok(None),
        };
        return Ok(Some(MeasuredGeometry {
            center,
            position_angle_degrees: angle.unwrap_or_else(|| reference.rem_euclid(360.0)),
            size,
            source: MeasuredSource::Solved,
            size_source,
            frames: count(solved.len()),
        }));
    }

    // The frames' headers: what the mount and rotator reported.
    let tree = context.get_directory_tree().ok();
    let headers: Vec<(HeaderSample, Option<f64>)> = tree
        .iter()
        .flat_map(|tree| {
            frames.iter().filter_map(move |frame| {
                let name = crate::server::handlers::filename_from_metadata(&frame.metadata)?;
                let path = tree.find_file_first(&name)?.clone();
                Some((path, frame))
            })
        })
        .take(MAX_HEADER_FRAMES)
        .filter_map(|(path, frame)| {
            header_sample(&path).map(|sample| {
                (
                    sample,
                    crate::server::handlers::rotator_position_from_metadata(&frame.metadata),
                )
            })
        })
        .collect();
    let samples: Vec<FrameSample> = headers
        .iter()
        .filter_map(|(sample, rotator)| {
            Some(FrameSample {
                center: sample.center?,
                position_angle_degrees: sample.position_angle_degrees.or(*rotator),
                size: sample.size,
            })
        })
        .collect();
    let header_size = || {
        let sized: Vec<FrameSample> = headers
            .iter()
            .filter_map(|(sample, _)| {
                Some(FrameSample {
                    center: plan_center?,
                    position_angle_degrees: None,
                    size: Some(sample.size?),
                })
            })
            .collect();
        median_geometry(&sized, reference).and_then(|(_, _, size)| size)
    };
    let size_of = |measured: Option<PanelSize>| match (measured.or_else(header_size), size) {
        (Some(measured), _) => Some((measured, MeasuredSource::Header)),
        (None, Some(size)) => Some((size, MeasuredSource::Plan)),
        (None, None) => None,
    };
    if let Some((center, angle, measured_size)) = median_geometry(&samples, reference) {
        let Some((size, size_source)) = size_of(measured_size) else {
            return Ok(None);
        };
        return Ok(Some(MeasuredGeometry {
            center,
            position_angle_degrees: angle.unwrap_or_else(|| reference.rem_euclid(360.0)),
            size,
            source: MeasuredSource::Header,
            size_source,
            frames: count(samples.len()),
        }));
    }

    // Nothing read from the frames: the target where the plan put it.
    let Some(center) = plan_center else {
        return Ok(None);
    };
    let Some((size, size_source)) = size_of(None) else {
        return Ok(None);
    };
    Ok(Some(MeasuredGeometry {
        center,
        position_angle_degrees: reference.rem_euclid(360.0) % 360.0,
        size,
        source: MeasuredSource::Plan,
        size_source,
        frames: count(frames.len()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_solved_cd_matrix_gives_the_angle_target_scheduler_stores() {
        let s = 1.0 / 3600.0;
        // North up and east left as the frame is drawn: no turn.
        assert_eq!(position_angle_from_cd([[-s, 0.0], [0.0, -s]]), Some(0.0));
        // Camera up turned east of north: north leans toward the frame's
        // right by the same angle, the field rotation the sequence analysis
        // compares with Target Scheduler's.
        for turn in [30.0_f64, -30.0, 120.0] {
            let (sin, cos) = turn.to_radians().sin_cos();
            let cd = [[-s * cos, -s * sin], [s * sin, -s * cos]];
            let (rotation, mirrored) =
                crate::sequence_analysis::field_rotation_from_cd(cd).unwrap();
            assert!(!mirrored);
            assert!((rotation - turn).abs() < 1e-9, "{turn}: {rotation}");
            let angle = position_angle_from_cd(cd).unwrap();
            assert!(
                (angle - turn.rem_euclid(360.0)).abs() < 1e-9,
                "{turn}: {angle}"
            );
        }
        // A mirrored field turns the other way for the same angle.
        let mirrored = position_angle_from_cd([[s, 0.0], [0.0, -s]]).unwrap();
        assert!(
            mirrored.abs() < 1e-9 || (mirrored - 360.0).abs() < 1e-9,
            "{mirrored}"
        );
    }
}
