//! Panels as convex quadrilaterals on the framing's tangent plane, turned
//! into the mosaic's camera frame, and the minimum overlap rule read from
//! their intersections. Planned panels are rectangles on the axes; a panel
//! measured from its frames keeps the size and the slight turn it was shot
//! with, so panels of different sizes and angles are compared as they are.

use super::layout::{LayoutIssue, LayoutIssueKind, PLACE_TOLERANCE, SIDE_BY_SIDE};
use serde::{Deserialize, Serialize};

/// A point in the camera frame: right and up, in degrees on the plane (or
/// in panel sizes for a layout read without a plane).
pub type Point = [f64; 2];

/// One panel's outline in the camera frame.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PanelShape {
    pub id: String,
    /// Corners in order around the panel.
    pub corners: [Point; 4],
}

impl PanelShape {
    /// An unturned rectangle centered at `center`, `width` by `height`.
    pub fn rectangle(id: impl Into<String>, center: Point, width: f64, height: f64) -> Self {
        let (hw, hh) = (width / 2.0, height / 2.0);
        Self {
            id: id.into(),
            corners: [
                [center[0] - hw, center[1] + hh],
                [center[0] - hw, center[1] - hh],
                [center[0] + hw, center[1] - hh],
                [center[0] + hw, center[1] + hh],
            ],
        }
    }

    /// A rectangle turned counter-clockwise by `degrees` about its center.
    pub fn turned(
        id: impl Into<String>,
        center: Point,
        width: f64,
        height: f64,
        degrees: f64,
    ) -> Self {
        let mut shape = Self::rectangle(id, center, width, height);
        if degrees != 0.0 {
            let (sin, cos) = degrees.to_radians().sin_cos();
            for corner in &mut shape.corners {
                let (dx, dy) = (corner[0] - center[0], corner[1] - center[1]);
                *corner = [
                    center[0] + dx * cos - dy * sin,
                    center[1] + dx * sin + dy * cos,
                ];
            }
        }
        shape
    }

    /// Left, right, bottom and top of the outline.
    pub fn bounds(&self) -> [f64; 4] {
        bounds(&self.corners)
    }

    pub fn center(&self) -> Point {
        let mut sum = [0.0, 0.0];
        for corner in &self.corners {
            sum[0] += corner[0] / 4.0;
            sum[1] += corner[1] / 4.0;
        }
        sum
    }

    pub fn contains(&self, point: Point) -> bool {
        contains(&self.corners, point)
    }
}

fn bounds(points: &[Point]) -> [f64; 4] {
    let mut out = [
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for point in points {
        out[0] = out[0].min(point[0]);
        out[1] = out[1].max(point[0]);
        out[2] = out[2].min(point[1]);
        out[3] = out[3].max(point[1]);
    }
    out
}

fn cross(o: Point, a: Point, b: Point) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Twice the signed area: positive when the corners run counter-clockwise.
fn signed_area(points: &[Point]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// Whether a point lies inside a convex polygon or on its edge, whichever
/// way its corners run.
pub fn contains(polygon: &[Point], point: Point) -> bool {
    let orientation = signed_area(polygon).signum();
    let n = polygon.len();
    (0..n).all(|i| cross(polygon[i], polygon[(i + 1) % n], point) * orientation >= -1e-12)
}

/// The intersection of two convex polygons (Sutherland–Hodgman), empty when
/// they do not overlap.
pub fn intersection(subject: &[Point], clip: &[Point]) -> Vec<Point> {
    let orientation = signed_area(clip).signum();
    let mut output: Vec<Point> = subject.to_vec();
    let n = clip.len();
    for i in 0..n {
        if output.is_empty() {
            break;
        }
        let (a, b) = (clip[i], clip[(i + 1) % n]);
        let inside = |p: Point| cross(a, b, p) * orientation >= 0.0;
        let input = std::mem::take(&mut output);
        for j in 0..input.len() {
            let current = input[j];
            let previous = input[(j + input.len() - 1) % input.len()];
            let (current_in, previous_in) = (inside(current), inside(previous));
            if current_in {
                if !previous_in {
                    output.push(crossing(previous, current, a, b));
                }
                output.push(current);
            } else if previous_in {
                output.push(crossing(previous, current, a, b));
            }
        }
    }
    if output.len() < 3 || signed_area(&output).abs() < 1e-18 {
        return vec![];
    }
    output
}

/// Where the segment from `p` to `q` crosses the line through `a` and `b`.
fn crossing(p: Point, q: Point, a: Point, b: Point) -> Point {
    let (dpx, dpy) = (q[0] - p[0], q[1] - p[1]);
    let (dax, day) = (b[0] - a[0], b[1] - a[1]);
    let denominator = dpx * day - dpy * dax;
    if denominator.abs() < 1e-300 {
        return p;
    }
    let t = ((a[0] - p[0]) * day - (a[1] - p[1]) * dax) / denominator;
    [p[0] + t * dpx, p[1] + t * dpy]
}

/// How two panels sit against each other, as shares of their sides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeOverlap {
    /// Share of the side across the seam the two overlap, of the larger of
    /// the two panels; negative for a gap between them.
    pub thickness: f64,
    /// Share of the side along the seam they have in common, likewise.
    pub run: f64,
    /// Whether the seam runs up and down the camera frame (panels side by
    /// side left and right) rather than across it.
    pub vertical_seam: bool,
}

/// The overlap of two panels. Where they intersect, its extent along each
/// camera axis over the larger panel's extent there; where they do not, the
/// overlap of their extents along each axis, negative for a gap. The axis
/// with the smaller share is across the seam. `None` for two panels apart
/// on both axes. For panels of one size on the camera's axes this is one
/// minus how far apart their centers are, in panel sizes.
pub fn shape_overlap(a: &PanelShape, b: &PanelShape) -> Option<ShapeOverlap> {
    let (ab, bb) = (a.bounds(), b.bounds());
    let span_x = (ab[1] - ab[0]).max(bb[1] - bb[0]);
    let span_y = (ab[3] - ab[2]).max(bb[3] - bb[2]);
    if !(span_x > 0.0 && span_y > 0.0) {
        return None;
    }
    let ox = (ab[1].min(bb[1]) - ab[0].max(bb[0])) / span_x;
    let oy = (ab[3].min(bb[3]) - ab[2].max(bb[2])) / span_y;
    if ox <= 0.0 && oy <= 0.0 {
        return None;
    }
    // Extents apart along an axis share no area: no clipping needed, and
    // the answer is the same as clipping would give. Most pairs of a large
    // mosaic end here.
    let shared = if ox <= 0.0 || oy <= 0.0 {
        Vec::new()
    } else {
        intersection(&a.corners, &b.corners)
    };
    // Where they intersect, a turned outline makes the shared strip wider at
    // one end than the other; the strip's mean width across the seam (its
    // area over its length) is what the share counts.
    let mut mean_width = None;
    let (across_x, across_y) = if shared.is_empty() {
        // Outlines that miss each other although their extents overlap on
        // both axes (turned corners passing by): the gap is the distance
        // between the outlines, across the axis it mostly runs along.
        if ox > 0.0 && oy > 0.0 {
            let (distance, direction) = separation(&a.corners, &b.corners);
            if direction[0].abs() >= direction[1].abs() {
                (-distance / span_x, oy)
            } else {
                (ox, -distance / span_y)
            }
        } else {
            (ox, oy)
        }
    } else {
        let sb = bounds(&shared);
        let area = signed_area(&shared).abs() / 2.0;
        mean_width = Some((
            area / (sb[3] - sb[2]) / span_x,
            area / (sb[1] - sb[0]) / span_y,
        ));
        ((sb[1] - sb[0]) / span_x, (sb[3] - sb[2]) / span_y)
    };
    let vertical_seam = across_x <= across_y;
    let (thickness, run) = match (vertical_seam, mean_width) {
        (true, Some((width, _))) => (width, across_y),
        (false, Some((_, height))) => (height, across_x),
        (true, None) => (across_x, across_y),
        (false, None) => (across_y, across_x),
    };
    Some(ShapeOverlap {
        thickness,
        run,
        vertical_seam,
    })
}

/// The shortest distance between two convex outlines that do not overlap,
/// and its direction from `a` toward `b`: the closest of every corner to
/// every edge, both ways.
fn separation(a: &[Point], b: &[Point]) -> (f64, Point) {
    let mut best = (f64::INFINITY, [0.0, 0.0]);
    let mut closest = |p: Point, from: &[Point], sign: f64| {
        for i in 0..from.len() {
            let (s, e) = (from[i], from[(i + 1) % from.len()]);
            let (dx, dy) = (e[0] - s[0], e[1] - s[1]);
            let length = dx * dx + dy * dy;
            let t = if length > 0.0 {
                (((p[0] - s[0]) * dx + (p[1] - s[1]) * dy) / length).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = [s[0] + t * dx, s[1] + t * dy];
            let distance = (p[0] - q[0]).hypot(p[1] - q[1]);
            if distance < best.0 {
                // From the corner's outline toward the other's edge.
                best = (distance, [sign * (q[0] - p[0]), sign * (q[1] - p[1])]);
            }
        }
    };
    for corner in a {
        closest(*corner, b, 1.0);
    }
    for corner in b {
        closest(*corner, a, -1.0);
    }
    best
}

fn round_percent(share: f64) -> f64 {
    (share * 1000.0).round() / 10.0
}

/// Where panels fall short of the minimum overlap rule. Two panels are
/// joined when they overlap by at least the minimum, side by side or at a
/// corner. Two panels sit side by side when they share at least half a side
/// along their seam; such a pair that overlaps by less than the minimum
/// across the seam is a thin overlap, and one with uncovered sky between
/// them, narrower than a panel, is a gap. A corner that overlaps by less is
/// no seam and is left alone. Panels joined to nobody are isolated, and
/// groups that never meet are disconnected (said only when nothing else
/// explains it). Shares are of the larger panel's side, so each panel of a
/// pair shares at least the minimum of its own side.
pub fn shape_issues(shapes: &[PanelShape], min_overlap_percent: u32) -> Vec<LayoutIssue> {
    let minimum = f64::from(min_overlap_percent) / 100.0;
    let n = shapes.len();
    let mut issues = Vec::new();
    if n < 2 {
        return issues;
    }
    let mut linked = vec![Vec::new(); n];
    let mut flagged = vec![false; n];
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (&shapes[i], &shapes[j]);
            let Some(pair) = shape_overlap(a, b) else {
                continue;
            };
            if pair.thickness >= -PLACE_TOLERANCE && pair.thickness + PLACE_TOLERANCE >= minimum {
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
            } else if -pair.thickness < 1.0 && !gap_covered(shapes, i, j, pair) {
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
                panels: vec![shapes[i].id.clone()],
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
                    .map(|group| shapes[group[0]].id.clone())
                    .collect(),
                percent: None,
            });
        }
    }
    issues
}

/// Whether other panels cover the sky between panels `i` and `j`, which
/// face each other across a gap: sampled at points strictly inside the gap,
/// which is enough at the sizes a person drags to.
fn gap_covered(shapes: &[PanelShape], i: usize, j: usize, pair: ShapeOverlap) -> bool {
    let (ab, bb) = (shapes[i].bounds(), shapes[j].bounds());
    // Across the seam: from the nearer edge of one to the other. Along it:
    // the stretch both panels span.
    let (across_a, across_b, along_a, along_b) = if pair.vertical_seam {
        (
            (ab[0], ab[1]),
            (bb[0], bb[1]),
            (ab[2], ab[3]),
            (bb[2], bb[3]),
        )
    } else {
        (
            (ab[2], ab[3]),
            (bb[2], bb[3]),
            (ab[0], ab[1]),
            (bb[0], bb[1]),
        )
    };
    let (low, high) = if across_a.0 + across_a.1 < across_b.0 + across_b.1 {
        (across_a.1, across_b.0)
    } else {
        (across_b.1, across_a.0)
    };
    let start = along_a.0.max(along_b.0);
    let end = along_a.1.min(along_b.1);
    // Only a panel reaching into the gap can cover a sample in it, so the
    // rest are not tried at every sample: a large mosaic has hundreds.
    let (across_low, across_high) = (low.min(high), low.max(high));
    let (along_low, along_high) = (start.min(end), start.max(end));
    let region = if pair.vertical_seam {
        [across_low, across_high, along_low, along_high]
    } else {
        [along_low, along_high, across_low, across_high]
    };
    let near: Vec<&PanelShape> = shapes
        .iter()
        .enumerate()
        .filter(|&(k, shape)| {
            let b = shape.bounds();
            let t = PLACE_TOLERANCE * (b[1] - b[0]).max(b[3] - b[2]);
            k != i
                && k != j
                && b[0] - t <= region[1]
                && b[1] + t >= region[0]
                && b[2] - t <= region[3]
                && b[3] + t >= region[2]
        })
        .map(|(_, shape)| shape)
        .collect();
    const ACROSS: usize = 3;
    const ALONG: usize = 5;
    for s in 0..ACROSS {
        let across = low + (high - low) * (s as f64 + 0.5) / ACROSS as f64;
        for t in 0..ALONG {
            let along = start + (end - start) * (t as f64 + 0.5) / ALONG as f64;
            let point = if pair.vertical_seam {
                [across, along]
            } else {
                [along, across]
            };
            let covered = near.iter().any(|shape| grown_contains(shape, point));
            if !covered {
                return false;
            }
        }
    }
    true
}

/// Inside the shape or within the place tolerance of it.
fn grown_contains(shape: &PanelShape, point: Point) -> bool {
    if shape.contains(point) {
        return true;
    }
    let b = shape.bounds();
    let tolerance = PLACE_TOLERANCE * (b[1] - b[0]).max(b[3] - b[2]);
    [[-1.0, 0.0], [1.0, 0.0], [0.0, -1.0], [0.0, 1.0]]
        .iter()
        .any(|d| shape.contains([point[0] + d[0] * tolerance, point[1] + d[1] * tolerance]))
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

    fn turned(id: &str, center: Point, width: f64, height: f64, degrees: f64) -> PanelShape {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let mut shape = PanelShape::rectangle(id, [0.0, 0.0], width, height);
        for corner in &mut shape.corners {
            let [x, y] = *corner;
            *corner = [center[0] + x * cos - y * sin, center[1] + x * sin + y * cos];
        }
        shape
    }

    #[test]
    fn two_squares_clip_to_their_shared_strip() {
        let a = PanelShape::rectangle("a", [0.0, 0.0], 1.0, 1.0);
        let b = PanelShape::rectangle("b", [0.8, 0.0], 1.0, 1.0);
        let shared = intersection(&a.corners, &b.corners);
        let box_ = bounds(&shared);
        assert!((box_[1] - box_[0] - 0.2).abs() < 1e-12 && (box_[3] - box_[2] - 1.0).abs() < 1e-12);
        // Corner order does not matter.
        let mut reversed = b.corners;
        reversed.reverse();
        assert_eq!(intersection(&a.corners, &reversed).len(), shared.len());
        let far = PanelShape::rectangle("c", [3.0, 0.0], 1.0, 1.0);
        assert!(intersection(&a.corners, &far.corners).is_empty());
        assert!(a.contains([0.5, 0.5]) && !a.contains([0.51, 0.0]));
    }

    #[test]
    fn panels_of_one_size_read_as_before() {
        let a = PanelShape::rectangle("a", [0.0, 0.0], 1.0, 1.0);
        for (x, y, thickness, run) in [
            (0.8, 0.0, 0.2, 1.0),
            (0.9, 0.1, 0.1, 0.9),
            (0.8, 0.5, 0.2, 0.5),
            (1.25, 0.0, -0.25, 1.0),
        ] {
            let pair = shape_overlap(&a, &PanelShape::rectangle("b", [x, y], 1.0, 1.0)).unwrap();
            assert!(
                (pair.thickness - thickness).abs() < 1e-12,
                "{x} {y}: {pair:?}"
            );
            assert!((pair.run - run).abs() < 1e-12, "{x} {y}: {pair:?}");
            assert!(pair.vertical_seam);
        }
    }

    #[test]
    fn a_larger_shot_panel_shares_its_seam_with_a_smaller_new_one() {
        // A shot panel 2° by 1.4°, a new one 1.8° by 1.2° to its right. Each
        // should share 20% of its own width: 20% of the larger, 0.4°.
        let shot = PanelShape::rectangle("r1c1", [0.0, 0.0], 2.0, 1.4);
        let new = PanelShape::rectangle("p2", [1.0 + 0.9 - 0.4, 0.0], 1.8, 1.2);
        let pair = shape_overlap(&shot, &new).unwrap();
        assert!((pair.thickness - 0.2).abs() < 1e-12, "{pair:?}");
        assert!((pair.run - 1.2 / 1.4).abs() < 1e-12, "{pair:?}");
        assert!(shape_issues(&[shot.clone(), new], 20).is_empty());
        // 0.3° is 15% of the larger width: thin.
        let thin = PanelShape::rectangle("p2", [1.0 + 0.9 - 0.3, 0.0], 1.8, 1.2);
        let issues = shape_issues(&[shot, thin], 20);
        assert_eq!(issues[0].kind, LayoutIssueKind::ThinOverlap);
        assert_eq!(issues[0].percent, Some(15.0));
    }

    #[test]
    fn a_slightly_turned_panel_is_read_by_its_true_outline() {
        // Shot two degrees off the camera axes: the strip it shares with a
        // neighbour is wider at one end than the other, and its mean width
        // counts, over the turned panel's wider reach.
        let shot = turned("r1c1", [0.0, 0.0], 1.0, 1.0, 2.0);
        let new = PanelShape::rectangle("p2", [0.8, 0.0], 1.0, 1.0);
        let pair = shape_overlap(&shot, &new).unwrap();
        assert!(pair.vertical_seam);
        // The turned square's right edge crosses x = 0.3 + w(y): its mean
        // over the strip is the straight edge's reach, 0.5 / cos 2°.
        let reach = 0.5 * (2f64.to_radians().cos() + 2f64.to_radians().sin());
        let mean = (0.5 / 2f64.to_radians().cos() - 0.3) / (2.0 * reach);
        assert!(
            (pair.thickness - mean).abs() < 0.005,
            "{pair:?} against {mean}"
        );
        assert!(pair.thickness < (reach - 0.3) / (2.0 * reach));
    }

    #[test]
    fn a_gap_is_one_only_where_no_panel_covers_it() {
        let a = PanelShape::rectangle("a", [-0.8, 0.0], 1.0, 1.0);
        let b = PanelShape::rectangle("b", [0.8, 0.0], 1.0, 1.0);
        let middle = PanelShape::rectangle("m", [0.0, 0.0], 1.0, 1.0);
        assert!(shape_issues(&[a.clone(), middle, b.clone()], 20).is_empty());
        let issues = shape_issues(&[a, b], 20);
        assert_eq!(issues[0].kind, LayoutIssueKind::Gap);
        assert_eq!(issues[0].percent, Some(60.0));
    }
}
