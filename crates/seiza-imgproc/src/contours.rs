//! External contour extraction and contour metrics.
//!
//! [`find_external_contours`] implements Suzuki-Abe border following (the
//! algorithm behind OpenCV's `findContours`) with full hierarchy
//! bookkeeping, returning only the extreme outer contours — the equivalent
//! of `RETR_EXTERNAL` with `CHAIN_APPROX_SIMPLE`. The metric helpers match
//! how OpenCV computes them for contours: polygon (Green's theorem) area and
//! moments over the pixel-center polygon, not pixel counts.

/// A contour point in `(x, y)` order, like `cv::Point`.
pub type Point = (i32, i32);

/// 8-neighborhood in counterclockwise order starting East, `(di, dj)` with
/// `i` = row and `j` = column.
const NBR: [(isize, isize); 8] = [
    (0, 1),   // 0: E
    (-1, 1),  // 1: NE
    (-1, 0),  // 2: N
    (-1, -1), // 3: NW
    (0, -1),  // 4: W
    (1, -1),  // 5: SW
    (1, 0),   // 6: S
    (1, 1),   // 7: SE
];

fn nbr_index(di: isize, dj: isize) -> usize {
    match (di, dj) {
        (0, 1) => 0,
        (-1, 1) => 1,
        (-1, 0) => 2,
        (-1, -1) => 3,
        (0, -1) => 4,
        (1, -1) => 5,
        (1, 0) => 6,
        (1, 1) => 7,
        _ => unreachable!("not an 8-neighbor offset"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BorderType {
    Outer,
    Hole,
}

/// Find the extreme outer contours of all non-zero regions, 8-connected.
///
/// Equivalent to `cv::findContours(img, RETR_EXTERNAL, CHAIN_APPROX_SIMPLE)`:
/// nested blobs inside holes of other blobs are not reported, thin
/// structures produce contours that revisit pixels, and straight runs are
/// compressed to their endpoints.
pub fn find_external_contours(src: &[u8], width: usize, height: usize) -> Vec<Vec<Point>> {
    assert_eq!(src.len(), width * height);
    if width == 0 || height == 0 {
        return Vec::new();
    }

    // Work image padded with a one-pixel zero frame, i32 labels.
    let pw = width + 2;
    let ph = height + 2;
    let mut f = vec![0i32; pw * ph];
    for y in 0..height {
        for x in 0..width {
            if src[y * width + x] != 0 {
                f[(y + 1) * pw + (x + 1)] = 1;
            }
        }
    }

    // Border bookkeeping. Index = border number (NBD). Entry 1 is the frame.
    let mut border_type: Vec<BorderType> = vec![BorderType::Hole, BorderType::Hole];
    let mut parent: Vec<usize> = vec![0, 0];
    let mut contours: Vec<Option<Vec<Point>>> = vec![None, None];
    let mut nbd: usize = 1;

    for i in 1..=height {
        let mut lnbd: usize = 1;
        for j in 1..=width {
            let fij = f[i * pw + j];
            if fij == 0 {
                continue;
            }

            let mut from: Option<(usize, usize, BorderType)> = None;
            if fij == 1 && f[i * pw + j - 1] == 0 {
                from = Some((i, j.wrapping_sub(1), BorderType::Outer));
            } else if fij >= 1 && f[i * pw + j + 1] == 0 {
                from = Some((i, j + 1, BorderType::Hole));
                if fij > 1 {
                    lnbd = fij as usize;
                }
            }

            if let Some((i2, j2, btype)) = from {
                nbd += 1;
                // Parent from Suzuki-Abe's decision table.
                let b_prime = lnbd;
                let par = match (btype, border_type[b_prime]) {
                    (BorderType::Outer, BorderType::Outer) => parent[b_prime],
                    (BorderType::Outer, BorderType::Hole) => b_prime,
                    (BorderType::Hole, BorderType::Outer) => b_prime,
                    (BorderType::Hole, BorderType::Hole) => parent[b_prime],
                };
                border_type.push(btype);
                parent.push(par);

                let keep_points = btype == BorderType::Outer && par == 1;
                let pts = trace_border(&mut f, pw, (i, j), (i2, j2), nbd as i32, keep_points);
                contours.push(pts);
            }

            let fij = f[i * pw + j];
            if fij != 1 {
                lnbd = fij.unsigned_abs() as usize;
            }
        }
    }

    contours
        .into_iter()
        .flatten()
        .map(|pts| compress_simple(&pts))
        .collect()
}

/// Follow one border (Suzuki-Abe step 3), marking pixels in `f`. Returns
/// the traced points (image coordinates, `(x, y)`) when `keep_points`.
fn trace_border(
    f: &mut [i32],
    pw: usize,
    start: (usize, usize),
    from: (usize, usize),
    nbd: i32,
    keep_points: bool,
) -> Option<Vec<Point>> {
    let (si, sj) = start;
    let to_point = |i: usize, j: usize| -> Point { (j as i32 - 1, i as i32 - 1) };
    let mut points: Vec<Point> = Vec::new();

    // (3.1) Clockwise from `from` around `start`, find the first non-zero.
    let start_idx = nbr_index(from.0 as isize - si as isize, from.1 as isize - sj as isize);
    let mut i1: Option<(usize, usize)> = None;
    for k in 0..8 {
        let idx = (start_idx + 8 - k) % 8;
        let (di, dj) = NBR[idx];
        let (pi, pj) = ((si as isize + di) as usize, (sj as isize + dj) as usize);
        if f[pi * pw + pj] != 0 {
            i1 = Some((pi, pj));
            break;
        }
    }
    let Some(i1) = i1 else {
        // Isolated pixel.
        f[si * pw + sj] = -nbd;
        return keep_points.then(|| vec![to_point(si, sj)]);
    };

    // (3.2)
    let mut i2 = i1;
    let mut i3 = (si, sj);

    loop {
        // (3.3) Counterclockwise from the neighbor after `i2` around `i3`.
        let base = nbr_index(i2.0 as isize - i3.0 as isize, i2.1 as isize - i3.1 as isize);
        let mut examined_east_zero = false;
        let mut i4 = (0usize, 0usize);
        for k in 1..=8 {
            let idx = (base + k) % 8;
            let (di, dj) = NBR[idx];
            let (pi, pj) = ((i3.0 as isize + di) as usize, (i3.1 as isize + dj) as usize);
            if f[pi * pw + pj] != 0 {
                i4 = (pi, pj);
                break;
            }
            if idx == 0 {
                examined_east_zero = true;
            }
        }

        // (3.4) Mark the current border pixel.
        let cur = i3.0 * pw + i3.1;
        if examined_east_zero {
            f[cur] = -nbd;
        } else if f[cur] == 1 {
            f[cur] = nbd;
        }
        if keep_points {
            points.push(to_point(i3.0, i3.1));
        }

        // (3.5) Termination: back at the start, entering the same first pixel.
        if i4 == (si, sj) && i3 == i1 {
            break;
        }
        i2 = i3;
        i3 = i4;
    }

    keep_points.then_some(points)
}

/// `CHAIN_APPROX_SIMPLE`: drop points interior to straight 8-connected runs.
/// The first point is always kept. Geometry (area, perimeter, hull, moments)
/// is unchanged because removed points are exactly collinear.
fn compress_simple(points: &[Point]) -> Vec<Point> {
    let n = points.len();
    if n <= 2 {
        return points.to_vec();
    }
    let dir = |a: Point, b: Point| -> (i32, i32) { ((b.0 - a.0).signum(), (b.1 - a.1).signum()) };
    let mut out = Vec::with_capacity(n);
    out.push(points[0]);
    for k in 1..n {
        let prev = points[k - 1];
        let cur = points[k];
        let next = points[(k + 1) % n];
        if dir(prev, cur) != dir(cur, next) {
            out.push(cur);
        }
    }
    out
}

/// Polygon area of a closed contour (absolute value), as `cv::contourArea`
/// with `oriented = false`.
pub fn contour_area(points: &[Point]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut a00 = 0.0f64;
    let mut prev = *points.last().unwrap();
    for &p in points {
        a00 += prev.0 as f64 * p.1 as f64 - p.0 as f64 * prev.1 as f64;
        prev = p;
    }
    (a00 * 0.5).abs()
}

/// Perimeter of a closed contour, as `cv::arcLength(..., closed = true)`.
pub fn arc_length_closed(points: &[Point]) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    let mut prev = *points.last().unwrap();
    for &p in points {
        let dx = (p.0 - prev.0) as f64;
        let dy = (p.1 - prev.1) as f64;
        sum += (dx * dx + dy * dy).sqrt();
        prev = p;
    }
    sum
}

/// Spatial moments of a contour polygon up to first order, computed the way
/// `cv::moments` does for contour input (Green's theorem with sign
/// normalization so `m00 >= 0`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Moments {
    pub m00: f64,
    pub m10: f64,
    pub m01: f64,
}

pub fn contour_moments(points: &[Point]) -> Moments {
    if points.len() < 3 {
        return Moments::default();
    }
    let (mut a00, mut a10, mut a01) = (0.0f64, 0.0f64, 0.0f64);
    let (mut xi_1, mut yi_1) = {
        let p = *points.last().unwrap();
        (p.0 as f64, p.1 as f64)
    };
    for &p in points {
        let (xi, yi) = (p.0 as f64, p.1 as f64);
        let dxy = xi_1 * yi - xi * yi_1;
        let xii_1 = xi_1 + xi;
        let yii_1 = yi_1 + yi;
        a00 += dxy;
        a10 += dxy * xii_1;
        a01 += dxy * yii_1;
        xi_1 = xi;
        yi_1 = yi;
    }
    if a00.abs() > f32::EPSILON as f64 {
        let (db1_2, db1_6) = if a00 > 0.0 {
            (0.5, 1.0 / 6.0)
        } else {
            (-0.5, -1.0 / 6.0)
        };
        Moments {
            m00: a00 * db1_2,
            m10: a10 * db1_6,
            m01: a01 * db1_6,
        }
    } else {
        Moments::default()
    }
}

/// Convex hull of a point set (monotone chain). Interior collinear points
/// are excluded, like OpenCV's `convexHull`.
pub fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut pts: Vec<Point> = points.to_vec();
    pts.sort_unstable();
    pts.dedup();
    let n = pts.len();
    if n <= 2 {
        return pts;
    }
    let cross = |o: Point, a: Point, b: Point| -> i64 {
        (a.0 - o.0) as i64 * (b.1 - o.1) as i64 - (a.1 - o.1) as i64 * (b.0 - o.0) as i64
    };
    let mut hull: Vec<Point> = Vec::with_capacity(2 * n);
    for &p in pts.iter() {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0 {
            hull.pop();
        }
        hull.push(p);
    }
    let lower_len = hull.len() + 1;
    for &p in pts.iter().rev() {
        while hull.len() >= lower_len && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0 {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop();
    hull
}

/// Tight bounding rectangle `(x, y, width, height)` of integer points, as
/// `cv::boundingRect` (inclusive extents, so width is `max - min + 1`).
pub fn bounding_rect(points: &[Point]) -> (i32, i32, i32, i32) {
    assert!(!points.is_empty());
    let (mut minx, mut miny, mut maxx, mut maxy) =
        (points[0].0, points[0].1, points[0].0, points[0].1);
    for &(x, y) in points.iter().skip(1) {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    }
    (minx, miny, maxx - minx + 1, maxy - miny + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: usize, h: usize, on: &[(usize, usize)]) -> Vec<u8> {
        let mut v = vec![0u8; w * h];
        for &(x, y) in on {
            v[y * w + x] = 255;
        }
        v
    }

    #[test]
    fn empty_image() {
        let v = vec![0u8; 100];
        assert!(find_external_contours(&v, 10, 10).is_empty());
    }

    #[test]
    fn single_pixel() {
        let v = img(5, 5, &[(2, 2)]);
        let cs = find_external_contours(&v, 5, 5);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0], vec![(2, 2)]);
        assert_eq!(contour_area(&cs[0]), 0.0);
        assert_eq!(bounding_rect(&cs[0]), (2, 2, 1, 1));
    }

    #[test]
    fn filled_square_metrics() {
        // 4x4 filled square at (1,1)..(4,4): contour polygon is the 3x3
        // pixel-center ring -> area 9, perimeter 12 (OpenCV semantics).
        let mut v = vec![0u8; 36];
        for y in 1..5 {
            for x in 1..5 {
                v[y * 6 + x] = 255;
            }
        }
        let cs = find_external_contours(&v, 6, 6);
        assert_eq!(cs.len(), 1);
        assert_eq!(contour_area(&cs[0]), 9.0);
        assert_eq!(arc_length_closed(&cs[0]), 12.0);
        assert_eq!(bounding_rect(&cs[0]), (1, 1, 4, 4));
        // CHAIN_APPROX_SIMPLE leaves the four corners.
        assert_eq!(cs[0].len(), 4);
        let m = contour_moments(&cs[0]);
        assert!((m.m00 - 9.0).abs() < 1e-12);
        // Centroid at the square center (2.5, 2.5).
        assert!((m.m10 / m.m00 - 2.5).abs() < 1e-12);
        assert!((m.m01 / m.m00 - 2.5).abs() < 1e-12);
    }

    #[test]
    fn two_blobs() {
        let v = img(10, 5, &[(1, 1), (2, 1), (7, 3), (8, 3), (7, 2)]);
        let cs = find_external_contours(&v, 10, 5);
        assert_eq!(cs.len(), 2);
    }

    #[test]
    fn blob_inside_hole_not_reported() {
        // 7x7 ring (donut) with a lone pixel in its hole center.
        let w = 9;
        let mut v = vec![0u8; w * w];
        for y in 1..8 {
            for x in 1..8 {
                v[y * w + x] = 255;
            }
        }
        for y in 3..6 {
            for x in 3..6 {
                v[y * w + x] = 0;
            }
        }
        v[4 * w + 4] = 255; // isolated pixel inside the hole
        let cs = find_external_contours(&v, w, w);
        // RETR_EXTERNAL: only the donut's outer border.
        assert_eq!(cs.len(), 1);
        assert_eq!(bounding_rect(&cs[0]), (1, 1, 7, 7));
    }

    #[test]
    fn thin_line_contour() {
        // Horizontal 4-pixel line: contour visits endpoints once; area 0,
        // perimeter 2 * 3.
        let v = img(8, 3, &[(2, 1), (3, 1), (4, 1), (5, 1)]);
        let cs = find_external_contours(&v, 8, 3);
        assert_eq!(cs.len(), 1);
        assert_eq!(contour_area(&cs[0]), 0.0);
        assert_eq!(arc_length_closed(&cs[0]), 6.0);
    }

    #[test]
    fn hull_of_l_shape() {
        let pts = vec![(0, 0), (4, 0), (4, 1), (1, 1), (1, 4), (0, 4)];
        let hull = convex_hull(&pts);
        // Extreme points: (0,0), (4,0), (4,1), (1,4), (0,4) — the inner
        // corner (1,1) is dropped.
        assert_eq!(hull.len(), 5);
        assert!(!hull.contains(&(1, 1)));
        assert!((contour_area(&hull) - 11.5).abs() < 1e-9);
    }

    #[test]
    fn blob_touching_border() {
        // Blob touching image edge still produces a contour.
        let mut v = vec![0u8; 25];
        for y in 0..3 {
            for x in 0..3 {
                v[y * 5 + x] = 255;
            }
        }
        let cs = find_external_contours(&v, 5, 5);
        assert_eq!(cs.len(), 1);
        assert_eq!(bounding_rect(&cs[0]), (0, 0, 3, 3));
        assert_eq!(contour_area(&cs[0]), 4.0);
    }
}
