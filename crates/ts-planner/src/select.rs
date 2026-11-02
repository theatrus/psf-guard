// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How Target Scheduler picks a target's next exposure: the filter cadence,
//! repeat until done, the override exposure order and smart selection, with
//! the dither count each keeps.

use crate::snapshot::{CadenceItem, OrderItem};

/// `overrideexposureorderitem.action` and `filtercadenceitem.action`.
const ACTION_EXPOSURE: i64 = 0;
const ACTION_DITHER: i64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// An exposure of the target's enabled plan at this index.
    Exposure(usize),
    Dither,
}

/// A target's place in its cadence, which Target Scheduler saves as it
/// plans.
#[derive(Clone, Debug)]
pub(crate) struct Cadence {
    steps: Vec<Step>,
    next: usize,
    last: Option<usize>,
}

impl Cadence {
    /// The saved rows, when they are well formed: orders 1 to N and exactly
    /// one marked next.
    fn restore(items: &[CadenceItem], plans: usize) -> Option<Self> {
        if items.is_empty() {
            return None;
        }
        let mut sorted = items.to_vec();
        sorted.sort_by_key(|item| item.order);
        let ordered = sorted
            .iter()
            .enumerate()
            .all(|(i, item)| item.order == i as i64 + 1);
        let marked: Vec<usize> = sorted
            .iter()
            .enumerate()
            .filter(|(_, item)| item.next)
            .map(|(i, _)| i)
            .collect();
        if !ordered || marked.len() != 1 {
            return None;
        }
        let steps = sorted
            .iter()
            .map(|item| match item.action {
                ACTION_DITHER => Some(Step::Dither),
                ACTION_EXPOSURE => item
                    .reference_index
                    .and_then(|i| usize::try_from(i).ok())
                    .filter(|i| *i < plans)
                    .map(Step::Exposure),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            steps,
            next: marked[0],
            last: None,
        })
    }

    /// The basic cadence: each plan `frequency` times in turn.
    fn generate(plans: usize, frequency: u32) -> Self {
        let mut steps = Vec::new();
        for index in 0..plans {
            for _ in 0..frequency {
                steps.push(Step::Exposure(index));
            }
        }
        Self {
            steps,
            next: 0,
            last: None,
        }
    }

    fn from_order(order: &[OrderItem], plans: usize) -> Self {
        let steps = order
            .iter()
            .filter_map(|item| match item.action {
                ACTION_DITHER => Some(Step::Dither),
                ACTION_EXPOSURE => item
                    .reference_index
                    .and_then(|i| usize::try_from(i).ok())
                    .filter(|i| *i < plans)
                    .map(Step::Exposure),
                _ => None,
            })
            .collect();
        Self {
            steps,
            next: 0,
            last: None,
        }
    }

    /// The steps from `next`, once round.
    fn walk(&self) -> impl Iterator<Item = (usize, Step)> + '_ {
        let n = self.steps.len();
        (0..n).map(move |k| {
            let i = (self.next + k) % n;
            (i, self.steps[i])
        })
    }

    /// The exposure just taken: the cadence moves on to the step after it.
    fn advance(&mut self) {
        if let Some(last) = self.last.take()
            && !self.steps.is_empty()
        {
            self.next = (last + 1) % self.steps.len();
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Selector {
    Basic(Cadence),
    RepeatUntilDone,
    Override(Cadence),
    Smart {
        frequency: u32,
        rotation: Vec<Rotating>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct Rotating {
    plan: usize,
    active: bool,
    count: u32,
}

/// What a selector may look at: for each of the target's enabled plans,
/// whether it is rejected now, and its Moon avoidance score.
pub(crate) struct Choices<'a> {
    /// Indices of the plans in the target's working list, in its order.
    pub(crate) working: &'a [usize],
    pub(crate) rejected: &'a dyn Fn(usize) -> bool,
    pub(crate) moon_score: &'a dyn Fn(usize) -> f64,
}

/// A selection: the plan, and whether a dither comes first. The dither of
/// basic, repeat-until-done and smart selection is the target's dither
/// count's to decide.
pub(crate) struct Selected {
    pub(crate) plan: usize,
    /// Set by the override order's own dither steps.
    pub(crate) order_dither: Option<bool>,
}

impl Selector {
    pub(crate) fn new(
        smart: bool,
        frequency: u32,
        order: &[OrderItem],
        cadence: &[CadenceItem],
        plans: usize,
    ) -> Self {
        if smart {
            Self::Smart {
                frequency,
                rotation: Vec::new(),
            }
        } else if !order.is_empty() {
            Self::Override(
                Cadence::restore(cadence, plans)
                    .unwrap_or_else(|| Cadence::from_order(order, plans)),
            )
        } else if frequency == 0 {
            Self::RepeatUntilDone
        } else {
            Self::Basic(
                Cadence::restore(cadence, plans)
                    .unwrap_or_else(|| Cadence::generate(plans, frequency)),
            )
        }
    }

    pub(crate) fn is_override(&self) -> bool {
        matches!(self, Self::Override(_))
    }

    /// Plans the override order refers to; empty for any other selector.
    pub(crate) fn referenced(&self) -> Vec<usize> {
        match self {
            Self::Override(cadence) => cadence
                .steps
                .iter()
                .filter_map(|step| match step {
                    Step::Exposure(i) => Some(*i),
                    Step::Dither => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn select(&mut self, choices: &Choices) -> Option<Selected> {
        if choices.working.iter().all(|i| (choices.rejected)(*i)) {
            return None;
        }
        match self {
            Self::RepeatUntilDone => choices
                .working
                .iter()
                .copied()
                .find(|i| !(choices.rejected)(*i))
                .map(|plan| Selected {
                    plan,
                    order_dither: None,
                }),
            Self::Basic(cadence) => {
                let found = cadence.walk().find_map(|(at, step)| match step {
                    Step::Exposure(plan) if !(choices.rejected)(plan) => Some((at, plan)),
                    _ => None,
                })?;
                cadence.last = Some(found.0);
                Some(Selected {
                    plan: found.1,
                    order_dither: None,
                })
            }
            Self::Override(cadence) => {
                let mut dither = false;
                let mut found = None;
                for (at, step) in cadence.walk() {
                    match step {
                        Step::Dither => dither = true,
                        Step::Exposure(plan) if !(choices.rejected)(plan) => {
                            found = Some((at, plan));
                            break;
                        }
                        Step::Exposure(_) => {}
                    }
                }
                let (at, plan) = found?;
                cadence.last = Some(at);
                Some(Selected {
                    plan,
                    order_dither: Some(dither),
                })
            }
            Self::Smart {
                frequency,
                rotation,
            } => {
                let mut best: Option<(usize, f64)> = None;
                for &plan in choices.working {
                    if (choices.rejected)(plan) {
                        continue;
                    }
                    let score = (choices.moon_score)(plan);
                    if best.is_none_or(|(_, top)| score > top) {
                        best = Some((plan, score));
                    }
                }
                let (mut chosen, top) = best?;
                if *frequency > 0 {
                    let candidates: Vec<usize> = choices
                        .working
                        .iter()
                        .copied()
                        .filter(|plan| {
                            !(choices.rejected)(*plan)
                                && ((choices.moon_score)(*plan) - top).abs() <= 0.01
                        })
                        .collect();
                    if candidates.len() > 1 {
                        if rotation.is_empty() {
                            *rotation = choices
                                .working
                                .iter()
                                .map(|&plan| Rotating {
                                    plan,
                                    active: false,
                                    count: 0,
                                })
                                .collect();
                        }
                        for entry in rotation.iter_mut() {
                            entry.active = candidates.contains(&entry.plan);
                        }
                        if !rotation.iter().any(|e| e.active && e.count < *frequency) {
                            for entry in rotation.iter_mut().filter(|e| e.active) {
                                entry.count = 0;
                            }
                        }
                        if let Some(entry) = rotation.iter().find(|e| {
                            e.active && e.count < *frequency && candidates.contains(&e.plan)
                        }) {
                            chosen = entry.plan;
                        }
                    }
                }
                Some(Selected {
                    plan: chosen,
                    order_dither: None,
                })
            }
        }
    }

    /// The exposure the last selection chose was taken.
    pub(crate) fn taken(&mut self, plan: usize) {
        match self {
            Self::Basic(cadence) | Self::Override(cadence) => cadence.advance(),
            Self::RepeatUntilDone => {}
            Self::Smart {
                frequency,
                rotation,
            } => {
                if let Some(entry) = rotation.iter_mut().find(|e| e.plan == plan) {
                    entry.count = (entry.count + 1).min(*frequency);
                }
            }
        }
    }

    /// Forget the smart rotation, as the preview does on returning to a
    /// target.
    pub(crate) fn reset(&mut self) {
        if let Self::Smart { rotation, .. } = self {
            rotation.clear();
        }
    }
}

/// The filters taken since the last dither, by name.
#[derive(Clone, Debug, Default)]
pub(crate) struct DitherCount {
    taken: Vec<String>,
}

impl DitherCount {
    /// Whether to dither before an exposure of `filter`: once `every`
    /// exposures of it have been taken since the last dither. 0 never.
    pub(crate) fn required(&self, filter: &str, every: u32) -> bool {
        every > 0 && self.taken.iter().filter(|f| f.as_str() == filter).count() >= every as usize
    }

    pub(crate) fn taken(&mut self, filter: &str, dithered: bool) {
        if dithered {
            self.taken.clear();
        }
        self.taken.push(filter.to_owned());
    }

    pub(crate) fn clear(&mut self) {
        self.taken.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices<'a>(
        working: &'a [usize],
        rejected: &'a dyn Fn(usize) -> bool,
        score: &'a dyn Fn(usize) -> f64,
    ) -> Choices<'a> {
        Choices {
            working,
            rejected,
            moon_score: score,
        }
    }

    #[test]
    fn the_basic_cadence_takes_each_filter_in_turn_and_skips_a_rejected_one() {
        let mut selector = Selector::new(false, 2, &[], &[], 3);
        let none = |_| false;
        let zero = |_| 0.0;
        let mut taken = Vec::new();
        for _ in 0..6 {
            let pick = selector
                .select(&choices(&[0, 1, 2], &none, &zero))
                .unwrap()
                .plan;
            selector.taken(pick);
            taken.push(pick);
        }
        assert_eq!(taken, [0, 0, 1, 1, 2, 2]);
        // With R rejected the cadence goes on to G without moving its place.
        let no_red = |i| i == 1;
        let pick = selector
            .select(&choices(&[0, 1, 2], &no_red, &zero))
            .unwrap()
            .plan;
        assert_eq!(pick, 0);
        selector.taken(pick);
        let pick = selector
            .select(&choices(&[0, 1, 2], &no_red, &zero))
            .unwrap()
            .plan;
        assert_eq!(pick, 0);
        selector.taken(pick);
        assert_eq!(
            selector
                .select(&choices(&[0, 1, 2], &no_red, &zero))
                .unwrap()
                .plan,
            2
        );
    }

    #[test]
    fn the_override_order_dithers_where_it_says() {
        let order = [
            OrderItem {
                order: 1,
                action: 0,
                reference_index: Some(0),
            },
            OrderItem {
                order: 2,
                action: 1,
                reference_index: Some(-1),
            },
            OrderItem {
                order: 3,
                action: 0,
                reference_index: Some(1),
            },
        ];
        let mut selector = Selector::new(false, 1, &order, &[], 2);
        let none = |_| false;
        let zero = |_| 0.0;
        let first = selector.select(&choices(&[0, 1], &none, &zero)).unwrap();
        assert_eq!((first.plan, first.order_dither), (0, Some(false)));
        selector.taken(first.plan);
        let second = selector.select(&choices(&[0, 1], &none, &zero)).unwrap();
        assert_eq!((second.plan, second.order_dither), (1, Some(true)));
    }

    #[test]
    fn smart_selection_takes_the_strictest_moon_rule_and_rotates_ties() {
        let mut selector = Selector::new(true, 1, &[], &[], 3);
        let none = |_| false;
        let score = |i| [0.2, 0.6, 0.6][i];
        let first = selector
            .select(&choices(&[0, 1, 2], &none, &score))
            .unwrap()
            .plan;
        selector.taken(first);
        let second = selector
            .select(&choices(&[0, 1, 2], &none, &score))
            .unwrap()
            .plan;
        selector.taken(second);
        let third = selector
            .select(&choices(&[0, 1, 2], &none, &score))
            .unwrap()
            .plan;
        assert_eq!((first, second, third), (1, 2, 1));
    }

    #[test]
    fn a_dither_comes_before_the_next_use_of_a_filter_after_every_n() {
        let mut count = DitherCount::default();
        assert!(!count.required("L", 2));
        count.taken("L", false);
        count.taken("R", false);
        assert!(!count.required("L", 2));
        count.taken("L", false);
        assert!(count.required("L", 2));
        assert!(!count.required("R", 2));
        count.taken("L", true);
        assert!(!count.required("R", 2));
        assert!(!count.required("L", 0));
    }
}
