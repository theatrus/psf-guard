//! A night's overrides on a Director rig, in the terms a program already
//! carries: windows its goals lose, and the goals' priorities for the night.
//! The host resolves what a person named (a plan, a target, an exposure
//! plan) to goal ids; this only narrows. Applying a narrowing takes time
//! from goals' windows and never adds any, so an override cannot widen
//! eligibility, extend an allocation or relax a limit: the rig's geometry
//! still intersects what is left with its own windows.

use crate::windows::Interval;
use crate::Goal;
use std::collections::BTreeMap;

/// What one night's overrides take from a program's goals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Narrowing {
    /// The night, noon to noon. A hold runs to its end.
    pub night: Interval,
    /// The windows each goal, by id, loses.
    pub lost: BTreeMap<String, Vec<Interval>>,
    /// Each goal's priority under the night's own plan order, in place of
    /// the program's. `None` keeps the program's.
    pub priorities: Option<BTreeMap<String, u32>>,
    /// From this instant to the night's end every goal loses its windows.
    pub hold_from_ms: Option<u64>,
}

impl Narrowing {
    /// No windows lost, no hold, and the program's priorities.
    pub fn none(night: Interval) -> Self {
        Self {
            night,
            lost: BTreeMap::new(),
            priorities: None,
            hold_from_ms: None,
        }
    }

    /// Take the night's lost windows and any hold from each goal's
    /// eligibility, and give each goal its priority for the night.
    pub fn apply(&self, goals: &mut [Goal]) {
        let held = self.held();
        for goal in goals {
            let mut lost: Vec<Interval> = self.lost.get(&goal.id).cloned().unwrap_or_default();
            lost.extend(held);
            if !lost.is_empty() {
                goal.eligible_windows = subtract(&goal.eligible_windows, &lost);
            }
            if let Some(priority) = self.priorities.as_ref().and_then(|p| p.get(&goal.id)) {
                goal.priority = *priority;
            }
        }
    }

    /// The stretch a hold takes, from its start to the night's end.
    pub fn held(&self) -> Option<Interval> {
        let from = self.hold_from_ms?.max(self.night.start_ms);
        (from < self.night.end_ms).then_some(Interval {
            start_ms: from,
            end_ms: self.night.end_ms,
        })
    }
}

/// `windows` less every instant in `lost`. Either may be unsorted or
/// overlap; the result is sorted, apart, and holds no empty window.
pub fn subtract(windows: &[Interval], lost: &[Interval]) -> Vec<Interval> {
    let mut lost: Vec<Interval> = lost
        .iter()
        .copied()
        .filter(|w| w.start_ms < w.end_ms)
        .collect();
    lost.sort_by_key(|w| w.start_ms);
    let mut kept = Vec::new();
    let mut sorted: Vec<Interval> = windows
        .iter()
        .copied()
        .filter(|w| w.start_ms < w.end_ms)
        .collect();
    sorted.sort_by_key(|w| w.start_ms);
    for window in sorted {
        let mut start = window.start_ms;
        for cut in &lost {
            if cut.end_ms <= start || cut.start_ms >= window.end_ms {
                continue;
            }
            if cut.start_ms > start {
                kept.push(Interval {
                    start_ms: start,
                    end_ms: cut.start_ms,
                });
            }
            start = start.max(cut.end_ms);
            if start >= window.end_ms {
                break;
            }
        }
        if start < window.end_ms {
            kept.push(Interval {
                start_ms: start,
                end_ms: window.end_ms,
            });
        }
    }
    // Overlapping inputs leave overlapping pieces; merge them.
    let mut merged: Vec<Interval> = Vec::with_capacity(kept.len());
    for window in kept {
        match merged.last_mut() {
            Some(last) if window.start_ms <= last.end_ms => {
                last.end_ms = last.end_ms.max(window.end_ms);
            }
            _ => merged.push(window),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start_ms: u64, end_ms: u64) -> Interval {
        Interval { start_ms, end_ms }
    }

    #[test]
    fn subtracting_cuts_holes_and_ends_and_never_adds() {
        assert_eq!(subtract(&[w(0, 100)], &[w(20, 30)]), [w(0, 20), w(30, 100)]);
        assert_eq!(subtract(&[w(0, 100)], &[w(0, 30), w(90, 200)]), [w(30, 90)]);
        assert_eq!(subtract(&[w(0, 100)], &[w(0, 100)]), []);
        assert_eq!(subtract(&[w(10, 20)], &[w(30, 40)]), [w(10, 20)]);
        // Unsorted, overlapping inputs.
        assert_eq!(
            subtract(&[w(50, 80), w(0, 60)], &[w(40, 55), w(45, 50)]),
            [w(0, 40), w(55, 80)]
        );
        assert_eq!(subtract(&[], &[w(0, 10)]), []);
    }

    #[test]
    fn a_hold_takes_every_goal_to_the_nights_end() {
        let narrowing = Narrowing {
            hold_from_ms: Some(70),
            ..Narrowing::none(w(0, 100))
        };
        assert_eq!(narrowing.held(), Some(w(70, 100)));
        let early = Narrowing {
            hold_from_ms: Some(0),
            ..Narrowing::none(w(10, 100))
        };
        assert_eq!(early.held(), Some(w(10, 100)));
        let late = Narrowing {
            hold_from_ms: Some(100),
            ..Narrowing::none(w(0, 100))
        };
        assert_eq!(late.held(), None);
    }
}
