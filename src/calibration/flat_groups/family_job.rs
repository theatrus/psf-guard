//! The measuring pass's last stage: ask lights whose angle points at two
//! or more response families which one their flats come from.
//!
//! The lights of one imaging session are asked per filter, target and
//! recorded angle (lights within the rig's rotation tolerance of each
//! other), so a background never mixes two fields or two angles. For each
//! such group whose lights have two or more candidate families
//! ([`Book::family_candidates`]): the sky background of up to
//! [`evidence::MAX_LIGHTS`] of its lights, fitted against each pair of the
//! candidates' flats (the session of each family that stands for it,
//! [`Book::family_representative`]) by Seiza's `flat_response::choice`. The
//! answer is kept per group ([`super::family_store`]), so choosing flats
//! reads no pixels. A group whose answer is still current under its key is
//! not asked again, and no file of it is read, not even a header.
//!
//! Which groups are asked and what their answer is are Seiza's
//! (`seiza_calibration::library::family_questions`), shared with the
//! desktop apps; this module reads the catalog's lights, their headers and
//! their pixels.
//!
//! [`evidence::MAX_LIGHTS`]: super::evidence::MAX_LIGHTS

use super::family_store::Kept;
use super::job::{read_binned, session_plane, Pass, PedestalTable};
use super::measure::rows::FamilyEvidenceRow;
use super::metric::Plane;
use super::model::{Book, ImagingSession};
use crate::calibration::LibrarySnapshot;
use anyhow::Result;
use rayon::prelude::*;
use seiza_calibration::library::EvidenceLight;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One group of an imaging session's lights the stage asks.
pub(crate) type Question = seiza_calibration::library::FamilyQuestion;

pub(crate) fn questions(
    pass: &Pass<'_>,
    library: &LibrarySnapshot,
    book: &Book,
    for_rig: &str,
) -> Result<Vec<Question>> {
    if book.imaging.is_empty() {
        return Ok(Vec::new());
    }
    // A calibration library without lights has nothing to ask.
    let Ok(mut statement) = pass.conn.prepare(
        "SELECT ai.filtername, ai.metadata, t.name, ai.acquireddate FROM acquiredimage ai
         LEFT JOIN target t ON t.Id = ai.targetId
         WHERE ai.Id = ?1 AND ai.gradingStatus IS NOT 2",
    ) else {
        return Ok(Vec::new());
    };
    let mut lights_of = |session: &ImagingSession| -> Vec<EvidenceLight> {
        let mut lights = Vec::new();
        for id in &session.lights {
            let row: Option<(Option<String>, String, Option<String>, i64)> = statement
                .query_row([id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .ok();
            let Some((filter, metadata, target, at)) = row else {
                continue;
            };
            let Some(name) = crate::utils::extract_filename(&metadata) else {
                continue;
            };
            let Some(path) = (pass.find_light)(crate::utils::file_name_of(&name)) else {
                continue;
            };
            // Its mechanical angle from N.I.N.A.'s record; the stage reads a
            // header for lights without one.
            let angle = serde_json::from_str::<serde_json::Value>(&metadata)
                .ok()
                .and_then(|value| value.get("RotatorMechanicalPosition")?.as_f64())
                .filter(|angle| angle.is_finite());
            lights.push(EvidenceLight {
                path,
                at,
                angle,
                filter: filter.unwrap_or_default(),
                target: target.unwrap_or_default(),
            });
        }
        lights
    };
    let mut header = |path: &Path| {
        crate::calibration::library_light(&crate::commands::import::headers::read_frame_meta(path))
    };
    Ok(seiza_calibration::library::family_questions(
        library.library(),
        &crate::calibration::rules::rule_book(),
        for_rig,
        &mut lights_of,
        &mut header,
    ))
}

/// Whether a question's answer is kept under its current key.
pub(crate) fn answered(question: &Question) -> bool {
    question.answered
}

/// The place a question's row is kept under.
pub(crate) fn kept(book: &Book, question: &Question) -> Kept {
    question.retention_key(book)
}

/// Read a question's lights and fit their background against every pair
/// of its candidates.
///
/// `None` when none of its lights could be read (the image folders do not
/// hold them yet): the caller tries again later. Lights read but nothing
/// to fit keep a row that chooses nothing, so they are not read on every
/// pass.
pub(crate) fn answer(
    pass: &Pass<'_>,
    book: &Book,
    question: &Question,
    pedestals: &PedestalTable,
    frames_read: &AtomicUsize,
) -> Option<FamilyEvidenceRow> {
    // Each candidate's flats nearest the session in time, pedestal out.
    let representatives = question.representatives(book)?;
    let planes: Vec<Option<Plane>> = representatives
        .iter()
        .map(|index| session_plane(pass.dir, &book.sessions[*index].row))
        .collect();
    let picked = question.sample();
    let yield_now = pass.yield_now;
    let read: Vec<Result<(Plane, usize)>> = pass.in_pool(|| {
        picked
            .par_iter()
            .map(|light| {
                yield_now();
                read_binned(&light.path)
            })
            .collect()
    });
    let binned: Vec<Plane> = read
        .into_iter()
        .filter_map(Result::ok)
        .map(|(plane, _)| plane)
        .collect();
    if binned.is_empty() {
        return None;
    }
    frames_read.fetch_add(binned.len(), Ordering::Relaxed);
    let meta = crate::commands::import::headers::read_frame_meta(&picked[0].path);
    let pedestal = pedestals
        .pedestal_for(&question.rig, meta.gain, meta.offset, None)
        .unwrap_or(0.0) as f32;
    let row = pass.in_pool(|| question.decide(book, &representatives, binned, planes, pedestal));
    tracing::debug!(
        "family evidence {} {}: chose {:?} of {:?}",
        row.session_key,
        row.filter,
        row.chosen,
        row.candidates
    );
    Some(row)
}
