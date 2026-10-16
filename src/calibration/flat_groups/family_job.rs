//! The measuring pass's last stage: ask lights whose angle points at two
//! or more response families which one their flats come from.
//!
//! For each imaging session and filter whose lights have two or more
//! candidate families ([`Book::family_candidates`]): the sky background of
//! up to [`evidence::MAX_LIGHTS`] of its lights, fitted against each pair of
//! the candidates' flats (the session of each family nearest in time) by
//! Seiza's `flat_response::choice`. The answer is kept per session and
//! filter ([`super::family_store`]), so choosing flats reads no pixels.

use super::job::{pedestal_for, read_binned, session_plane, Pass, PedestalTable};
use super::measure::rows::{FamilyEvidenceRow, FamilyPairRow};
use super::metric::Plane;
use super::model::{canonical_filter, Book};
use super::{choice, evidence};
use crate::calibration::LibrarySnapshot;
use anyhow::Result;
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One light of an imaging session, as the stage needs it.
struct Light {
    path: PathBuf,
    target: String,
    at: i64,
    /// Its mechanical angle from N.I.N.A.'s record, else none.
    angle: Option<f64>,
}

/// The lights of each imaging session and filter that the stage asks:
/// those with two or more candidate families, with the candidates.
pub(crate) struct Question {
    session: usize,
    rig: String,
    filter: String,
    lights: Vec<Light>,
    candidates: Vec<usize>,
    /// The asked lights' middle angle.
    angle: f64,
    key: String,
}

pub(crate) fn questions(
    pass: &Pass<'_>,
    library: &LibrarySnapshot,
    book: &Book,
    for_rig: &str,
) -> Result<Vec<Question>> {
    let mut out = Vec::new();
    if book.imaging.is_empty() {
        return Ok(out);
    }
    // A calibration library without lights has nothing to ask.
    let Ok(mut statement) = pass.conn.prepare(
        "SELECT ai.filtername, ai.metadata, t.name, ai.acquireddate FROM acquiredimage ai
         LEFT JOIN target t ON t.Id = ai.targetId
         WHERE ai.Id = ?1 AND ai.gradingStatus IS NOT 2",
    ) else {
        return Ok(out);
    };
    for (index, session) in book.imaging.iter().enumerate() {
        let measured: Vec<&String> = session
            .rigs
            .iter()
            .filter(|rig| book.limits(rig).is_some_and(|limits| limits.measured))
            .collect();
        let [rig] = measured.as_slice() else {
            continue;
        };
        if rig.as_str() != for_rig {
            continue;
        }
        let mut by_filter: BTreeMap<String, Vec<Light>> = BTreeMap::new();
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
            let angle = serde_json::from_str::<serde_json::Value>(&metadata)
                .ok()
                .and_then(|value| value.get("RotatorMechanicalPosition")?.as_f64())
                .filter(|angle| angle.is_finite());
            by_filter
                .entry(filter.unwrap_or_default())
                .or_default()
                .push(Light {
                    path,
                    target: target.unwrap_or_default(),
                    at,
                    angle,
                });
        }
        for (filter, lights) in by_filter {
            if lights.len() < evidence::MIN_LIGHTS {
                continue;
            }
            // The first light's header: its sensor, optics and rig.
            let mut meta = crate::commands::import::headers::read_frame_meta(&lights[0].path);
            if crate::calibration::rules::light_rig_uuid(&meta) != **rig {
                continue;
            }
            meta.filter = Some(filter.clone());
            let rules = crate::calibration::rules_for_light(&meta);
            let compatible = library.flat_family_compatible(&meta, &rules);
            if compatible.len() < 2 {
                continue;
            }
            let mut asked: BTreeSet<usize> = BTreeSet::new();
            let mut angles: Vec<f64> = Vec::new();
            for light in &lights {
                let Some(angle) = light.angle.or(meta.rotator_position) else {
                    continue;
                };
                let candidates = book.family_candidates(rig, Some(light.at), angle, &compatible);
                if candidates.len() >= 2 {
                    asked.extend(candidates.iter().map(|(family, _)| *family));
                    angles.push(angle);
                }
            }
            if asked.len() < 2 {
                continue;
            }
            let candidates: Vec<usize> = asked.into_iter().collect();
            // The lights' middle angle stands for them.
            angles.sort_by(f64::total_cmp);
            let angle = angles[angles.len() / 2];
            let key = book.family_evidence_key(&candidates, rig, session.start, angle);
            out.push(Question {
                session: index,
                rig: (*rig).clone(),
                filter: canonical_filter(&filter),
                lights,
                candidates,
                angle,
                key,
            });
        }
    }
    Ok(out)
}

/// Whether a question's answer is kept under its current key.
pub(crate) fn answered(book: &Book, question: &Question) -> bool {
    book.family_evidence
        .get(&(
            book.imaging[question.session].key.clone(),
            question.filter.clone(),
        ))
        .is_some_and(|row| row.key == question.key)
}

/// The row a question keeps: `(session, filter, key)`.
pub(crate) fn kept(book: &Book, question: &Question) -> (String, String, String) {
    (
        book.imaging[question.session].key.clone(),
        question.filter.clone(),
        question.key.clone(),
    )
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
    let session = &book.imaging[question.session];
    // Each candidate's flats nearest the session in time, pedestal out.
    let representatives: Vec<usize> = question
        .candidates
        .iter()
        .filter_map(|family| {
            book.family_representative(*family, &question.rig, session.start, question.angle)
        })
        .collect();
    if representatives.len() != question.candidates.len() {
        return None;
    }
    let planes: Vec<Option<Plane>> = representatives
        .iter()
        .map(|index| session_plane(pass.dir, &book.sessions[*index].row))
        .collect();
    let lights = &question.lights;
    let picked: Vec<&Light> = if lights.len() > evidence::MAX_LIGHTS {
        (0..evidence::MAX_LIGHTS)
            .map(|i| &lights[i * (lights.len() - 1) / (evidence::MAX_LIGHTS - 1)])
            .collect()
    } else {
        lights.iter().collect()
    };
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
    // A candidate whose response is not on disk is no candidate: an empty
    // plane, never weighed, so nothing wins past it.
    let planes: Vec<Plane> = planes
        .into_iter()
        .map(|plane| plane.unwrap_or_else(|| Plane::filled(0, 0, 1.0)))
        .collect();
    let (width, height) = (binned[0].width, binned[0].height);
    let binned: Vec<Plane> = binned
        .into_iter()
        .filter(|plane| plane.width == width && plane.height == height)
        .collect();
    let meta = crate::commands::import::headers::read_frame_meta(&picked[0].path);
    let pedestal =
        pedestal_for(pedestals, &question.rig, meta.gain, meta.offset, None).unwrap_or(0.0) as f32;
    let bin = book.sessions[representatives[0]].row.bin.unwrap_or(1);
    let picked_choice = match evidence::background(&binned, pedestal, bin) {
        Some(background) => pass.in_pool(|| {
            choice::choose_response(&background, &planes, |a, b| {
                book.measured_alike(&[representatives[a], representatives[b]])
            })
        }),
        None => choice::ResponseChoice::default(),
    };
    let key_of = |position: usize| book.families[question.candidates[position]].key.clone();
    let mut targets: Vec<String> = picked.iter().map(|light| light.target.clone()).collect();
    targets.sort();
    targets.dedup();
    tracing::debug!(
        "family evidence {} {}: winners {:?} of {:?}",
        session.key,
        question.filter,
        picked_choice.winners,
        question.candidates
    );
    // Several winners are alike with each other: the nearest of them to
    // the lights' angle stands for them.
    let chosen = match picked_choice.winners.as_slice() {
        [] => None,
        [only] => Some(*only),
        // A pair the background could not be weighed against leaves
        // nothing to choose among.
        _ if picked_choice.any_unfitted() => None,
        several => {
            let angle = picked
                .iter()
                .find_map(|light| light.angle)
                .or(meta.rotator_position);
            angle.and_then(|angle| {
                several.iter().copied().min_by(|a, b| {
                    let gap = |position: usize| {
                        book.families[question.candidates[position]]
                            .angles
                            .iter()
                            .map(|span| span.gap_to(angle))
                            .fold(f64::INFINITY, f64::min)
                    };
                    gap(*a).total_cmp(&gap(*b))
                })
            })
        }
    };
    Some(FamilyEvidenceRow {
        session_key: session.key.clone(),
        filter: question.filter.clone(),
        key: question.key.clone(),
        candidates: (0..question.candidates.len()).map(key_of).collect(),
        chosen: chosen.map(key_of),
        lights: binned.len(),
        targets,
        pairs: picked_choice
            .pairs
            .iter()
            .map(|pair| FamilyPairRow {
                first: key_of(pair.first),
                second: key_of(pair.second),
                outcome: match pair.outcome {
                    choice::PairOutcome::Alike => "alike",
                    choice::PairOutcome::Unfitted => "unfitted",
                    choice::PairOutcome::Fit(_) => "fit",
                }
                .into(),
                carried: pair.fit().map(|fit| fit.carried),
                spread: pair
                    .fit()
                    .and_then(|fit| fit.spread.is_finite().then_some(fit.spread)),
                verdict: pair.fit().map(|fit| fit.verdict),
            })
            .collect(),
    })
}
