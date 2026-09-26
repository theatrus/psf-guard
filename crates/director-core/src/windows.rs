//! Interval composition only. Hosts supply visibility and transit calculations
//! from an astronomy implementation; this module does not calculate sky positions.

use serde::{Deserialize, Serialize};

const MAX_WINDOWS: usize = 128;
const MAX_TRANSITS: usize = 64;

/// Half-open interval in Unix milliseconds. An operation may finish at `end_ms`.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Interval {
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Effective local rig restriction after profile/legacy inheritance is resolved.
/// This is not the project's near-transit imaging preference or a flip command.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeridianExclusion {
    pub before_ms: u64,
    pub after_ms: u64,
}

/// Complete, sorted transit results for the stated search interval. An empty
/// result is known absence; `None` at the caller is unknown, not absence.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TransitCoverage {
    pub searched: Interval,
    pub transits_ms: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowError {
    InvalidWindows,
    InvalidTransits,
    IncompleteTransitCoverage,
    MeridianTimeOverflow,
}

fn valid(window: Interval) -> bool {
    window.start_ms < window.end_ms
}

fn normalize(windows: &[Interval]) -> Result<Vec<Interval>, WindowError> {
    if windows.len() > MAX_WINDOWS || windows.iter().any(|w| !valid(*w)) {
        return Err(WindowError::InvalidWindows);
    }
    let mut sorted = windows.to_vec();
    sorted.sort_by_key(|w| (w.start_ms, w.end_ms));
    let mut result: Vec<Interval> = Vec::new();
    for window in sorted {
        if let Some(previous) = result.last_mut()
            && window.start_ms <= previous.end_ms
        {
            previous.end_ms = previous.end_ms.max(window.end_ms);
        } else {
            result.push(window);
        }
    }
    Ok(result)
}

/// Intersect eligibility with assignment validity, then remove every meridian
/// exclusion. Gaps in the input union are never bridged. No mutable host state
/// is read or changed. Transit coverage must include transits just outside the
/// assignment whose before/after margins can still overlap its valid lifetime.
pub fn observing_windows(
    eligibility: &[Interval],
    assignment: Interval,
    policy: MeridianExclusion,
    transits: Option<&TransitCoverage>,
) -> Result<Vec<Interval>, WindowError> {
    if !valid(assignment) {
        return Err(WindowError::InvalidWindows);
    }
    let mut windows: Vec<_> = normalize(eligibility)?
        .into_iter()
        .filter_map(|window| {
            let clipped = Interval {
                start_ms: window.start_ms.max(assignment.start_ms),
                end_ms: window.end_ms.min(assignment.end_ms),
            };
            valid(clipped).then_some(clipped)
        })
        .collect();
    if policy.before_ms == 0 && policy.after_ms == 0 {
        return Ok(windows);
    }
    let coverage = transits.ok_or(WindowError::IncompleteTransitCoverage)?;
    if !valid(coverage.searched)
        || coverage.transits_ms.len() > MAX_TRANSITS
        || coverage
            .transits_ms
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || coverage
            .transits_ms
            .iter()
            .any(|t| *t < coverage.searched.start_ms || *t >= coverage.searched.end_ms)
    {
        return Err(WindowError::InvalidTransits);
    }
    let required_start = assignment.start_ms.saturating_sub(policy.after_ms);
    let required_end = assignment
        .end_ms
        .checked_add(policy.before_ms)
        .ok_or(WindowError::MeridianTimeOverflow)?;
    if coverage.searched.start_ms > required_start || coverage.searched.end_ms < required_end {
        return Err(WindowError::IncompleteTransitCoverage);
    }
    for transit in &coverage.transits_ms {
        let blocked = Interval {
            start_ms: transit.saturating_sub(policy.before_ms),
            end_ms: transit
                .checked_add(policy.after_ms)
                .ok_or(WindowError::MeridianTimeOverflow)?,
        };
        // Subtract rather than collapsing to one before/after span: an intervening
        // obstruction remains a gap when a target becomes available again.
        windows = windows
            .into_iter()
            .flat_map(|window| {
                let before = Interval {
                    start_ms: window.start_ms,
                    end_ms: window.end_ms.min(blocked.start_ms),
                };
                let after = Interval {
                    start_ms: window.start_ms.max(blocked.end_ms),
                    end_ms: window.end_ms,
                };
                [before, after].into_iter().filter(|w| valid(*w))
            })
            .collect();
    }
    Ok(windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(start_ms: u64, end_ms: u64) -> Interval {
        Interval { start_ms, end_ms }
    }

    #[test]
    fn asymmetric_exclusion_preserves_horizon_gaps() {
        let coverage = TransitCoverage {
            searched: window(0, 400),
            transits_ms: vec![100],
        };
        let result = observing_windows(
            &[window(10, 120), window(150, 250)],
            window(0, 250),
            MeridianExclusion {
                before_ms: 60,
                after_ms: 10,
            },
            Some(&coverage),
        )
        .unwrap();
        assert_eq!(
            result,
            vec![window(10, 40), window(110, 120), window(150, 250)]
        );
    }

    #[test]
    fn before_only_matches_local_ts_clipper_examples() {
        // Relative-minute cases from the local TS MeridianAvoidanceClipperTest:
        // selecting the first interval that fits a 30-minute session must agree.
        let minute = 60_000;
        let at = |relative: i64| u64::try_from(relative + 180).unwrap() * minute;
        let coverage = TransitCoverage {
            searched: window(0, at(500)),
            transits_ms: vec![at(0)],
        };
        for (start, end, expected_start, expected_end) in [
            (-120, -90, -120, -90),
            (-120, -60, -120, -60),
            (-120, 120, -120, -60),
            (-90, 120, -90, -60),
            (-70, 120, 0, 120),
            (-60, 120, 0, 120),
            (-30, 120, 0, 120),
            (0, 120, 0, 120),
            (30, 120, 30, 120),
        ] {
            let span = window(at(start), at(end));
            let result = observing_windows(
                &[span],
                span,
                MeridianExclusion {
                    before_ms: 60 * minute,
                    after_ms: 0,
                },
                Some(&coverage),
            )
            .unwrap();
            assert_eq!(
                result
                    .into_iter()
                    .find(|w| w.end_ms - w.start_ms >= 30 * minute),
                Some(window(at(expected_start), at(expected_end)))
            );
        }
    }

    #[test]
    fn before_margin_can_cross_epoch_without_wrapping() {
        let coverage = TransitCoverage {
            searched: window(0, 100),
            transits_ms: vec![5],
        };
        assert_eq!(
            observing_windows(
                &[window(0, 50)],
                window(0, 50),
                MeridianExclusion {
                    before_ms: 10,
                    after_ms: 2
                },
                Some(&coverage)
            )
            .unwrap(),
            vec![window(7, 50)]
        );
    }

    #[test]
    fn subtract_matches_discrete_membership_exhaustively() {
        for before in 0..6 {
            for after in 0..6 {
                for transit in 6..14 {
                    let coverage = TransitCoverage {
                        searched: window(0, 30),
                        transits_ms: vec![transit],
                    };
                    let windows = observing_windows(
                        &[window(2, 8), window(10, 17)],
                        window(3, 16),
                        MeridianExclusion {
                            before_ms: before,
                            after_ms: after,
                        },
                        Some(&coverage),
                    )
                    .unwrap();
                    for time in 0..25 {
                        let expected = (3..8).contains(&time) || (10..16).contains(&time);
                        let blocked = (transit - before..transit + after).contains(&time);
                        assert_eq!(
                            windows
                                .iter()
                                .any(|w| w.start_ms <= time && time < w.end_ms),
                            expected && !blocked
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn overlapping_exclusions_do_not_duplicate_or_reopen_windows() {
        let coverage = TransitCoverage {
            searched: window(0, 400),
            transits_ms: vec![100, 120],
        };
        let windows = observing_windows(
            &[window(0, 250)],
            window(0, 250),
            MeridianExclusion {
                before_ms: 30,
                after_ms: 30,
            },
            Some(&coverage),
        )
        .unwrap();
        assert_eq!(windows, vec![window(0, 70), window(150, 250)]);
    }

    #[test]
    fn normalize_overlaps_and_touching_intervals_but_not_gaps() {
        assert_eq!(
            normalize(&[window(10, 20), window(0, 5), window(3, 10), window(22, 30)]).unwrap(),
            vec![window(0, 20), window(22, 30)]
        );
        assert_eq!(normalize(&[window(5, 5)]), Err(WindowError::InvalidWindows));
        assert_eq!(
            normalize(&vec![window(0, 1); MAX_WINDOWS + 1]),
            Err(WindowError::InvalidWindows)
        );
    }

    #[test]
    fn require_transits_outside_the_assignment_that_can_overlap_it() {
        let policy = MeridianExclusion {
            before_ms: 20,
            after_ms: 10,
        };
        let mut coverage = TransitCoverage {
            searched: window(100, 200),
            transits_ms: vec![],
        };
        assert_eq!(
            observing_windows(
                &[window(100, 200)],
                window(100, 200),
                policy,
                Some(&coverage)
            ),
            Err(WindowError::IncompleteTransitCoverage)
        );
        coverage.searched = window(90, 220);
        coverage.transits_ms = vec![95, 210];
        assert_eq!(
            observing_windows(
                &[window(100, 200)],
                window(100, 200),
                policy,
                Some(&coverage)
            )
            .unwrap(),
            vec![window(105, 190)]
        );
    }

    #[test]
    fn missing_data_overflow_and_invalid_transits_fail_closed() {
        let policy = MeridianExclusion {
            before_ms: 10,
            after_ms: 5,
        };
        assert_eq!(
            observing_windows(&[], window(0, 100), policy, None),
            Err(WindowError::IncompleteTransitCoverage)
        );
        let mut coverage = TransitCoverage {
            searched: window(0, u64::MAX),
            transits_ms: vec![],
        };
        assert_eq!(
            observing_windows(&[], window(0, u64::MAX), policy, Some(&coverage)),
            Err(WindowError::MeridianTimeOverflow)
        );
        for transits in [
            vec![50, 50],
            vec![60, 50],
            vec![u64::MAX],
            vec![1; MAX_TRANSITS + 1],
        ] {
            coverage.transits_ms = transits;
            assert_eq!(
                observing_windows(&[], window(0, 100), policy, Some(&coverage)),
                Err(WindowError::InvalidTransits)
            );
        }
    }
}
