//! A year from one measured night a week: the nights between, worked out.
//!
//! From one night to the next the stars come up about four minutes
//! earlier, so the part of the sky the dark hours reach moves on by about
//! a degree of right ascension, while dusk, dawn and the length of the
//! dark change slowly. A night between two measured nights is therefore
//! close to either of them with the sky turned: to the one before, turned
//! back a degree for each night since, and to the one after, turned on a
//! degree for each night to come.
//!
//! The estimate takes a place to be in view on such a night only where
//! both measured nights have it in view, each turned to that night. The
//! turn is taken in steps of two degrees, the grid's own, and an odd
//! number of degrees asks both steps around it. So the estimate errs one
//! way: it may leave out a night a place was in view, and does not add
//! one. Sky it calls in view every night is in view on every measured
//! night with seven degrees of right ascension to spare each way.
//!
//! Held against a year with every night measured, at six sites from the
//! equator to Tromsø (`year_check`): the line round the sky in view every
//! night lies within about two degrees of the true one, and where a
//! week's sampling left teeth three degrees deep it leaves a quarter of a
//! degree; a node here and there, three at most, is called in view every
//! night when it misses one or two; a node's count is at most two nights
//! over and, right on an edge, up to a sixth of the year under. Over the
//! whole grid a count is a third of a percent of the nights under on
//! average, and 1.3% at Tromsø: where the dark comes back within a few
//! weeks, the nights between a measured night with no dark and the next
//! with some are all left out. More measured nights in those weeks would
//! mend that, at the cost of their skies.
//!
//! The same rule serves a node of the grid, where the turned places are
//! other nodes and cost nothing, and any place between nodes, where each
//! turned place is measured: a card about a place and the picture beside
//! it are one estimate.

/// The measured nights lie this many nights apart.
pub(super) const EVERY: usize = 7;
/// How far the sky in view is taken to turn from one night to the next,
/// in degrees of right ascension. The true figure is 0.986.
pub(super) const TURN_DEGREES_A_NIGHT: f64 = 1.0;
/// How far, in steps of two degrees, a measured night is turned either
/// way to stand for the nights around it: six degrees for six nights.
pub(super) const REACH: usize = 3;

/// The six nights between two measured nights, for one place. `before[i]`
/// is whether the place `2 i` degrees of right ascension behind it is in
/// view on the measured night before, and `after[i]` whether the place
/// `2 i` degrees ahead is in view on the measured night after.
pub(super) fn between(before: [bool; REACH + 1], after: [bool; REACH + 1]) -> [bool; EVERY - 1] {
    let (a, b) = (before, after);
    [
        // One night on: a degree back from the night before, six degrees
        // on from the night after.
        a[0] && a[1] && b[3],
        a[1] && b[2] && b[3],
        a[1] && a[2] && b[2],
        a[2] && b[1] && b[2],
        a[2] && a[3] && b[1],
        a[3] && b[0] && b[1],
    ]
}

/// Whether one node is in view on one night of the year, from the
/// measured nights' grids: night 0 is the first measured night. The rule
/// [`counts`] counts by, one night at a time.
#[cfg(test)]
pub(super) fn night_seen(grids: &[&[bool]], cols: usize, night: usize, index: usize) -> bool {
    let (week, since) = (night / EVERY, night % EVERY);
    if since == 0 {
        return grids[week][index];
    }
    let (row, col) = (index / cols, index % cols);
    let at = |grid: &[bool], step: i32| {
        grid[row * cols + (col as i32 + step).rem_euclid(cols as i32) as usize]
    };
    let before: [bool; REACH + 1] = std::array::from_fn(|i| at(grids[week], -(i as i32)));
    let after: [bool; REACH + 1] = std::array::from_fn(|i| at(grids[week + 1], i as i32));
    between(before, after)[since - 1]
}

/// For every node, on how many of `nights` nights from night `first` of
/// the year it is in view: what a client adds up from the measured
/// nights' grids to show one time of year.
#[cfg(test)]
pub(super) fn window_counts(
    grids: &[&[bool]],
    cols: usize,
    first: usize,
    nights: usize,
) -> Vec<u16> {
    let nodes = grids.first().map_or(0, |grid| grid.len());
    (0..nodes)
        .map(|index| {
            (first..first + nights)
                .filter(|night| night_seen(grids, cols, *night, index))
                .count() as u16
        })
        .collect()
}

/// The measured nights' grids packed for a client: a bit a node, the
/// lowest bit of each byte first, each night padded to whole bytes.
pub(super) fn pack(grids: &[&[bool]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for grid in grids {
        for nodes in grid.chunks(8) {
            let mut byte = 0_u8;
            for (bit, seen) in nodes.iter().enumerate() {
                byte |= u8::from(*seen) << bit;
            }
            bytes.push(byte);
        }
    }
    bytes
}

/// How many nights the measured nights stand for: themselves and the six
/// between each two.
pub(super) fn nights_counted(measured: usize) -> usize {
    if measured == 0 {
        0
    } else {
        (measured - 1) * EVERY + 1
    }
}

/// On how many nights of the year one place is in view. `seen(night, i)`
/// is whether, on measured night `night`, the place `2 i` degrees of right
/// ascension from it is in view: behind it for a negative `i`. It is asked
/// at most once for each night and step.
pub(super) fn count<E>(
    measured: usize,
    mut seen: impl FnMut(usize, i32) -> Result<bool, E>,
) -> Result<u32, E> {
    let mut nights = 0;
    // The night before's places behind, kept from the last turn.
    let mut before: Option<[bool; REACH + 1]> = None;
    for night in 0..measured {
        let here = seen(night, 0)?;
        nights += u32::from(here);
        if let Some(before) = before {
            let after = [here, seen(night, 1)?, seen(night, 2)?, seen(night, 3)?];
            nights += between(before, after).iter().filter(|seen| **seen).count() as u32;
        }
        if night + 1 < measured {
            before = Some([here, seen(night, -1)?, seen(night, -2)?, seen(night, -3)?]);
        }
    }
    Ok(nights)
}

/// Whether one place is in view on every night of the year, or, with
/// `every` false, on any: [`count`] at its two ends, stopping as soon as
/// the answer is known.
pub(super) fn all_or_any<E>(
    measured: usize,
    every: bool,
    mut seen: impl FnMut(usize, i32) -> Result<bool, E>,
) -> Result<bool, E> {
    let mut before: Option<[bool; REACH + 1]> = None;
    for night in 0..measured {
        let here = seen(night, 0)?;
        if here != every {
            return Ok(!every);
        }
        if let Some(before) = before {
            let after = [here, seen(night, 1)?, seen(night, 2)?, seen(night, 3)?];
            if between(before, after).contains(&!every) {
                return Ok(!every);
            }
        }
        if night + 1 < measured {
            before = Some([here, seen(night, -1)?, seen(night, -2)?, seen(night, -3)?]);
        }
    }
    Ok(every)
}

/// For every node of a grid `cols` wide, on how many nights of the year
/// it is in view, from the measured nights' grids. A grid's columns are
/// two degrees of right ascension apart and wrap, so a turned node is
/// another node of its row.
pub(super) fn counts(grids: &[&[bool]], cols: usize) -> Vec<u16> {
    let nodes = grids.first().map_or(0, |grid| grid.len());
    (0..nodes)
        .map(|index| {
            let (row, col) = (index / cols, index % cols);
            let seen = |night: usize, step: i32| -> Result<bool, std::convert::Infallible> {
                let turned = (col as i32 + step).rem_euclid(cols as i32) as usize;
                Ok(grids[night][row * cols + turned])
            };
            match count(grids.len(), seen) {
                Ok(nights) => nights as u16,
                Err(never) => match never {},
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_night_between_needs_both_measured_nights_turned_to_it() {
        let all = [true; 4];
        assert_eq!(between(all, all), [true; 6]);
        assert_eq!(between([false; 4], all), [false; 6]);
        assert_eq!(between(all, [false; 4]), [false; 6]);
        // The place two degrees behind, out of view on the night before:
        // the nights one to three on, which turn back to it, are lost.
        assert_eq!(
            between([true, false, true, true], all),
            [false, false, false, true, true, true]
        );
        // The place six degrees ahead, out of view on the night after: the
        // first two nights, which turn on to it, are lost.
        assert_eq!(
            between(all, [true, true, true, false]),
            [false, false, true, true, true, true]
        );
        // The place itself out of view on the night after costs only the
        // night just before it.
        assert_eq!(
            between(all, [false, true, true, true]),
            [true, true, true, true, true, false]
        );
    }

    #[test]
    fn a_year_counts_the_measured_nights_and_the_six_between_each_two() {
        assert_eq!(nights_counted(53), 365);
        assert_eq!(nights_counted(1), 1);
        assert_eq!(nights_counted(0), 0);
        let always = |_: usize, _: i32| Ok::<bool, ()>(true);
        assert_eq!(count(53, always), Ok(365));
        assert_eq!(count(53, |_, _| Ok::<bool, ()>(false)), Ok(0));
        assert_eq!(all_or_any(53, true, always), Ok(true));
        assert_eq!(
            all_or_any(53, false, |_, _| Ok::<bool, ()>(false)),
            Ok(false)
        );
        // In view on the measured nights only where it stands, not a step
        // either way: the 53 measured nights count and none between.
        let alone = |_: usize, step: i32| Ok::<bool, ()>(step == 0);
        assert_eq!(count(53, alone), Ok(53));
        assert_eq!(all_or_any(53, true, alone), Ok(false));
        assert_eq!(all_or_any(53, false, alone), Ok(true));
        // One measured night out of view takes its own night and the six
        // either side with it.
        let but_one = |night: usize, _: i32| Ok::<bool, ()>(night != 10);
        assert_eq!(count(53, but_one), Ok(365 - 13));
        assert_eq!(all_or_any(53, true, but_one), Ok(false));
        // Each night and step is asked once, and errors pass through.
        let mut asked = std::collections::BTreeSet::new();
        let once = |night: usize, step: i32| {
            assert!(asked.insert((night, step)), "{night} {step} asked twice");
            Ok::<bool, ()>(true)
        };
        assert_eq!(count(5, once), Ok(29));
        // The first night is never turned on, nor the last turned back.
        assert_eq!(asked.len(), 5 + 4 * 6);
        assert!(!asked.contains(&(0, 1)) && !asked.contains(&(4, -1)));
        assert_eq!(
            count(5, |night, _| if night == 3 {
                Err("lost")
            } else {
                Ok(true)
            }),
            Err("lost")
        );
        // Stopping early gives the answers counting gives.
        for pattern in 0..64_u32 {
            let seen = |night: usize, step: i32| {
                Ok::<bool, ()>(
                    (pattern >> ((night * 3 + (step + 3) as usize) % 6)) & 1 == 1 || night == 2,
                )
            };
            let nights = count(6, seen).unwrap();
            assert_eq!(
                all_or_any(6, true, seen).unwrap(),
                nights == 36,
                "{pattern}"
            );
            assert_eq!(all_or_any(6, false, seen).unwrap(), nights > 0, "{pattern}");
        }
    }

    /// The rule as a client works it out from the constants an answer
    /// carries: a night `since` nights after a measured night turns back
    /// `since` degrees from it and on `EVERY - since` degrees from the next,
    /// and a turn between two steps of the grid asks both.
    #[test]
    fn the_table_is_the_turn_in_steps_of_the_grid() {
        let step_degrees = 2.0;
        for pattern in 0..256_u32 {
            let before: [bool; 4] = std::array::from_fn(|i| pattern >> i & 1 == 1);
            let after: [bool; 4] = std::array::from_fn(|i| pattern >> (i + 4) & 1 == 1);
            for since in 1..EVERY {
                let back = since as f64 * TURN_DEGREES_A_NIGHT / step_degrees;
                let on = (EVERY - since) as f64 * TURN_DEGREES_A_NIGHT / step_degrees;
                let wanted = before[back.floor() as usize]
                    && before[back.ceil() as usize]
                    && after[on.floor() as usize]
                    && after[on.ceil() as usize];
                assert_eq!(
                    between(before, after)[since - 1],
                    wanted,
                    "{pattern} {since}"
                );
            }
        }
        assert_eq!(
            REACH,
            ((EVERY - 1) as f64 * TURN_DEGREES_A_NIGHT / step_degrees).ceil() as usize
        );
    }

    /// A small sky the client's tests hold too (`skySeason.test.ts`): six
    /// measured nights of three rows by twelve columns.
    fn small() -> Vec<Vec<bool>> {
        (0..6_usize)
            .map(|night| {
                (0..36_usize)
                    .map(|index| {
                        let (row, col) = (index / 12, index % 12);
                        match row {
                            0 => false,
                            1 => (col + 12 - (3 * night) % 12) % 12 < 7,
                            _ => night != 3 || col > 4,
                        }
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_window_adds_up_the_nights_the_whole_count_is_made_of() {
        let small = small();
        let grids: Vec<&[bool]> = small.iter().map(|grid| grid.as_slice()).collect();
        let whole = counts(&grids, 12);
        assert_eq!(window_counts(&grids, 12, 0, 36), whole);
        let (early, late) = (
            window_counts(&grids, 12, 0, 10),
            window_counts(&grids, 12, 10, 26),
        );
        assert!(whole
            .iter()
            .zip(early.iter().zip(&late))
            .all(|(all, (a, b))| all == &(a + b)));
        // The numbers the client's test expects of its own adding up.
        assert_eq!(
            window_counts(&grids, 12, 0, 10),
            [
                vec![0; 12],
                vec![1, 3, 5, 8, 10, 10, 10, 7, 5, 3, 0, 0],
                vec![10; 12]
            ]
            .concat()
        );
        assert_eq!(
            window_counts(&grids, 12, 14, 10),
            [
                vec![0; 12],
                vec![10, 7, 5, 3, 0, 0, 1, 3, 5, 8, 10, 10],
                vec![2, 1, 2, 4, 6, 8, 10, 10, 10, 8, 6, 4]
            ]
            .concat()
        );
        // Packed a bit a node, lowest bit first, a night to whole bytes.
        let packed = pack(&grids);
        assert_eq!(packed.len(), 6 * 5);
        for (night, grid) in grids.iter().enumerate() {
            for (index, seen) in grid.iter().enumerate() {
                assert_eq!(packed[night * 5 + index / 8] >> (index % 8) & 1 == 1, *seen);
            }
        }
    }

    #[test]
    fn a_grid_turns_along_its_rows_and_wraps() {
        // One row, a ring of twelve columns. A gap one column wide, at
        // column 11 on the first measured night and three columns on, at
        // column 2, on the next: the sky turned six degrees in the week.
        let cols = 12;
        let first: Vec<bool> = (0..cols).map(|col| col != 11).collect();
        let second: Vec<bool> = (0..cols).map(|col| col != 2).collect();
        let nights = counts(&[first.as_slice(), second.as_slice()], cols);
        // Eight nights in all. Far from the gap nothing is lost.
        assert!((4..=8).all(|col| nights[col] == 8), "{nights:?}");
        // The gap crosses columns 11, 0, 1 and 2 over the week, across the
        // wrap, and each loses nights: more nights are lost than the eight
        // the gap really takes, never fewer.
        assert!(
            [11, 0, 1, 2].iter().all(|col| nights[*col] < 8),
            "{nights:?}"
        );
        let lost: u32 = nights.iter().map(|n| 8 - u32::from(*n)).sum();
        assert!((8..=20).contains(&lost), "{lost} {nights:?}");
        // A gap that stays put while the sky turns is not a thing of the
        // sky: each night takes it where the turn says, so a column out
        // of view on both nights is still counted in view between them.
        let stuck: Vec<bool> = (0..cols).map(|col| col != 0).collect();
        let nights = counts(&[stuck.as_slice(), stuck.as_slice()], cols);
        assert_eq!(nights[6], 8);
        assert!(nights[0] > 0 && nights[0] < 8);
        // A pole row, the same in every column, is untouched by the turn.
        let pole = vec![true; cols];
        assert!(counts(&[&pole, &pole, &pole], cols)
            .iter()
            .all(|n| *n == 15));
    }
}
