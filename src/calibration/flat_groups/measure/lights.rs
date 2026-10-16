//! Which response family a light's flats come from.
//!
//! A light records the rotator's mechanical angle, and so do its flats. The
//! reading is a good prior but not an identity: a rotator re-homed or a
//! camera re-seated reads the same angle for another position, and two
//! angles far apart can share one light path (see [`super::model`]).
//!
//! - **Angle epochs.** N.I.N.A. syncs the rotator's sky angle from its own
//!   plate solves; PSF Guard's pixel solves give the same angle where they
//!   exist. Sky − mechanical is constant for one optical train and pier
//!   side, and jumps when the rotator is re-homed. Each run of lights with
//!   one offset is an **epoch**. A light shot in one epoch compared with
//!   flats of another is turned by the difference of their offsets.
//! - **Candidates.** The response families of the light's filter and
//!   sensor mode whose measured angle range lies within
//!   [`ANGLE_PRIOR_DEG`] of the light's angle, as recorded or turned into
//!   each flat session's epoch.
//! - **Evidence.** With two or more candidates, the light's imaging
//!   session's own sky background is fitted against each pair of them
//!   (`seiza_calibration::flat_response::choice`), and the family it
//!   follows wins. Nothing standing clear, the nearest angle is taken, and
//!   the light's account says so.
//!
//! Nothing here reads pixels or the catalog: the pass stores what lights
//! said ([`super::rows::FamilyEvidenceRow`]) and the book reads it back.

use super::model::{canonical_filter, Book, ImagingSession};
use super::rows::FamilyEvidenceRow;
use serde::Serialize;
use std::collections::HashMap;

/// How far from the light's angle a response family may lie and still be a
/// candidate for its flats. Wider than the 2° one master's flats are held
/// to, so a rotator that drifted or re-homed still finds its family; the
/// light's own pixels decide between the candidates.
pub const ANGLE_PRIOR_DEG: f64 = 10.0;
/// A change of sky − mechanical offset larger than this between two lights
/// of one rig starts a new epoch. N.I.N.A.'s own re-syncs move it by a
/// degree or two (RedCat61 wanders between 138.6° and 140.6° night to
/// night); re-homes and re-seats by several (C925: 76°, 79°, 87°, 97°).
pub const EPOCH_JUMP_DEG: f64 = 2.5;
/// Bump when equal lights and candidates may choose differently.
pub const FAMILY_EVIDENCE_VERSION: u32 = 1;
/// The largest spread a light's own pixels may decide with when they move
/// its flats away from the family at its recorded angle: half what decides
/// which side of a change it is on.
pub const MOVE_MAX_SPREAD: f64 = 0.15;

/// The signed angle from `b` to `a`, in (−180, 180].
pub fn wrap(difference: f64) -> f64 {
    let d = difference.rem_euclid(360.0);
    if d > 180.0 {
        d - 360.0
    } else {
        d
    }
}

/// A difference of offsets kept modulo a half turn, in (−90, 90]: the sky
/// angle N.I.N.A. records turns by half a turn at a meridian flip, and
/// which pier side does so is the mount's own convention.
pub fn half(difference: f64) -> f64 {
    let d = difference.rem_euclid(180.0);
    if d > 90.0 {
        d - 180.0
    } else {
        d
    }
}

/// One run of lights of a rig with one sky − mechanical offset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Epoch {
    pub start: i64,
    pub end: i64,
    /// Sky − mechanical, degrees, modulo a half turn (see [`half`]).
    pub offset: f64,
    pub lights: usize,
}

/// Each rig's epochs, in time order.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AngleEpochs {
    pub by_rig: HashMap<String, Vec<Epoch>>,
}

impl AngleEpochs {
    /// Epochs from each light's `(rig, time, offset)`: a light whose
    /// offset differs from the one before it by more than
    /// [`EPOCH_JUMP_DEG`] starts a new one.
    pub fn from_samples(mut samples: Vec<(String, i64, f64)>) -> Self {
        samples.retain(|(_, _, offset)| offset.is_finite());
        samples.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut by_rig: HashMap<String, Vec<Epoch>> = HashMap::new();
        let mut run: Vec<f64> = Vec::new();
        let mut current: Option<(String, i64, i64)> = None;
        let close = |by_rig: &mut HashMap<String, Vec<Epoch>>,
                     current: &Option<(String, i64, i64)>,
                     run: &mut Vec<f64>| {
            if let Some((rig, start, end)) = current
                && !run.is_empty()
            {
                // Unwrapped about the run's first, so 359° and 1° average
                // to 0°.
                let first = run[0];
                let mut unwrapped: Vec<f64> = run.iter().map(|o| first + half(o - first)).collect();
                unwrapped.sort_by(f64::total_cmp);
                by_rig.entry(rig.clone()).or_default().push(Epoch {
                    start: *start,
                    end: *end,
                    offset: unwrapped[unwrapped.len() / 2].rem_euclid(180.0),
                    lights: run.len(),
                });
                run.clear();
            }
        };
        for (rig, at, offset) in samples {
            let joins = match (&current, run.last()) {
                (Some((current_rig, _, _)), Some(last)) => {
                    *current_rig == rig && half(offset - last).abs() <= EPOCH_JUMP_DEG
                }
                _ => false,
            };
            if joins {
                if let Some((_, _, end)) = current.as_mut() {
                    *end = at;
                }
            } else {
                close(&mut by_rig, &current, &mut run);
                current = Some((rig, at, at));
            }
            run.push(offset.rem_euclid(180.0));
        }
        close(&mut by_rig, &current, &mut run);
        Self { by_rig }
    }

    /// The offset of a rig at `at`: the epoch holding it, else the nearest
    /// in time.
    pub fn offset_at(&self, rig: &str, at: i64) -> Option<f64> {
        let epochs = self.by_rig.get(rig)?;
        epochs
            .iter()
            .min_by_key(|epoch| {
                if at < epoch.start {
                    epoch.start - at
                } else if at > epoch.end {
                    at - epoch.end
                } else {
                    0
                }
            })
            .map(|epoch| epoch.offset)
    }

    /// How far a light shot at `light_at` turns when compared with flats
    /// shot at `flat_at`: the change of the rig's offset between them, 0
    /// when either is unknown.
    pub fn shift(&self, rig: &str, light_at: i64, flat_at: i64) -> f64 {
        match (self.offset_at(rig, light_at), self.offset_at(rig, flat_at)) {
            (Some(light), Some(flat)) => half(light - flat),
            _ => 0.0,
        }
    }
}

/// What lights said, by imaging session key and canonical filter.
pub type FamilyEvidence = HashMap<(String, String), FamilyEvidenceRow>;

/// How a light's response family was chosen.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum FamilyBasis {
    /// The only candidate, at the light's recorded angle.
    Angle,
    /// The only candidate, within the prior but not at the recorded angle:
    /// reached through the rotator's epochs or the family's measured range.
    Prior,
    /// Its imaging session's own lights follow this family's flats.
    Pixels { lights: usize, targets: Vec<String> },
    /// Nothing stood clear between the candidates: the family at the
    /// light's recorded angle, else the nearest through the rotator's
    /// epochs. `undecided` when the lights were read and could not tell;
    /// otherwise they have not been read yet.
    Nearest { undecided: bool },
}

/// The response family a light's flats come from.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FamilyChoice {
    /// Index into the book's families.
    pub family: usize,
    pub basis: FamilyBasis,
    /// The candidates, nearest first.
    pub candidates: Vec<usize>,
    /// The light's recorded angle.
    pub light_angle: f64,
    /// How far the light's angle turns into the chosen family's nearest
    /// session's epoch: 0 when nothing re-homed.
    pub shift: f64,
}

impl FamilyChoice {
    /// Whether the choice took flats at an angle the light did not record.
    pub fn moved(&self) -> bool {
        !matches!(self.basis, FamilyBasis::Angle)
    }
}

impl Book {
    /// The measured angle range of a family, as one label: "100.1–104.4°",
    /// "72.6° & 82.2°", or "no angle".
    pub fn angle_label(&self, family: usize) -> String {
        let spans = &self.families[family].angles;
        if spans.is_empty() {
            return "no angle".into();
        }
        spans
            .iter()
            .map(|span| span.label())
            .collect::<Vec<_>>()
            .join(" & ")
    }

    /// The response family a flat's session belongs to.
    pub fn family_of_frame(&self, frame_uuid: &str) -> Option<usize> {
        let session = self.session_of_frame(frame_uuid)?;
        self.family_of
            .get(session)
            .copied()
            .filter(|f| *f != usize::MAX)
    }

    /// How far `family` lies from a light at `angle` shot at `at`: the
    /// least gap from any of its sessions, the light's angle turned into
    /// that session's epoch; and the same without turning it.
    fn family_gaps(&self, family: usize, rig: &str, at: Option<i64>, angle: f64) -> (f64, f64) {
        let mut shifted = f64::INFINITY;
        let mut recorded = f64::INFINITY;
        for index in &self.families[family].sessions {
            let session = &self.sessions[*index];
            let Some(flat) = session.row.rotation.filter(|r| r.is_finite()) else {
                continue;
            };
            let shift = at.map_or(0.0, |at| self.epochs.shift(rig, at, session.first));
            shifted = shifted.min(wrap(angle + shift - flat).abs());
            recorded = recorded.min(wrap(angle - flat).abs());
        }
        (shifted, recorded)
    }

    /// The candidate families for a light at `angle`, nearest first, with
    /// how far each lies: among `compatible` (families whose flats pass
    /// every gate but the angle), those with an angle within
    /// [`ANGLE_PRIOR_DEG`], recorded or turned through the epochs.
    pub fn family_candidates(
        &self,
        rig: &str,
        at: Option<i64>,
        angle: f64,
        compatible: &[usize],
    ) -> Vec<(usize, f64)> {
        let mut out: Vec<(usize, f64)> = compatible
            .iter()
            .copied()
            .filter(|family| !self.families[*family].angles.is_empty())
            .filter_map(|family| {
                let (shifted, recorded) = self.family_gaps(family, rig, at, angle);
                (shifted.min(recorded) <= ANGLE_PRIOR_DEG).then_some((family, shifted))
            })
            .collect();
        out.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        out.dedup_by_key(|(family, _)| *family);
        out
    }

    /// The flat session of `family` a light at `angle` shot at `at` is
    /// measured against: the nearest in time among its sessions within
    /// [`ANGLE_PRIOR_DEG`] of the light's angle, recorded or turned into
    /// that session's epoch, else among all of them. A family that is a
    /// candidate through one epoch's session is weighed by that session,
    /// not by one shot where the rotator stood elsewhere.
    pub fn family_representative(
        &self,
        family: usize,
        rig: &str,
        at: i64,
        angle: f64,
    ) -> Option<usize> {
        let near = |index: usize| {
            let session = &self.sessions[index];
            session
                .row
                .rotation
                .filter(|r| r.is_finite())
                .is_some_and(|flat| {
                    let shift = self.epochs.shift(rig, at, session.first);
                    wrap(angle + shift - flat)
                        .abs()
                        .min(wrap(angle - flat).abs())
                        <= ANGLE_PRIOR_DEG
                })
        };
        let sessions = &self.families[family].sessions;
        let within: Vec<usize> = sessions.iter().copied().filter(|i| near(*i)).collect();
        let pool = if within.is_empty() {
            sessions.clone()
        } else {
            within
        };
        pool.into_iter()
            .min_by_key(|index| ((self.sessions[*index].first - at).abs(), *index))
    }

    /// The key a session's family evidence is kept under: the candidates
    /// and the sessions that stand for them. New flats nearer the session,
    /// or a family that joined another, ask the lights again.
    pub fn family_evidence_key(
        &self,
        candidates: &[usize],
        rig: &str,
        at: i64,
        angle: f64,
    ) -> String {
        use sha2::Digest;
        let mut parts: Vec<String> = candidates
            .iter()
            .map(|family| {
                format!(
                    "{}={}",
                    self.families[*family].key,
                    self.family_representative(*family, rig, at, angle)
                        .map_or("-", |i| self.sessions[i].row.response_key.as_str())
                )
            })
            .collect();
        parts.sort();
        let digest = sha2::Sha256::digest(
            format!("{FAMILY_EVIDENCE_VERSION}\u{1f}{}", parts.join(",")).as_bytes(),
        );
        digest.iter().take(12).map(|b| format!("{b:02x}")).collect()
    }

    /// What the lights of `session` said in `filter`, when they were read.
    pub fn family_evidence_for(
        &self,
        session: &ImagingSession,
        filter: &str,
    ) -> Option<&FamilyEvidenceRow> {
        self.family_evidence
            .get(&(session.key.clone(), canonical_filter(filter)))
    }

    /// The response family a light's flats come from, or `None` to match
    /// them by angle as before: the light records no angle, or no family
    /// lies within the prior. `compatible` are the families whose flats
    /// pass every gate but the angle; `tolerance` the rig's rotation
    /// tolerance.
    pub fn choose_family(
        &self,
        rig: &str,
        filter: Option<&str>,
        at: Option<i64>,
        angle: Option<f64>,
        compatible: &[usize],
        tolerance: f64,
    ) -> Option<FamilyChoice> {
        let angle = angle.filter(|a| a.is_finite())?;
        let candidates = self.family_candidates(rig, at, angle, compatible);
        let (by_epoch, _) = *candidates.first()?;
        // Without the lights' word, the family at the angle the light
        // recorded stands, as before families; the epochs only widen the
        // candidates. With none at its recorded angle, the nearest through
        // the epochs.
        let at_recorded = candidates
            .iter()
            .map(|(family, _)| (*family, self.family_gaps(*family, rig, at, angle).1))
            .filter(|(_, recorded)| *recorded <= tolerance)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(family, _)| family);
        let nearest = at_recorded.unwrap_or(by_epoch);
        let shift_to = |family: usize| {
            at.and_then(|at| {
                let session = self.family_representative(family, rig, at, angle)?;
                Some(self.epochs.shift(rig, at, self.sessions[session].first))
            })
            .unwrap_or(0.0)
        };
        let ordered: Vec<usize> = candidates.iter().map(|(family, _)| *family).collect();
        let choice = |family: usize, basis: FamilyBasis| FamilyChoice {
            family,
            basis,
            candidates: ordered.clone(),
            light_angle: angle,
            shift: shift_to(family),
        };
        if candidates.len() == 1 {
            let (_, recorded) = self.family_gaps(nearest, rig, at, angle);
            let at_angle = self.families[nearest]
                .angles
                .iter()
                .any(|span| span.gap_to(angle) <= tolerance)
                || recorded <= tolerance;
            return Some(choice(
                nearest,
                if at_angle {
                    FamilyBasis::Angle
                } else {
                    FamilyBasis::Prior
                },
            ));
        }
        let evidence = at
            .and_then(|at| self.imaging_session(rig, at))
            .zip(filter)
            .and_then(|(session, filter)| self.family_evidence_for(session, filter));
        if let Some(row) = evidence {
            let keys: Vec<&str> = ordered
                .iter()
                .map(|family| self.families[*family].key.as_str())
                .collect();
            let covered = keys
                .iter()
                .all(|key| row.candidates.iter().any(|c| c == key));
            // Away from the family at its recorded angle only on a tight
            // fit: over a nebula a spread near the limit decides a change's
            // side safely, but moving a light's flats to another angle
            // against its own record wants more.
            let tight = |chosen: &str| {
                row.pairs
                    .iter()
                    .filter(|pair| pair.first == chosen || pair.second == chosen)
                    .filter(|pair| {
                        pair.verdict
                            .is_some_and(|v| v != super::rows::Verdict::Undecided)
                    })
                    .all(|pair| pair.spread.is_some_and(|spread| spread <= MOVE_MAX_SPREAD))
            };
            if covered
                && let Some(chosen) = row.chosen.as_deref()
                && let Some(family) = ordered
                    .iter()
                    .copied()
                    .find(|family| self.families[*family].key == chosen)
                && (at_recorded.is_none_or(|recorded| recorded == family) || tight(chosen))
            {
                return Some(choice(
                    family,
                    FamilyBasis::Pixels {
                        lights: row.lights,
                        targets: row.targets.clone(),
                    },
                ));
            }
            return Some(choice(nearest, FamilyBasis::Nearest { undecided: covered }));
        }
        Some(choice(nearest, FamilyBasis::Nearest { undecided: false }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_split_into_epochs_at_re_homes_not_at_syncs() {
        let samples = vec![
            ("rig".to_string(), 0, 76.0),
            ("rig".to_string(), 100, 76.8),
            // A sync of about a degree stays in the epoch.
            ("rig".to_string(), 200, 77.6),
            // A re-home moves it by twenty.
            ("rig".to_string(), 300, 97.0),
            ("rig".to_string(), 400, 96.5),
            ("other".to_string(), 50, 10.0),
        ];
        let epochs = AngleEpochs::from_samples(samples);
        let rig = &epochs.by_rig["rig"];
        assert_eq!(rig.len(), 2, "{rig:?}");
        assert_eq!((rig[0].start, rig[0].end, rig[0].lights), (0, 200, 3));
        assert!((rig[0].offset - 76.8).abs() < 1e-9);
        assert!((epochs.shift("rig", 350, 100) - 20.2).abs() < 0.3);
        assert_eq!(epochs.shift("rig", 50, 150), 0.0);
        // Unknown rig or time: no turn.
        assert_eq!(epochs.shift("nobody", 50, 150), 0.0);
        assert_eq!(epochs.offset_at("other", 10_000), Some(10.0));
    }

    #[test]
    fn a_meridian_flip_is_the_same_epoch() {
        // N.I.N.A.'s sky angle turns half a turn at a flip.
        let epochs = AngleEpochs::from_samples(vec![
            ("rig".to_string(), 0, 76.0),
            ("rig".to_string(), 10, 256.2),
            ("rig".to_string(), 20, 76.1),
        ]);
        assert_eq!(epochs.by_rig["rig"].len(), 1, "{epochs:?}");
    }

    #[test]
    fn offsets_wrap_around_the_circle() {
        let epochs = AngleEpochs::from_samples(vec![
            ("rig".to_string(), 0, 359.6),
            ("rig".to_string(), 10, 0.4),
            ("rig".to_string(), 20, 0.2),
        ]);
        assert_eq!(epochs.by_rig["rig"].len(), 1);
        let offset = epochs.by_rig["rig"][0].offset;
        assert!(half(offset - 0.2).abs() < 1e-9, "{offset}");
    }

    use super::super::model::{imaging_sessions, Limits};
    use super::super::rows::{ComparisonRow, ResponseRow};

    fn row(key: &str, rotation: f64, at: i64) -> ResponseRow {
        ResponseRow {
            response_key: key.into(),
            session_key: format!("s-{key}"),
            rig_uuid: "rig".into(),
            family_key: "OIII".into(),
            night: String::new(),
            filter: Some("OIII".into()),
            rotation: Some(rotation),
            frame_uuids: vec![format!("{key}-1"), format!("{key}-2"), format!("{key}-3")],
            frames: 3,
            captured_first: Some(at),
            captured_last: Some(at + 600),
            level_adu: None,
            pedestal_adu: None,
            noise: None,
            width: None,
            height: None,
            bin: None,
            algorithm_version: 1,
            error: None,
        }
    }

    fn apart(a: &str, b: &str) -> ComparisonRow {
        ComparisonRow {
            earlier_key: a.into(),
            later_key: b.into(),
            broad: 0.003,
            dust: 0.03,
            sigma: 0.0002,
            spots: Vec::new(),
            broad_x: 0.5,
            broad_y: 0.5,
        }
    }

    const DAY: i64 = 86_400;

    /// OIII flats at 82° and 102°, measured apart, and a light session.
    fn book() -> Book {
        let limits = Limits {
            measured: true,
            dust: 0.01,
            broad: 0.02,
            window: DAY,
            rotation: 2.0,
        };
        let imaging = imaging_sessions(
            vec![
                (1, Some("p".into()), 10 * DAY),
                (2, Some("p".into()), 10 * DAY + 600),
            ],
            |_| vec!["rig".into()],
        );
        Book::build(
            vec![row("o82", 82.2, 0), row("o102", 102.0, 600)],
            vec![apart("o82", "o102")],
            vec![],
            imaging,
            |_| limits,
        )
    }

    #[test]
    fn a_lone_family_near_the_light_is_chosen_by_its_angle() {
        let book = book();
        let all: Vec<usize> = (0..book.families.len()).collect();
        let choice = book
            .choose_family("rig", Some("OIII"), Some(10 * DAY), Some(82.0), &all, 2.0)
            .unwrap();
        assert_eq!(book.angle_label(choice.family), "82.2°");
        assert_eq!(choice.basis, FamilyBasis::Angle);
        // Nothing within the prior: matched by angle as before.
        assert!(book
            .choose_family("rig", Some("OIII"), Some(10 * DAY), Some(30.0), &all, 2.0)
            .is_none());
    }

    #[test]
    fn a_re_homed_rotator_reaches_the_family_its_light_path_is_in() {
        let mut book = book();
        // The flats were shot in an epoch whose offset is 20° less than the
        // light's: a light recorded at 82° sits where 102° flats did.
        book.epochs =
            AngleEpochs::from_samples(vec![("rig".into(), 0, 76.0), ("rig".into(), 5 * DAY, 96.0)]);
        let all: Vec<usize> = (0..book.families.len()).collect();
        let choice = book
            .choose_family("rig", Some("OIII"), Some(10 * DAY), Some(82.0), &all, 2.0)
            .unwrap();
        // Both are candidates: the header's and the epoch's. Unread, the
        // family at its recorded angle stands, as before families.
        assert_eq!(choice.candidates.len(), 2);
        assert_eq!(book.angle_label(choice.family), "82.2°");
        assert_eq!(choice.basis, FamilyBasis::Nearest { undecided: false });
        // Recorded at 85°, no family within 2° of it: the epoch's nearest.
        let choice = book
            .choose_family("rig", Some("OIII"), Some(10 * DAY), Some(85.0), &all, 2.0)
            .unwrap();
        assert_eq!(book.angle_label(choice.family), "102.0°");
        assert!((choice.shift - 20.0).abs() < 1e-9);
        assert!(
            super::super::super::describe_family(&super::super::super::FlatFamily::of(
                &book, &choice
            ))
            .unwrap()
            .contains("105.0° in the flats' rotator epoch")
        );

        // Its own pixels move it off its recorded family only on a tight
        // fit.
        let session = book.imaging[0].key.clone();
        let key = |rotation: f64| {
            book.families
                .iter()
                .find(|f| f.rotation == Some(rotation))
                .unwrap()
                .key
                .clone()
        };
        let (k82, k102) = (key(82.2), key(102.0));
        let row = |spread: f64| FamilyEvidenceRow {
            session_key: session.clone(),
            filter: "oiii".into(),
            key: "k".into(),
            candidates: vec![k82.clone(), k102.clone()],
            chosen: Some(k102.clone()),
            lights: 8,
            targets: vec!["Sh2 140".into()],
            pairs: vec![super::super::rows::FamilyPairRow {
                first: k82.clone(),
                second: k102.clone(),
                outcome: "fit".into(),
                carried: Some(1.0),
                spread: Some(spread),
                verdict: Some(super::super::rows::Verdict::After),
            }],
        };
        for (spread, expected) in [(0.21, "82.2°"), (0.1, "102.0°")] {
            book.family_evidence
                .insert((session.clone(), "oiii".into()), row(spread));
            let choice = book
                .choose_family(
                    "rig",
                    Some("OIII"),
                    Some(10 * DAY + 300),
                    Some(82.0),
                    &all,
                    2.0,
                )
                .unwrap();
            assert_eq!(book.angle_label(choice.family), expected, "{spread}");
        }
    }

    #[test]
    fn the_lights_own_pixels_choose_between_candidates() {
        let mut book = book();
        let all: Vec<usize> = (0..book.families.len()).collect();
        // Two candidates within the prior of a light at 92°.
        let candidates: Vec<String> = book
            .family_candidates("rig", Some(10 * DAY), 92.0, &all)
            .iter()
            .map(|(f, _)| book.families[*f].key.clone())
            .collect();
        assert_eq!(candidates.len(), 2);
        let session = book.imaging[0].key.clone();
        let row = |chosen: Option<String>| FamilyEvidenceRow {
            session_key: session.clone(),
            filter: "oiii".into(),
            key: "k".into(),
            candidates: candidates.clone(),
            chosen,
            lights: 8,
            targets: vec!["M 27".into()],
            pairs: Vec::new(),
        };
        let o82 = book
            .families
            .iter()
            .position(|f| f.rotation == Some(82.2))
            .unwrap();
        book.family_evidence.insert(
            (session.clone(), "oiii".into()),
            row(Some(book.families[o82].key.clone())),
        );
        let choice = book
            .choose_family(
                "rig",
                Some("OIII"),
                Some(10 * DAY + 300),
                Some(92.0),
                &all,
                2.0,
            )
            .unwrap();
        assert_eq!(choice.family, o82);
        assert!(matches!(
            choice.basis,
            FamilyBasis::Pixels { lights: 8, .. }
        ));
        // The lights could not tell: the nearest angle, undecided.
        book.family_evidence
            .insert((session.clone(), "oiii".into()), row(None));
        let choice = book
            .choose_family(
                "rig",
                Some("OIII"),
                Some(10 * DAY + 300),
                Some(92.0),
                &all,
                2.0,
            )
            .unwrap();
        assert_eq!(choice.basis, FamilyBasis::Nearest { undecided: true });
    }

    #[test]
    fn a_family_is_weighed_by_its_session_near_the_lights_angle() {
        // One family, measured alike at 82° in two rotator epochs: the
        // later session lies where the rotator stood 20° off the light, the
        // earlier where it stood at the light's angle.
        let limits = Limits {
            measured: true,
            dust: 0.01,
            broad: 0.02,
            window: DAY,
            rotation: 2.0,
        };
        let mut book = Book::build(
            vec![row("early", 82.0, 0), row("late", 82.1, 9 * DAY)],
            vec![],
            vec![],
            vec![],
            |_| limits,
        );
        book.epochs =
            AngleEpochs::from_samples(vec![("rig".into(), 0, 96.0), ("rig".into(), 9 * DAY, 76.0)]);
        let family = 0;
        // A light at 102° on day 10, in the later epoch: through the
        // earlier epoch it sits at 82°; the later session, nearer in time,
        // sits 20° off.
        let chosen = book
            .family_representative(family, "rig", 10 * DAY, 102.0)
            .unwrap();
        assert_eq!(book.sessions[chosen].row.response_key, "early");
        // A light at 82° in the later epoch: the nearest in time.
        let chosen = book
            .family_representative(family, "rig", 10 * DAY, 82.0)
            .unwrap();
        assert_eq!(book.sessions[chosen].row.response_key, "late");
    }
}
