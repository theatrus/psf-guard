//! The boundary of the visible part of a sky grid, as closed lines.
//!
//! The grid's columns are right ascension and wrap around; its first and
//! last rows are the celestial poles, where every column is the same place
//! and so has the same value. A boundary therefore never ends: it closes on
//! itself, across the wrap if need be, and a cap around a pole comes out as
//! one ring.
//!
//! Each line runs with the visible sky on its left, columns to the right
//! and rows up.

use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap};

/// A grid node: column and row. A column equal to the grid's width is the
/// first column again, one turn on.
pub(super) type Node = (usize, usize);

/// A grid edge by its lower or left node: along a row, or up a column.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
enum Edge {
    Along(usize, usize),
    Up(usize, usize),
}

/// The lines between visible and hidden nodes of a `cols` by `rows` grid,
/// row by row from the lowest. `crossing` gives how far along the way from
/// a visible node to its hidden neighbour the boundary falls, from 0 to 1.
/// Points are in grid units: column, which may be a fraction and stays
/// under `cols`, then row. Every line closes, its first point repeated as
/// its last.
pub(super) fn contour(
    visible: &[bool],
    cols: usize,
    rows: usize,
    crossing: &(dyn Fn(Node, Node) -> f64 + Sync),
) -> Vec<Vec<(f64, f64)>> {
    if cols == 0 || rows < 2 || visible.len() != cols * rows {
        return Vec::new();
    }
    let at = |(col, row): Node| visible[row * cols + col % cols];
    // Each boundary piece inside a cell, from the edge it enters by to the
    // edge it leaves by.
    let mut next: HashMap<Edge, Edge> = HashMap::new();
    // Corners counter-clockwise from the lower left; edge `i` joins corner
    // `i` to the next.
    const CORNER: [(f64, f64); 4] = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    const MIDDLE: [(f64, f64); 4] = [(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)];
    for row in 0..rows - 1 {
        for col in 0..cols {
            let corners = [
                at((col, row)),
                at((col + 1, row)),
                at((col + 1, row + 1)),
                at((col, row + 1)),
            ];
            let edges = [
                Edge::Along(col, row),
                Edge::Up((col + 1) % cols, row),
                Edge::Along(col, row + 1),
                Edge::Up(col, row),
            ];
            let mut join = |a: usize, b: usize, seen: usize| {
                // Run the piece so the visible corner is on its left.
                let (p, q, corner) = (MIDDLE[a], MIDDLE[b], CORNER[seen]);
                let turn = (q.0 - p.0) * (corner.1 - p.1) - (q.1 - p.1) * (corner.0 - p.0);
                let (from, to) = if turn > 0.0 { (a, b) } else { (b, a) };
                next.insert(edges[from], edges[to]);
            };
            let crossed: Vec<usize> = (0..4)
                .filter(|i| corners[*i] != corners[(*i + 1) % 4])
                .collect();
            match crossed.len() {
                2 => {
                    if let Some(seen) = (0..4).find(|i| corners[*i]) {
                        join(crossed[0], crossed[1], seen);
                    }
                }
                // Two visible corners on a diagonal: cut each off on its
                // own, so the lines never cross.
                4 => {
                    for seen in (0..4).filter(|i| corners[*i]) {
                        join((seen + 3) % 4, seen, seen);
                    }
                }
                _ => {}
            }
        }
    }
    // Where the boundary crosses each edge, every edge on its own thread
    // of the shared pool: placing a crossing is the costly part.
    let crossed: BTreeSet<Edge> = next.iter().flat_map(|(from, to)| [*from, *to]).collect();
    let crossed: Vec<Edge> = crossed.into_iter().collect();
    let places: HashMap<Edge, (f64, f64)> = crossed
        .par_iter()
        .map(|edge| {
            let (a, b) = match *edge {
                Edge::Along(col, row) => ((col, row), (col + 1, row)),
                Edge::Up(col, row) => ((col, row), (col, row + 1)),
            };
            let (from, to) = if at(a) { (a, b) } else { (b, a) };
            let share = crossing(from, to).clamp(0.0, 1.0);
            let across = from.0 as f64 + (to.0 as f64 - from.0 as f64) * share;
            let up = from.1 as f64 + (to.1 as f64 - from.1 as f64) * share;
            // The far side of the last column is the first column.
            let across = if across >= cols as f64 {
                across - cols as f64
            } else {
                across
            };
            (*edge, (across, up))
        })
        .collect();
    let mut lines = Vec::new();
    let mut walked: BTreeSet<Edge> = BTreeSet::new();
    for start in &crossed {
        if walked.contains(start) || !next.contains_key(start) {
            continue;
        }
        let mut line = Vec::new();
        let mut edge = *start;
        loop {
            walked.insert(edge);
            line.push(places[&edge]);
            match next.get(&edge) {
                Some(following) if *following == *start => {
                    line.push(places[start]);
                    break;
                }
                Some(following) if !walked.contains(following) => edge = *following,
                // A line that does not close cannot come from a grid whose
                // pole rows are uniform; leave it open rather than loop.
                Some(following) => {
                    line.push(places[following]);
                    break;
                }
                None => break,
            }
        }
        if line.len() > 2 {
            lines.push(line);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLS: usize = 36;
    const ROWS: usize = 19;

    fn grid(seen: impl Fn(usize, usize) -> bool) -> Vec<bool> {
        (0..ROWS)
            .flat_map(|row| (0..COLS).map(move |col| (col, row)))
            .map(|(col, row)| seen(col, row))
            .collect()
    }

    fn halfway(_: Node, _: Node) -> f64 {
        0.5
    }

    /// Twice the area a closed line encloses, positive when it runs
    /// counter-clockwise: the visible side on the left.
    fn turning(line: &[(f64, f64)]) -> f64 {
        line.windows(2)
            .map(|pair| pair[0].0 * pair[1].1 - pair[1].0 * pair[0].1)
            .sum()
    }

    fn closed(line: &[(f64, f64)]) -> bool {
        line.first() == line.last() && line.len() > 3
    }

    #[test]
    fn a_patch_is_one_closed_line_with_the_patch_on_its_left() {
        let visible = grid(|col, row| (10..=14).contains(&col) && (5..=8).contains(&row));
        let lines = contour(&visible, COLS, ROWS, &halfway);
        assert_eq!(lines.len(), 1);
        assert!(closed(&lines[0]));
        assert!(turning(&lines[0]) > 0.0);
        // Half a cell outside the patch all round.
        let across = lines[0].iter().map(|p| p.0);
        assert_eq!(across.clone().fold(f64::MAX, f64::min), 9.5);
        assert_eq!(across.fold(f64::MIN, f64::max), 14.5);
        // A hole in an otherwise visible sky runs the other way.
        let hole = grid(|col, row| !((10..=14).contains(&col) && (5..=8).contains(&row)));
        let lines = contour(&hole, COLS, ROWS, &halfway);
        assert_eq!(lines.len(), 1);
        assert!(closed(&lines[0]));
        assert!(turning(&lines[0]) < 0.0);
    }

    #[test]
    fn a_patch_across_the_wrap_is_still_one_closed_line() {
        let visible = grid(|col, row| !(3..=33).contains(&col) && (5..=8).contains(&row));
        let lines = contour(&visible, COLS, ROWS, &halfway);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(closed(&lines[0]));
        // Columns stay inside the grid: the wrap is a jump, not a spike.
        assert!(lines[0].iter().all(|p| (0.0..COLS as f64).contains(&p.0)));
        // 5 columns either side of the wrap and half a cell beyond: 2.5 to
        // 33.5 the short way round, never through the middle of the grid.
        assert!(lines[0].iter().all(|p| p.0 <= 2.5 || p.0 >= 33.5));
        assert!(lines[0].iter().any(|p| p.0 == 2.5));
        assert!(lines[0].iter().any(|p| p.0 == 33.5));
    }

    #[test]
    fn a_polar_cap_is_one_ring_and_all_or_nothing_is_no_line() {
        let cap = grid(|_, row| row >= 14);
        let lines = contour(&cap, COLS, ROWS, &|_, _| 0.25);
        assert_eq!(lines.len(), 1);
        let ring = &lines[0];
        assert!(closed(ring));
        // One point per column, a quarter of the way down from the lowest
        // visible row, and no point at the pole.
        assert_eq!(ring.len(), COLS + 1);
        assert!(ring.iter().all(|p| p.1 == 13.75), "{ring:?}");
        // The cap is to the north, so the ring runs with rising columns.
        let rising = ring.windows(2).filter(|pair| pair[1].0 > pair[0].0).count();
        assert_eq!(rising, COLS - 1);
        // The southern cap runs the other way.
        let south = grid(|_, row| row <= 3);
        let ring = &contour(&south, COLS, ROWS, &halfway)[0];
        assert!(closed(ring));
        let rising = ring.windows(2).filter(|pair| pair[1].0 > pair[0].0).count();
        assert_eq!(rising, 1);
        assert!(contour(&grid(|_, _| true), COLS, ROWS, &halfway).is_empty());
        assert!(contour(&grid(|_, _| false), COLS, ROWS, &halfway).is_empty());
        assert!(contour(&[true], COLS, ROWS, &halfway).is_empty());
    }

    #[test]
    fn corners_that_touch_stay_two_closed_lines() {
        // Two patches meeting at one corner.
        let visible = grid(|col, row| (col, row) == (10, 5) || (col, row) == (11, 6));
        let lines = contour(&visible, COLS, ROWS, &halfway);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().all(|line| closed(line)));
        assert!(lines.iter().all(|line| turning(line) > 0.0));
    }

    #[test]
    fn the_crossing_is_measured_from_the_visible_node() {
        let visible = grid(|col, row| (10..=14).contains(&col) && (5..=8).contains(&row));
        let asked = std::sync::Mutex::new(Vec::new());
        let lines = contour(&visible, COLS, ROWS, &|from, to| {
            asked.lock().unwrap().push((from, to));
            0.1
        });
        let asked = asked.into_inner().unwrap();
        assert!(asked
            .iter()
            .all(|(from, to)| visible[from.1 * COLS + from.0] && !visible[to.1 * COLS + to.0]));
        // A tenth of a cell outside the patch.
        let across = lines[0].iter().map(|p| p.0);
        assert!((across.clone().fold(f64::MAX, f64::min) - 9.9).abs() < 1e-9);
        assert!((across.fold(f64::MIN, f64::max) - 14.1).abs() < 1e-9);
    }
}
