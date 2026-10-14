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

/// Solves read per panel, newest first: plenty for a steady median, and
/// few enough stats of files on network storage per mosaic read.
const MAX_SOLVED_FRAMES: usize = 60;
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

/// A frame file as it is now: its path, size and modification time. A
/// header read is kept under this, so a replaced file is read again.
type HeaderKey = (PathBuf, u64, Option<std::time::SystemTime>);

fn header_cache() -> &'static Mutex<HashMap<HeaderKey, HeaderSample>> {
    static CACHE: OnceLock<Mutex<HashMap<HeaderKey, HeaderSample>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// An angle in [0, 360): `rem_euclid` alone gives 360 for a hair below 0.
fn whole_turn(degrees: f64) -> f64 {
    degrees.rem_euclid(360.0) % 360.0
}

/// The camera's up, east of north, from a CD matrix: the field rotation the
/// sequence analysis reads, turned to sky handedness. The same angle Target
/// Scheduler stores as a target's rotation.
fn position_angle_from_cd(cd: [[f64; 2]; 2]) -> Option<f64> {
    let (rotation, mirrored) = crate::sequence_analysis::field_rotation_from_cd(cd)?;
    let angle = if mirrored { -rotation } else { rotation };
    angle.is_finite().then(|| whole_turn(angle))
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

/// What a frame's header says, read once per version of the file: kept
/// under its size and modification time, so a replaced frame is read again.
/// A read that fails is not kept, so a passing network error does not drop
/// the frame for good.
fn header_sample(path: &Path) -> Option<HeaderSample> {
    let metadata = std::fs::metadata(path).ok()?;
    let key: HeaderKey = (path.to_path_buf(), metadata.len(), metadata.modified().ok());
    if let Some(known) = header_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&key).copied())
    {
        return Some(known);
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
    if let (Some(sample), Ok(mut cache)) = (sample, header_cache().lock()) {
        cache.insert(key, sample);
    }
    sample
}

/// The target as Target Scheduler holds it: center and planned rotation.
/// A target kept in JNow coordinates has no ICRS center to give.
fn planned(
    connection: &Connection,
    target_id: i64,
) -> rusqlite::Result<Option<(Option<IcrsPosition>, Option<f64>)>> {
    let column = |name: &'static str| {
        if super::import_drafts::has_column(connection, "target", name) {
            name
        } else {
            "NULL"
        }
    };
    connection
        .query_row(
            &format!(
                "SELECT ra, dec, {}, IFNULL({}, 2) FROM target WHERE Id = ?1",
                column("rotation"),
                column("epochcode")
            ),
            [target_id],
            |row| {
                let ra: Option<f64> = row.get(0)?;
                let dec: Option<f64> = row.get(1)?;
                let rotation: Option<f64> = row.get(2)?;
                let epoch: i64 = row.get(3)?;
                // Target Scheduler stores right ascension in hours.
                Ok((
                    ra.zip(dec)
                        .filter(|_| epoch == 2)
                        .and_then(|(ra, dec)| position(ra * 15.0, dec)),
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
            position_angle_degrees: angle.unwrap_or_else(|| whole_turn(reference)),
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
            position_angle_degrees: angle.unwrap_or_else(|| whole_turn(reference)),
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
        position_angle_degrees: whole_turn(reference),
        size,
        source: MeasuredSource::Plan,
        size_source,
        frames: count(frames.len()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four real lights from N.I.N.A. 3 rigs with rotators (Askar 107,
    /// RedCat 61, C925 and Ultracat 108, planned at 75°, 319.31°, 160° and
    /// 35°), solved fresh from their pixels: each CD matrix reads as the sky
    /// angle the rotator reported (`RotatorPosition`), a half turn either way
    /// being the same framing. A handedness error would read the mirror
    /// angle, tens of degrees away.
    #[test]
    fn real_rotator_frames_read_as_the_angle_the_rotator_reported() {
        let frames: [([[f64; 2]; 2], f64); 4] = [
            (
                [
                    [6.990482307099638e-5, 2.773326610787368e-4],
                    [-2.773614711812854e-4, 6.989856989050347e-5],
                ],
                75.5322036743164,
            ),
            (
                [
                    [5.446752282112514e-4, -4.7006450224301835e-4],
                    [4.7064917129732616e-4, 5.436290419800887e-4],
                ],
                139.1587677001953,
            ),
            (
                [
                    [-8.540919365504887e-5, 3.0773532793127025e-5],
                    [-3.0762088540537845e-5, -8.53921599650019e-5],
                ],
                340.1150817871094,
            ),
            (
                [
                    [3.3623309231744136e-4, 2.4357134561677068e-4],
                    [-2.4351777146067272e-4, 3.3630491101969084e-4],
                ],
                35.01029968261719,
            ),
        ];
        let half_turn_apart = |a: f64, b: f64| {
            let d = (a - b).rem_euclid(180.0);
            d.min(180.0 - d)
        };
        for (cd, reported) in frames {
            let angle = position_angle_from_cd(cd).unwrap();
            assert!(
                half_turn_apart(angle, reported) < 1.5,
                "{angle} against {reported}"
            );
            assert!(
                half_turn_apart(-angle, reported) > 10.0,
                "{angle} against {reported}"
            );
        }
    }

    fn write_header(path: &Path, dec: f64) {
        let cards = [
            "SIMPLE  =                    T".to_string(),
            "BITPIX  =                   16".into(),
            "NAXIS   =                    2".into(),
            "NAXIS1  =                  100".into(),
            "NAXIS2  =                   80".into(),
            "RA      =                 38.2".into(),
            format!("DEC     = {dec:>20}"),
            "END".into(),
        ];
        let mut header = Vec::new();
        for card in &cards {
            let mut bytes = card.as_bytes().to_vec();
            bytes.resize(80, b' ');
            header.extend(bytes);
        }
        header.resize(header.len().div_ceil(2880) * 2880, b' ');
        std::fs::write(path, header).unwrap();
    }

    /// A frame that cannot be read yet is read again later, and a frame
    /// replaced on disk is read anew: the cache keys on the file as it is.
    #[test]
    fn a_header_read_is_kept_per_version_of_the_file_and_failures_are_not_kept() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("frame.fits");
        assert!(header_sample(&path).is_none());
        std::fs::write(&path, b"not a fits file").unwrap();
        assert!(header_sample(&path).is_none());
        write_header(&path, 61.45);
        let first = header_sample(&path).unwrap();
        assert!((first.center.unwrap().dec_degrees - 61.45).abs() < 1e-9);
        // Replaced with another frame: a new size or time, read anew.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_header(&path, 12.5);
        let second = header_sample(&path).unwrap();
        assert!((second.center.unwrap().dec_degrees - 12.5).abs() < 1e-9);
    }

    #[test]
    fn a_fallback_angle_a_hair_below_zero_is_a_whole_turn_angle() {
        assert_eq!((-1e-14f64).rem_euclid(360.0), 360.0);
        assert_eq!(whole_turn(-1e-14), 0.0);
        assert_eq!(whole_turn(-90.0), 270.0);
        assert_eq!(whole_turn(720.5), 0.5);
    }

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
