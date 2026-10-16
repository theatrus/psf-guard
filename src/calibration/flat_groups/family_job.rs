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

use super::family_store::Kept;
use super::job::{pedestal_for, read_binned, session_plane, Pass, PedestalTable};
use super::lights::wrap;
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
#[derive(Debug, Clone)]
struct Light {
    path: PathBuf,
    at: i64,
    /// Its mechanical angle from N.I.N.A.'s record, else from its header.
    angle: Option<f64>,
}

/// One group of an imaging session's lights the stage asks: one filter,
/// one target, one recorded angle, with two or more candidate families.
pub(crate) struct Question {
    session: usize,
    rig: String,
    filter: String,
    target: String,
    lights: Vec<Light>,
    candidates: Vec<usize>,
    /// The lights' middle, lowest and highest recorded angles.
    angle: f64,
    low: f64,
    high: f64,
    start: i64,
    end: i64,
    key: String,
    /// Its answer is kept under its current key: nothing to read.
    answered: bool,
}

/// Groups of lights within `tolerance` of each other's angle, around the
/// circle, each in angle order.
fn angle_groups(mut lights: Vec<Light>, tolerance: f64) -> Vec<Vec<Light>> {
    lights.sort_by(|a, b| {
        a.angle
            .unwrap_or(f64::NAN)
            .rem_euclid(360.0)
            .total_cmp(&b.angle.unwrap_or(f64::NAN).rem_euclid(360.0))
            .then(a.at.cmp(&b.at))
    });
    let mut groups: Vec<Vec<Light>> = Vec::new();
    for light in lights {
        let joins = groups.last().and_then(|group| group.last()).is_some_and(|last| {
            matches!((last.angle, light.angle), (Some(a), Some(b)) if wrap(b - a).abs() <= tolerance)
        });
        if joins {
            groups.last_mut().expect("joined").push(light);
        } else {
            groups.push(vec![light]);
        }
    }
    groups
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
    let tolerance = crate::calibration::rules::rules_for_rig(for_rig)
        .rotation_deg
        .min(super::model::ROTATION_CLUSTER_DEG);
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
        let mut by_target: BTreeMap<(String, String), Vec<Light>> = BTreeMap::new();
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
            by_target
                .entry((filter.unwrap_or_default(), target.unwrap_or_default()))
                .or_default()
                .push(Light { path, at, angle });
        }
        let kept = book.family_evidence.get(&session.key);
        for ((filter, target), mut lights) in by_target {
            if lights.len() < evidence::MIN_LIGHTS {
                continue;
            }
            // Lights N.I.N.A. recorded no angle for: their headers' angle.
            if lights.iter().any(|light| light.angle.is_none()) {
                let meta = crate::commands::import::headers::read_frame_meta(&lights[0].path);
                for light in lights.iter_mut().filter(|light| light.angle.is_none()) {
                    light.angle = meta.rotator_position;
                }
            }
            let canonical = canonical_filter(&filter);
            for group in angle_groups(lights, tolerance) {
                let angles: Vec<f64> = group.iter().filter_map(|light| light.angle).collect();
                if group.len() < evidence::MIN_LIGHTS || angles.len() != group.len() {
                    continue;
                }
                let mut sorted = angles.clone();
                sorted.sort_by(f64::total_cmp);
                let (low, high, angle) = (
                    group[0].angle.unwrap_or_default(),
                    group[group.len() - 1].angle.unwrap_or_default(),
                    sorted[sorted.len() / 2],
                );
                let (start, end) = (
                    group.iter().map(|light| light.at).min().unwrap_or_default(),
                    group.iter().map(|light| light.at).max().unwrap_or_default(),
                );
                let place = |row: &FamilyEvidenceRow| {
                    row.filter == canonical
                        && row.target == target
                        && row.angle_key() == format!("{low:.1}")
                };
                // Kept and current: nothing to read, not even a header.
                if let Some(row) = kept
                    .into_iter()
                    .flatten()
                    .find(|row| place(row) && book.family_evidence_current(row, rig, session.start))
                {
                    out.push(Question {
                        session: index,
                        rig: (*rig).clone(),
                        filter: canonical.clone(),
                        target: target.clone(),
                        lights: Vec::new(),
                        candidates: Vec::new(),
                        angle: row.angle,
                        low: row.angle_low,
                        high: row.angle_high,
                        start: row.start,
                        end: row.end,
                        key: row.key.clone(),
                        answered: true,
                    });
                    continue;
                }
                // The first light's header: its sensor, optics and rig.
                let mut meta = crate::commands::import::headers::read_frame_meta(&group[0].path);
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
                for light in &group {
                    let candidates = book.family_candidates(
                        rig,
                        Some(light.at),
                        light.angle.unwrap_or(angle),
                        &compatible,
                    );
                    if candidates.len() >= 2 {
                        asked.extend(candidates.iter().map(|(family, _)| *family));
                    }
                }
                if asked.len() < 2 {
                    continue;
                }
                let candidates: Vec<usize> = asked.into_iter().collect();
                let key = book.family_evidence_key(&candidates, rig, session.start, angle);
                out.push(Question {
                    session: index,
                    rig: (*rig).clone(),
                    filter: canonical.clone(),
                    target: target.clone(),
                    lights: group,
                    candidates,
                    angle,
                    low,
                    high,
                    start,
                    end,
                    key,
                    answered: false,
                });
            }
        }
    }
    Ok(out)
}

/// Whether a question's answer is kept under its current key.
pub(crate) fn answered(question: &Question) -> bool {
    question.answered
}

/// The place a question's row is kept under.
pub(crate) fn kept(book: &Book, question: &Question) -> Kept {
    (
        book.imaging[question.session].key.clone(),
        question.filter.clone(),
        question.target.clone(),
        format!("{:.1}", question.low),
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
    let targets = vec![question.target.clone()];
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
            let angle = Some(question.angle);
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
        target: question.target.clone(),
        angle: question.angle,
        angle_low: question.low,
        angle_high: question.high,
        start: question.start,
        end: question.end,
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
