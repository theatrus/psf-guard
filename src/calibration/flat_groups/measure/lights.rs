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

/// What lights said, by imaging session key.
pub type FamilyEvidence = HashMap<String, Vec<FamilyEvidenceRow>>;

/// How a light's response family was chosen.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum FamilyBasis {
    /// The only candidate, at the light's recorded angle.
    Angle,
    /// Its imaging session's own lights follow this family's flats. `moved`
    /// when that is not the family at its recorded angle; a move stands
    /// only when `corroborated` says what else points there.
    Pixels {
        lights: usize,
        targets: Vec<String>,
        moved: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        corroborated: Option<String>,
    },
    /// Two or more candidates, and its lights could not tell them apart
    /// (`undecided`) or have not been read: the family at its recorded
    /// angle stands.
    Nearest { undecided: bool },
    /// Its lights point to another family (`pointed`) than the one at its
    /// recorded angle, and nothing confirms it: it keeps the flats at its
    /// recorded angle and is flagged.
    Disputed {
        #[serde(skip)]
        pointed: usize,
        lights: usize,
        targets: Vec<String>,
    },
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
    /// Whether the family has flats at the light's recorded angle.
    pub at_recorded: bool,
}

impl FamilyChoice {
    /// Whether the choice took a family with no flats at the angle the
    /// light recorded.
    pub fn moved(&self) -> bool {
        !self.at_recorded
    }

    /// Whether its own pixels chose the family.
    pub fn by_pixels(&self) -> bool {
        matches!(self.basis, FamilyBasis::Pixels { .. })
    }

    /// Whether the light is worth a look for it.
    pub fn flags(&self) -> bool {
        matches!(self.basis, FamilyBasis::Disputed { .. })
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

    /// The key a session's family evidence is kept under: the candidates,
    /// the sessions that stand for them, which pairs of those measured
    /// alike (they are not fitted), and the versions of this and Seiza's
    /// choice. New flats nearer the session, a family that joined another,
    /// or a comparison that changed ask the lights again.
    pub fn family_evidence_key(
        &self,
        candidates: &[usize],
        rig: &str,
        at: i64,
        angle: f64,
    ) -> String {
        use sha2::Digest;
        let mut sorted: Vec<usize> = candidates.to_vec();
        sorted.sort_by(|a, b| self.families[*a].key.cmp(&self.families[*b].key));
        let representatives: Vec<Option<usize>> = sorted
            .iter()
            .map(|family| self.family_representative(*family, rig, at, angle))
            .collect();
        let mut parts: Vec<String> = sorted
            .iter()
            .zip(&representatives)
            .map(|(family, representative)| {
                format!(
                    "{}={}",
                    self.families[*family].key,
                    representative.map_or("-", |i| self.sessions[i].row.response_key.as_str())
                )
            })
            .collect();
        for (i, a) in representatives.iter().enumerate() {
            for b in &representatives[i + 1..] {
                if let (Some(a), Some(b)) = (a, b) {
                    parts.push(format!("alike={}", self.measured_alike(&[*a, *b])));
                }
            }
        }
        let digest = sha2::Sha256::digest(
            format!(
                "{FAMILY_EVIDENCE_VERSION}\u{1f}{}\u{1f}{}",
                super::choice::CHOICE_ALGORITHM_VERSION,
                parts.join(",")
            )
            .as_bytes(),
        );
        digest.iter().take(12).map(|b| format!("{b:02x}")).collect()
    }

    /// Whether a row's key is still the one its candidates, their sessions
    /// and comparisons give now: a row of another state is not read.
    pub fn family_evidence_current(&self, row: &FamilyEvidenceRow, rig: &str, start: i64) -> bool {
        let candidates: Option<Vec<usize>> = row
            .candidates
            .iter()
            .map(|key| self.families.iter().position(|family| family.key == *key))
            .collect();
        candidates.is_some_and(|candidates| {
            self.family_evidence_key(&candidates, rig, start, row.angle) == row.key
        })
    }

    /// What the lights of `session` in `filter` near `at` and `angle` said,
    /// when they were read and the answer is current: the row of their
    /// target and angle.
    pub fn family_evidence_for(
        &self,
        rig: &str,
        session: &ImagingSession,
        filter: &str,
        at: i64,
        angle: f64,
        tolerance: f64,
    ) -> Option<&FamilyEvidenceRow> {
        let filter = canonical_filter(filter);
        // A light's header time and its catalog time differ by up to an
        // exposure.
        const SLACK: i64 = 900;
        self.family_evidence
            .get(&session.key)?
            .iter()
            .filter(|row| row.filter == filter)
            .filter(|row| row.start - SLACK <= at && at <= row.end + SLACK)
            .filter(|row| {
                wrap(angle - row.angle_low) >= -tolerance
                    && wrap(angle - row.angle_high) <= tolerance
            })
            .filter(|row| self.family_evidence_current(row, rig, session.start))
            .min_by(|a, b| {
                wrap(angle - a.angle)
                    .abs()
                    .total_cmp(&wrap(angle - b.angle).abs())
            })
    }

    /// Whether `session` of `family` measured alike with one of the
    /// family's sessions at `angle`, within the rig's `tolerance`: then its
    /// flats, at another angle, describe the light path of a light at
    /// `angle` by measurement.
    pub fn alike_with_angle(
        &self,
        family: usize,
        session: usize,
        angle: f64,
        tolerance: f64,
    ) -> bool {
        self.families[family].sessions.iter().any(|other| {
            *other != session
                && self.sessions[*other]
                    .row
                    .rotation
                    .filter(|r| r.is_finite())
                    .is_some_and(|flat| wrap(angle - flat).abs() <= tolerance)
                && self.measured_alike(&[session, *other])
        })
    }

    /// The families a light at `angle` has flats of at that angle, within
    /// the rig's `tolerance`. An alike range never counts here: it hands a
    /// light flats only through `family_admits`, from its own side.
    pub(crate) fn at_recorded(&self, families: &[usize], angle: f64, tolerance: f64) -> Vec<usize> {
        families
            .iter()
            .copied()
            .filter(|family| {
                self.families[*family].sessions.iter().any(|index| {
                    self.sessions[*index]
                        .row
                        .rotation
                        .filter(|r| r.is_finite())
                        .is_some_and(|flat| wrap(angle - flat).abs() <= tolerance)
                })
            })
            .collect()
    }

    /// What besides a light's pixels points to `family`, when they move it
    /// off the family at its recorded angle: the rotator's epochs turning
    /// its angle onto the family, or the lights of another filter of the
    /// same imaging session moved onto a family at the same angles.
    fn corroboration(
        &self,
        rig: &str,
        session: &ImagingSession,
        row: &FamilyEvidenceRow,
        family: usize,
        (at, angle): (i64, f64),
        tolerance: f64,
    ) -> Option<String> {
        let (shifted, _) = self.family_gaps(family, rig, Some(at), angle);
        if shifted <= tolerance {
            return Some("the rotator's epochs".into());
        }
        let spans = &self.families[family].angles;
        let same_angles = |other: usize| {
            self.families[other].angles.iter().any(|a| {
                spans
                    .iter()
                    .any(|b| a.gap_to(b.low) <= tolerance || b.gap_to(a.low) <= tolerance)
            })
        };
        self.family_evidence
            .get(&session.key)?
            .iter()
            .filter(|other| other.filter != row.filter)
            .filter(|other| self.family_evidence_current(other, rig, session.start))
            .find_map(|other| {
                let chosen = other.chosen.as_deref()?;
                let index = self.families.iter().position(|f| f.key == chosen)?;
                // Moved too: no flat of its choice at its lights' angle.
                let moved = self
                    .at_recorded(&[index], other.angle, tolerance)
                    .is_empty();
                // Named as its flats name the filter: "its Ha lights".
                let filter = self.families[index]
                    .filter
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or(&other.filter);
                (moved && same_angles(index)).then(|| format!("its {filter} lights"))
            })
    }

    /// The response family a light's flats come from, or `None` to match
    /// them by angle as before: the light records no angle, no family lies
    /// within the prior, or none has flats at its recorded angle and its
    /// own pixels did not choose one.
    ///
    /// Header and epoch data only widen the candidates: without the
    /// lights' word, the family at the light's recorded angle stands. The
    /// lights' word confirms that family freely, but moves the light to
    /// another only when something else points there too
    /// ([`Self::corroboration`]); otherwise the light keeps its recorded
    /// family and is flagged (`Disputed`). `compatible` are the families
    /// whose flats pass every gate but the angle; `tolerance` the rig's
    /// rotation tolerance.
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
        if candidates.is_empty() {
            return None;
        }
        let ordered: Vec<usize> = candidates.iter().map(|(family, _)| *family).collect();
        let at_recorded = self
            .at_recorded(&ordered, angle, tolerance)
            .into_iter()
            .min_by(|a, b| {
                let gap = |f: usize| self.family_gaps(f, rig, at, angle).1;
                gap(*a).total_cmp(&gap(*b))
            });
        let shift_to = |family: usize| {
            at.and_then(|at| {
                let session = self.family_representative(family, rig, at, angle)?;
                Some(self.epochs.shift(rig, at, self.sessions[session].first))
            })
            .unwrap_or(0.0)
        };
        let choice = |family: usize, basis: FamilyBasis| FamilyChoice {
            family,
            basis,
            candidates: ordered.clone(),
            light_angle: angle,
            shift: shift_to(family),
            at_recorded: at_recorded == Some(family),
        };
        let session = at.and_then(|at| self.imaging_session(rig, at));
        let row = session
            .zip(at)
            .zip(filter)
            .and_then(|((session, at), filter)| {
                self.family_evidence_for(rig, session, filter, at, angle, tolerance)
            });
        let covered = row.is_some_and(|row| {
            ordered.iter().all(|family| {
                row.candidates
                    .iter()
                    .any(|key| *key == self.families[*family].key)
            })
        });
        if let (Some(row), Some(session), Some(at), true) = (row, session, at, covered) {
            let chosen = row.chosen.as_deref().and_then(|key| {
                ordered
                    .iter()
                    .copied()
                    .find(|f| self.families[*f].key == key)
            });
            match chosen {
                Some(family) if at_recorded == Some(family) => {
                    return Some(choice(
                        family,
                        FamilyBasis::Pixels {
                            lights: row.lights,
                            targets: row.targets.clone(),
                            moved: false,
                            corroborated: None,
                        },
                    ));
                }
                Some(family) => {
                    if let Some(by) =
                        self.corroboration(rig, session, row, family, (at, angle), tolerance)
                    {
                        return Some(choice(
                            family,
                            FamilyBasis::Pixels {
                                lights: row.lights,
                                targets: row.targets.clone(),
                                moved: true,
                                corroborated: Some(by),
                            },
                        ));
                    }
                    let disputed = FamilyBasis::Disputed {
                        pointed: family,
                        lights: row.lights,
                        targets: row.targets.clone(),
                    };
                    return Some(choice(at_recorded.unwrap_or(family), disputed));
                }
                None => {
                    return at_recorded
                        .map(|family| choice(family, FamilyBasis::Nearest { undecided: true }));
                }
            }
        }
        let family = at_recorded?;
        Some(choice(
            family,
            if candidates.len() == 1 {
                FamilyBasis::Angle
            } else {
                FamilyBasis::Nearest { undecided: false }
            },
        ))
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
            unread: Vec::new(),
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

    /// The epochs of a light shot `shift` degrees off the flats' offset.
    fn turned(shift: f64) -> AngleEpochs {
        AngleEpochs::from_samples(vec![
            ("rig".into(), 0, 76.0),
            ("rig".into(), 5 * DAY, 76.0 + shift),
        ])
    }

    #[test]
    fn a_re_homed_rotator_widens_the_candidates_but_moves_nothing_alone() {
        let mut book = book();
        // A light recorded at 82° sits where 102° flats did, by the epochs.
        book.epochs = turned(20.0);
        let all: Vec<usize> = (0..book.families.len()).collect();
        let choice = book
            .choose_family("rig", Some("OIII"), Some(10 * DAY), Some(82.0), &all, 2.0)
            .unwrap();
        // Both are candidates, and unread the family at its recorded angle
        // stands.
        assert_eq!(choice.candidates.len(), 2);
        assert_eq!(book.angle_label(choice.family), "82.2°");
        assert_eq!(choice.basis, FamilyBasis::Nearest { undecided: false });
        assert!(choice.at_recorded && !choice.moved());
        // Recorded at 85°, no family at its angle: the epochs alone hand it
        // nothing, and its flats are matched by angle as before.
        assert!(book
            .choose_family("rig", Some("OIII"), Some(10 * DAY), Some(85.0), &all, 2.0)
            .is_none());
    }

    /// What the lights of the book's imaging session at `angle` said:
    /// the family at `chosen` degrees, under the key the book gives their
    /// candidates now.
    fn said(book: &Book, angle: f64, chosen: f64) -> FamilyEvidenceRow {
        let session = &book.imaging[0];
        let candidates: Vec<usize> = (0..book.families.len()).collect();
        FamilyEvidenceRow {
            session_key: session.key.clone(),
            filter: "oiii".into(),
            target: "M 27".into(),
            angle,
            angle_low: angle - 0.1,
            angle_high: angle + 0.1,
            start: session.start,
            end: session.end,
            key: book.family_evidence_key(&candidates, "rig", session.start, angle),
            candidates: book.families.iter().map(|f| f.key.clone()).collect(),
            chosen: book
                .families
                .iter()
                .find(|f| f.rotation == Some(chosen))
                .map(|f| f.key.clone()),
            lights: 8,
            targets: vec!["M 27".into()],
            pairs: Vec::new(),
        }
    }

    fn with(mut book: Book, row: FamilyEvidenceRow) -> Book {
        book.family_evidence
            .insert(row.session_key.clone(), vec![row]);
        book
    }

    // Regression from the review of bc5d5286 (Medium, Q3/Q4): `at_recorded` now counts a family
    // whose alike range holds the light though none of its flats lie at
    // the light's angle. A pixel choice of it is then "confirmed" with no
    // corroboration, and `by_pixels()` makes `family_admits` admit every
    // flat of the family, at any angle and from either side of a change,
    // bypassing the range's own rules. Before this commit it was Disputed.
    #[test]
    fn review_repro_a_range_alone_does_not_confirm_a_pixel_move() {
        let limits = Limits {
            measured: true,
            dust: 0.01,
            broad: 0.02,
            window: DAY,
            rotation: 2.0,
        };
        let on = |key: &str, rotation: f64, day: i64| ResponseRow {
            night: format!("2026-09-{:02}", day + 1),
            ..row(key, rotation, day * DAY)
        };
        let alike = |a: &str, b: &str| ComparisonRow {
            dust: 0.002,
            broad: 0.002,
            ..apart(a, b)
        };
        let imaging = imaging_sessions(
            vec![
                (1, Some("p".into()), 10 * DAY),
                (2, Some("p".into()), 10 * DAY + 600),
            ],
            |_| vec!["rig".into()],
        );
        let book = Book::build(
            vec![
                on("f90", 90.0, 0),
                on("f110", 110.0, 1),
                on("f90b", 90.0, 2),
                on("e95", 95.0, 3),
                on("f110b", 110.0, 4),
            ],
            vec![
                alike("f90", "f110"),
                alike("f110", "f90b"),
                // A second 110° session, so the range stands under the
                // two-sessions-per-end rule.
                alike("f90b", "f110b"),
                apart("f90b", "e95"),
                apart("f110", "e95"),
            ],
            vec![],
            imaging,
            |_| limits,
        );
        assert_eq!(book.families.len(), 2, "{:?}", book.families);
        let f = book
            .families
            .iter()
            .position(|family| {
                family
                    .sessions
                    .iter()
                    .any(|s| book.sessions[*s].row.response_key == "f90")
            })
            .unwrap();
        assert_eq!(book.families[f].alike_ranges.len(), 1);
        let mut row = said(&book, 100.0, 95.0);
        row.chosen = Some(book.families[f].key.clone());
        let book = with(book, row);
        let all: Vec<usize> = (0..book.families.len()).collect();
        let choice = book
            .choose_family(
                "rig",
                Some("OIII"),
                Some(10 * DAY + 300),
                Some(100.0),
                &all,
                2.0,
            )
            .unwrap();
        assert_eq!(choice.family, f);
        assert!(!choice.by_pixels(), "{choice:?}");
    }

    #[test]
    fn the_lights_own_pixels_confirm_the_recorded_family_freely() {
        let mut book = book();
        book.epochs = turned(20.0);
        let row = said(&book, 82.0, 82.2);
        let book = with(book, row);
        let all: Vec<usize> = (0..book.families.len()).collect();
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
        assert_eq!(book.angle_label(choice.family), "82.2°");
        assert!(matches!(
            choice.basis,
            FamilyBasis::Pixels {
                moved: false,
                lights: 8,
                ..
            }
        ));
    }

    #[test]
    fn the_lights_own_pixels_move_a_light_only_when_something_else_agrees() {
        let all: Vec<usize> = (0..2).collect();
        let at = Some(10 * DAY + 300);
        // Recorded at 92°: 82.2° and 102.0° both lie within the prior, and
        // neither at its angle. Its lights point to 82.2°.
        let alone = with(book(), said(&book(), 92.0, 82.2));
        let choice = alone
            .choose_family("rig", Some("OIII"), at, Some(92.0), &all, 2.0)
            .unwrap();
        // Nothing else agrees: the move is a Check, and no family serves.
        assert!(
            matches!(choice.basis, FamilyBasis::Disputed { .. }),
            "{choice:?}"
        );
        assert!(choice.flags() && choice.moved() && !choice.by_pixels());

        // The rotator's epochs put the light at 82.2°: the move stands.
        let mut turned_book = book();
        turned_book.epochs = turned(-9.8);
        let row = said(&turned_book, 92.0, 82.2);
        let turned_book = with(turned_book, row);
        let choice = turned_book
            .choose_family("rig", Some("OIII"), at, Some(92.0), &all, 2.0)
            .unwrap();
        assert!(
            matches!(
                &choice.basis,
                FamilyBasis::Pixels { moved: true, corroborated: Some(by), .. }
                    if by == "the rotator's epochs"
            ),
            "{choice:?}"
        );

        // An answer of another state (a candidate's flats changed) is not
        // read: unread, and with no family at its angle, nothing serves.
        let mut stale = said(&book(), 92.0, 82.2);
        stale.key = "an older key".into();
        let stale_book = with(book(), stale);
        assert!(stale_book
            .choose_family("rig", Some("OIII"), at, Some(92.0), &all, 2.0)
            .is_none());
    }

    #[test]
    fn another_filter_moved_to_the_same_angles_corroborates_a_move() {
        // OIII and Hα flats at 82° and 102°, apart; both filters' lights of
        // one session, recorded at 92°, point to 82°.
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
        let ha = |key: &str, rotation: f64, at: i64| ResponseRow {
            family_key: "HA".into(),
            filter: Some("Ha".into()),
            ..row(key, rotation, at)
        };
        let mut book = Book::build(
            vec![
                row("o82", 82.2, 0),
                row("o102", 102.0, 600),
                ha("h82", 82.1, 900),
                ha("h102", 102.1, 1200),
            ],
            vec![apart("o82", "o102"), apart("h82", "h102")],
            vec![],
            imaging,
            |_| limits,
        );
        let session = book.imaging[0].clone();
        let of = |filter: &str| -> Vec<usize> {
            (0..book.families.len())
                .filter(|f| book.families[*f].filter.as_deref() == Some(filter))
                .collect()
        };
        let (oiii, ha_families) = (of("OIII"), of("Ha"));
        let at82 = |families: &[usize]| {
            families
                .iter()
                .copied()
                .find(|f| book.families[*f].rotation.is_some_and(|r| r < 90.0))
                .unwrap()
        };
        let row = |filter: &str, families: &[usize]| FamilyEvidenceRow {
            session_key: session.key.clone(),
            filter: filter.into(),
            target: "M 27".into(),
            angle: 92.0,
            angle_low: 91.9,
            angle_high: 92.1,
            start: session.start,
            end: session.end,
            key: book.family_evidence_key(families, "rig", session.start, 92.0),
            candidates: families
                .iter()
                .map(|f| book.families[*f].key.clone())
                .collect(),
            chosen: Some(book.families[at82(families)].key.clone()),
            lights: 8,
            targets: vec!["M 27".into()],
            pairs: Vec::new(),
        };
        let one = vec![row("oiii", &oiii)];
        let both = vec![row("oiii", &oiii), row("ha", &ha_families)];
        let at = Some(10 * DAY + 300);
        book.family_evidence.insert(session.key.clone(), one);
        let choice = book
            .choose_family("rig", Some("OIII"), at, Some(92.0), &oiii, 2.0)
            .unwrap();
        assert!(
            matches!(choice.basis, FamilyBasis::Disputed { .. }),
            "{choice:?}"
        );
        book.family_evidence.insert(session.key.clone(), both);
        let choice = book
            .choose_family("rig", Some("OIII"), at, Some(92.0), &oiii, 2.0)
            .unwrap();
        assert!(
            matches!(
                &choice.basis,
                FamilyBasis::Pixels { moved: true, corroborated: Some(by), .. }
                    if by == "its Ha lights"
            ),
            "{choice:?}"
        );
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
