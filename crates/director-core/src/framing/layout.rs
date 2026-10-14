//! A mosaic as a set of panels, each with a stable id, its own place and a
//! lock. A grid is one way to make such a set, not the stored shape.
//!
//! Places are measured on the tangent plane at the framing's center, along
//! the camera's axes, in panel sizes: `x` in panel widths toward the
//! camera's right (west at camera angle zero), `y` in panel heights toward
//! its up (north at angle zero). Every panel of one framing shares the
//! panel size and the camera angle, so overlaps are read in the camera's
//! own frame and turning the mosaic never changes one.

use super::{FramingError, Mosaic, MAX_PANELS};
use serde::{Deserialize, Serialize};

/// The longest panel id: activation records it beside the target, and
/// Target Scheduler shows it at the end of the target's name.
pub const MAX_PANEL_ID_LEN: usize = 32;
/// Overlap and gap comparisons forgive this much of a panel side, far below
/// a pixel, so a place snapped to the minimum reads as the minimum.
pub const PLACE_TOLERANCE: f64 = 1e-6;
/// Panels this close on both axes are one place: activation would write
/// two targets on the same sky.
const SAME_PLACE: f64 = 1e-6;
/// Two panels that share at least this much of a side along their seam sit
/// side by side, and their seam should carry the minimum overlap; less is a
/// corner.
pub const SIDE_BY_SIDE: f64 = 0.5;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PanelPlacement {
    /// Stable for the panel's life. A grid names its panels `r{row}c{column}`
    /// and a panel added since gets `p{n}`; neither changes when it moves.
    pub id: String,
    /// Panel widths toward the camera's right from the framing's center.
    pub x: f64,
    /// Panel heights toward the camera's up from the framing's center.
    pub y: f64,
    /// Fixed in place: a save may not move it, or the framing's center,
    /// angle or panel size under it, while it stays locked.
    #[serde(default)]
    pub locked: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    /// The least overlap two panels that sit side by side should share, as
    /// a percentage of the panel side across their seam, 0 to 90.
    pub min_overlap_percent: u32,
    pub panels: Vec<PanelPlacement>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutIssueKind {
    /// Two panels side by side share less than the minimum overlap.
    ThinOverlap,
    /// Two panels side by side leave uncovered sky between them, narrower
    /// than a panel.
    Gap,
    /// A panel shares the minimum overlap with no other panel.
    Isolated,
    /// The panels fall into groups that do not overlap one another.
    Disconnected,
}

/// Something about a layout a person should look at. None of these refuse a
/// save: a gap may be filled later, and a lone panel may be meant.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayoutIssue {
    pub kind: LayoutIssueKind,
    /// The panels involved, by id.
    pub panels: Vec<String>,
    /// The overlap (`thin_overlap`) or the gap (`gap`) as a percentage of
    /// the panel side across the seam.
    #[serde(default, deserialize_with = "Option::deserialize")]
    pub percent: Option<f64>,
}

/// How two panels sit against each other, in panel sizes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairOverlap {
    /// Share of the side across the seam the two panels overlap; negative
    /// for a gap between them.
    pub thickness: f64,
    /// Share of the side along the seam the two panels have in common.
    pub run: f64,
    /// Whether the seam runs up and down the camera frame (panels side by
    /// side left and right) rather than across it.
    pub vertical_seam: bool,
}

/// The overlap of two panels at offsets `a` and `b`: along each camera axis
/// one minus how far apart they are, in panel sizes. The thinner axis is
/// across the seam; the other runs along it. `None` for two panels apart on
/// both axes, which have no seam.
pub fn pair_overlap(a: (f64, f64), b: (f64, f64)) -> Option<PairOverlap> {
    let across_x = 1.0 - (a.0 - b.0).abs();
    let across_y = 1.0 - (a.1 - b.1).abs();
    if across_x <= 0.0 && across_y <= 0.0 {
        return None;
    }
    // The seam is where the panels are thinner together: the axis with the
    // smaller overlap, or the one with a gap.
    let vertical_seam = across_x <= across_y;
    let (thickness, run) = if vertical_seam {
        (across_x, across_y)
    } else {
        (across_y, across_x)
    };
    Some(PairOverlap {
        thickness,
        run,
        vertical_seam,
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_PANEL_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `r{row}c{column}`, one-based, as a grid names its panels.
pub fn grid_cell(id: &str) -> Option<(u32, u32)> {
    let rest = id.strip_prefix('r')?;
    let (row, column) = rest.split_once('c')?;
    if row.starts_with('0') || column.starts_with('0') {
        return None;
    }
    let (row, column) = (row.parse::<u32>().ok()?, column.parse::<u32>().ok()?);
    (row >= 1 && column >= 1).then_some((row, column))
}

impl Layout {
    /// One panel on the framing's center, as a grid of one names it.
    pub fn single(min_overlap_percent: u32) -> Self {
        Self::grid(Mosaic {
            rows: 1,
            columns: 1,
            overlap_percent: min_overlap_percent,
        })
    }

    /// The panels a rows-by-columns grid makes, centered on the framing:
    /// row one at the top as the camera sees it, column one at its left
    /// (east at angle zero), stepped by the unoverlapped share of a panel.
    /// The grid's overlap becomes the layout's minimum.
    pub fn grid(mosaic: Mosaic) -> Self {
        let keep = 1.0 - f64::from(mosaic.overlap_percent) / 100.0;
        let mut panels = Vec::with_capacity((mosaic.rows * mosaic.columns) as usize);
        for row in 0..mosaic.rows {
            let y = (f64::from(mosaic.rows.saturating_sub(1)) / 2.0 - f64::from(row)) * keep;
            for column in 0..mosaic.columns {
                let x =
                    (f64::from(column) - f64::from(mosaic.columns.saturating_sub(1)) / 2.0) * keep;
                panels.push(PanelPlacement {
                    id: format!("r{}c{}", row + 1, column + 1),
                    x,
                    y,
                    locked: false,
                });
            }
        }
        Self {
            min_overlap_percent: mosaic.overlap_percent,
            panels,
        }
    }

    /// The grid this layout is, when it is one: every cell of a rows-by-
    /// columns grid named as a grid names it and sitting where the grid at
    /// the minimum overlap puts it. Locks do not matter.
    pub fn as_grid(&self) -> Option<Mosaic> {
        let mut rows = 0;
        let mut columns = 0;
        for panel in &self.panels {
            let (row, column) = grid_cell(&panel.id)?;
            rows = rows.max(row);
            columns = columns.max(column);
        }
        let mosaic = Mosaic {
            rows,
            columns,
            overlap_percent: self.min_overlap_percent,
        };
        if mosaic.validate().is_err() || (rows * columns) as usize != self.panels.len() {
            return None;
        }
        let grid = Self::grid(mosaic);
        let fits = grid.panels.iter().all(|cell| {
            self.panels.iter().any(|panel| {
                panel.id == cell.id
                    && (panel.x - cell.x).abs() <= PLACE_TOLERANCE
                    && (panel.y - cell.y).abs() <= PLACE_TOLERANCE
            })
        });
        fits.then_some(mosaic)
    }

    pub fn panel(&self, id: &str) -> Option<&PanelPlacement> {
        self.panels.iter().find(|panel| panel.id == id)
    }

    pub fn has_locked(&self) -> bool {
        self.panels.iter().any(|panel| panel.locked)
    }

    /// A fresh id: `p{n}`, numbered on from the panels there are, so a
    /// panel added to a 2 by 2 grid is `p5`.
    pub fn next_panel_id(&self) -> String {
        let highest = self
            .panels
            .iter()
            .filter_map(|panel| panel.id.strip_prefix('p')?.parse::<usize>().ok())
            .max()
            .unwrap_or(0);
        let mut n = highest.max(self.panels.len()) + 1;
        while self.panel(&format!("p{n}")).is_some() {
            n += 1;
        }
        format!("p{n}")
    }

    /// The structure a save needs: one to 256 panels with distinct ids and
    /// places, finite offsets, and a minimum overlap up to 90%. Gaps and
    /// thin overlaps are [`Layout::issues`], not errors.
    pub fn validate(&self) -> Result<(), FramingError> {
        if self.panels.is_empty() || self.panels.len() > MAX_PANELS || self.min_overlap_percent > 90
        {
            return Err(FramingError::InvalidMosaic);
        }
        let mut ids = std::collections::BTreeSet::new();
        for panel in &self.panels {
            if !valid_id(&panel.id) || !ids.insert(panel.id.as_str()) {
                return Err(FramingError::InvalidMosaic);
            }
            if !panel.x.is_finite() || !panel.y.is_finite() {
                return Err(FramingError::InvalidMosaic);
            }
        }
        for (i, a) in self.panels.iter().enumerate() {
            for b in &self.panels[i + 1..] {
                if (a.x - b.x).abs() < SAME_PLACE && (a.y - b.y).abs() < SAME_PLACE {
                    return Err(FramingError::InvalidMosaic);
                }
            }
        }
        Ok(())
    }

    /// From the leftmost panel's left edge to the rightmost's right edge,
    /// and bottom to top, in panel sizes.
    pub fn span(&self) -> (f64, f64) {
        let Some(first) = self.panels.first() else {
            return (0.0, 0.0);
        };
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (first.x, first.x, first.y, first.y);
        for panel in &self.panels[1..] {
            min_x = min_x.min(panel.x);
            max_x = max_x.max(panel.x);
            min_y = min_y.min(panel.y);
            max_y = max_y.max(panel.y);
        }
        (max_x - min_x + 1.0, max_y - min_y + 1.0)
    }

    /// Each panel's row and column for listing them in order: row one holds
    /// the panels furthest up, column one those furthest left, with places
    /// within a tenth of a panel counted as one. A grid's panels get their
    /// own cells; a free layout gets the sparse grid its panels fall on.
    pub fn cells(&self) -> Vec<(u32, u32)> {
        let ys: Vec<f64> = self.panels.iter().map(|p| -p.y).collect();
        let xs: Vec<f64> = self.panels.iter().map(|p| p.x).collect();
        let rows = bands(&ys, 0.1);
        let columns = bands(&xs, 0.1);
        rows.into_iter().zip(columns).collect()
    }

    /// Where the layout falls short of the minimum overlap rule, read pair by
    /// pair in the camera frame. Two panels are joined when they overlap by
    /// at least the minimum on both axes, whether side by side or at a
    /// corner. Two panels sit side by side when they share at least half a
    /// side along their seam (`run`); such a pair that overlaps by less than
    /// the minimum across the seam is a thin overlap, and one with uncovered
    /// sky between them is a gap. A corner that overlaps by less is no seam
    /// and is left alone. Panels joined to nobody are isolated, and groups
    /// that never meet are disconnected.
    pub fn issues(&self) -> Vec<LayoutIssue> {
        let minimum = f64::from(self.min_overlap_percent) / 100.0;
        let n = self.panels.len();
        let mut issues = Vec::new();
        if n < 2 {
            return issues;
        }
        let mut linked = vec![Vec::new(); n];
        let mut flagged = vec![false; n];
        for i in 0..n {
            for j in i + 1..n {
                let (a, b) = (&self.panels[i], &self.panels[j]);
                let Some(pair) = pair_overlap((a.x, a.y), (b.x, b.y)) else {
                    continue;
                };
                if pair.thickness >= -PLACE_TOLERANCE && pair.thickness + PLACE_TOLERANCE >= minimum
                {
                    linked[i].push(j);
                    linked[j].push(i);
                    continue;
                }
                if pair.run + PLACE_TOLERANCE < SIDE_BY_SIDE {
                    continue;
                }
                let ids = vec![a.id.clone(), b.id.clone()];
                if pair.thickness >= -PLACE_TOLERANCE {
                    issues.push(LayoutIssue {
                        kind: LayoutIssueKind::ThinOverlap,
                        panels: ids,
                        percent: Some(round_percent(pair.thickness.max(0.0))),
                    });
                    flagged[i] = true;
                    flagged[j] = true;
                } else if -pair.thickness < 1.0 && !self.gap_covered(i, j, pair) {
                    issues.push(LayoutIssue {
                        kind: LayoutIssueKind::Gap,
                        panels: ids,
                        percent: Some(round_percent(-pair.thickness)),
                    });
                    flagged[i] = true;
                    flagged[j] = true;
                }
            }
        }
        for i in 0..n {
            if linked[i].is_empty() && !flagged[i] {
                issues.push(LayoutIssue {
                    kind: LayoutIssueKind::Isolated,
                    panels: vec![self.panels[i].id.clone()],
                    percent: None,
                });
            }
        }
        if issues.is_empty() {
            let groups = components(&linked);
            if groups.len() > 1 {
                issues.push(LayoutIssue {
                    kind: LayoutIssueKind::Disconnected,
                    panels: groups
                        .iter()
                        .map(|group| self.panels[group[0]].id.clone())
                        .collect(),
                    percent: None,
                });
            }
        }
        issues
    }

    /// Whether other panels cover the sky between panels `i` and `j`, which
    /// face each other across a gap. Sampled at points strictly inside the
    /// gap, which is enough at the sizes a person drags to.
    fn gap_covered(&self, i: usize, j: usize, pair: PairOverlap) -> bool {
        let (a, b) = (&self.panels[i], &self.panels[j]);
        // Across the seam: from the nearer edge of one to the other. Along
        // it: the stretch both panels span.
        let (across_a, across_b, along_a, along_b) = if pair.vertical_seam {
            (a.x, b.x, a.y, b.y)
        } else {
            (a.y, b.y, a.x, b.x)
        };
        let (low, high) = if across_a < across_b {
            (across_a + 0.5, across_b - 0.5)
        } else {
            (across_b + 0.5, across_a - 0.5)
        };
        let start = along_a.max(along_b) - 0.5;
        let end = along_a.min(along_b) + 0.5;
        const ACROSS: usize = 3;
        const ALONG: usize = 5;
        for s in 0..ACROSS {
            let across = low + (high - low) * (s as f64 + 0.5) / ACROSS as f64;
            for t in 0..ALONG {
                let along = start + (end - start) * (t as f64 + 0.5) / ALONG as f64;
                let (px, py) = if pair.vertical_seam {
                    (across, along)
                } else {
                    (along, across)
                };
                let covered = self.panels.iter().enumerate().any(|(k, panel)| {
                    k != i
                        && k != j
                        && (px - panel.x).abs() <= 0.5 + PLACE_TOLERANCE
                        && (py - panel.y).abs() <= 0.5 + PLACE_TOLERANCE
                });
                if !covered {
                    return false;
                }
            }
        }
        true
    }
}

fn round_percent(share: f64) -> f64 {
    (share * 1000.0).round() / 10.0
}

/// One-based band per value: values sorted, a new band where the next value
/// sits more than `tolerance` past the last.
fn bands(values: &[f64], tolerance: f64) -> Vec<u32> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|a, b| values[*a].total_cmp(&values[*b]));
    let mut out = vec![0; values.len()];
    let mut band = 0;
    let mut last: Option<f64> = None;
    for index in order {
        let value = values[index];
        if last.is_none_or(|last| value - last > tolerance) {
            band += 1;
        }
        last = Some(value);
        out[index] = band;
    }
    out
}

/// Connected groups of a link list, each by its members in input order.
fn components(linked: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut seen = vec![false; linked.len()];
    let mut groups = Vec::new();
    for start in 0..linked.len() {
        if seen[start] {
            continue;
        }
        let mut group = vec![start];
        seen[start] = true;
        let mut next = 0;
        while next < group.len() {
            for &other in &linked[group[next]] {
                if !seen[other] {
                    seen[other] = true;
                    group.push(other);
                }
            }
            next += 1;
        }
        group.sort_unstable();
        groups.push(group);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(id: &str, x: f64, y: f64) -> PanelPlacement {
        PanelPlacement {
            id: id.into(),
            x,
            y,
            locked: false,
        }
    }

    fn layout(panels: Vec<PanelPlacement>) -> Layout {
        Layout {
            min_overlap_percent: 20,
            panels,
        }
    }

    fn kinds(layout: &Layout) -> Vec<(LayoutIssueKind, Vec<String>)> {
        layout
            .issues()
            .into_iter()
            .map(|issue| (issue.kind, issue.panels))
            .collect()
    }

    #[test]
    fn a_grid_names_its_cells_and_reads_back_as_itself() {
        let mosaic = Mosaic {
            rows: 2,
            columns: 3,
            overlap_percent: 20,
        };
        let grid = Layout::grid(mosaic);
        assert_eq!(grid.panels.len(), 6);
        assert_eq!(grid.panels[0].id, "r1c1");
        // Row one is up, column one is left: east at camera angle zero.
        assert!((grid.panels[0].x + 0.8).abs() < 1e-12 && (grid.panels[0].y - 0.4).abs() < 1e-12);
        assert_eq!(grid.panels[5].id, "r2c3");
        assert_eq!(grid.as_grid(), Some(mosaic));
        assert!(grid.issues().is_empty(), "{:?}", grid.issues());
        assert_eq!(
            grid.cells(),
            vec![(1, 1), (1, 2), (1, 3), (2, 1), (2, 2), (2, 3)]
        );
        let mut moved = grid.clone();
        moved.panels[2].x += 0.01;
        assert_eq!(moved.as_grid(), None);
        let mut added = grid.clone();
        added.panels.push(place("p7", 1.6, 0.4));
        assert_eq!(added.as_grid(), None);
        assert_eq!(added.next_panel_id(), "p8");
        assert_eq!(Layout::single(15).next_panel_id(), "p2");
    }

    #[test]
    fn panels_side_by_side_at_the_minimum_are_neighbours() {
        let pair = layout(vec![place("r1c1", 0.0, 0.0), place("p2", -0.8, 0.0)]);
        assert!(pair.issues().is_empty(), "{:?}", pair.issues());
        // A diagonal step at the minimum shares a corner of the minimum both ways.
        let diagonal = layout(vec![place("r1c1", 0.0, 0.0), place("p2", 0.8, 0.8)]);
        assert!(diagonal.issues().is_empty(), "{:?}", diagonal.issues());
        // Half a panel along a brick row still sits side by side.
        let brick = layout(vec![place("r1c1", 0.0, 0.0), place("p2", 0.8, 0.5)]);
        assert!(brick.issues().is_empty(), "{:?}", brick.issues());
    }

    #[test]
    fn a_thin_overlap_a_gap_and_a_lone_panel_are_named() {
        let thin = layout(vec![place("r1c1", 0.0, 0.0), place("p2", 0.9, 0.1)]);
        let issues = thin.issues();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, LayoutIssueKind::ThinOverlap);
        assert_eq!(issues[0].percent, Some(10.0));
        let touching = layout(vec![place("r1c1", 0.0, 0.0), place("p2", 0.0, 1.0)]);
        assert_eq!(touching.issues()[0].kind, LayoutIssueKind::ThinOverlap);
        assert_eq!(touching.issues()[0].percent, Some(0.0));
        let gap = layout(vec![place("r1c1", 0.0, 0.0), place("p2", 1.25, 0.0)]);
        let issues = gap.issues();
        assert_eq!(issues[0].kind, LayoutIssueKind::Gap);
        assert_eq!(issues[0].percent, Some(25.0));
        let far = layout(vec![place("r1c1", 0.0, 0.0), place("p2", 3.0, 3.0)]);
        assert_eq!(
            kinds(&far),
            vec![
                (LayoutIssueKind::Isolated, vec!["r1c1".to_string()]),
                (LayoutIssueKind::Isolated, vec!["p2".to_string()]),
            ]
        );
    }

    #[test]
    fn a_gap_another_panel_covers_is_no_gap() {
        // Three in a row: the outer two face each other across the middle one.
        let row = layout(vec![
            place("r1c1", -0.8, 0.0),
            place("r1c2", 0.0, 0.0),
            place("r1c3", 0.8, 0.0),
        ]);
        assert!(row.issues().is_empty(), "{:?}", row.issues());
        // Take the middle away and the outer two leave a gap between them.
        let holed = layout(vec![place("r1c1", -0.8, 0.0), place("r1c3", 0.8, 0.0)]);
        assert_eq!(holed.issues()[0].kind, LayoutIssueKind::Gap);
    }

    #[test]
    fn two_islands_are_disconnected() {
        let islands = layout(vec![
            place("r1c1", 0.0, 0.0),
            place("p2", 0.8, 0.0),
            place("p3", 4.0, 0.0),
            place("p4", 4.8, 0.0),
        ]);
        assert_eq!(
            kinds(&islands),
            vec![(
                LayoutIssueKind::Disconnected,
                vec!["r1c1".to_string(), "p3".to_string()]
            )]
        );
    }

    #[test]
    fn structure_is_checked_and_issues_are_not() {
        assert!(layout(vec![]).validate().is_err());
        assert!(layout(vec![place("a", 0.0, 0.0), place("a", 1.0, 0.0)])
            .validate()
            .is_err());
        assert!(layout(vec![place("a", 0.0, 0.0), place("b", 0.0, 0.0)])
            .validate()
            .is_err());
        assert!(layout(vec![place("a b", 0.0, 0.0)]).validate().is_err());
        assert!(layout(vec![place("r1c1~3", 0.0, 0.0)]).validate().is_err());
        assert!(layout(vec![place(&"x".repeat(33), 0.0, 0.0)])
            .validate()
            .is_err());
        assert!(layout(vec![place("a", f64::NAN, 0.0)]).validate().is_err());
        let mut wide = layout(vec![place("a", 0.0, 0.0)]);
        wide.min_overlap_percent = 91;
        assert!(wide.validate().is_err());
        // A gap is a matter for the person, not a refusal.
        assert!(layout(vec![place("a", 0.0, 0.0), place("b", 5.0, 0.0)])
            .validate()
            .is_ok());
    }

    #[test]
    fn grid_ids_parse_only_as_a_grid_writes_them() {
        assert_eq!(grid_cell("r2c3"), Some((2, 3)));
        assert_eq!(grid_cell("r0c1"), None);
        assert_eq!(grid_cell("r01c1"), None);
        assert_eq!(grid_cell("p5"), None);
        assert_eq!(grid_cell("r1"), None);
    }
}
