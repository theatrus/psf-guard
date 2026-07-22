//! Canny edge detection, ported from OpenCV's scalar implementation
//! (`modules/imgproc/src/canny.cpp`).
//!
//! Matches `cv::Canny` with `apertureSize = 3` and `L2gradient = false`:
//! Sobel gradients with replicated borders, L1 magnitude, the TG22
//! fixed-point sector logic for non-maximum suppression (including its exact
//! `>` / `>=` tie-breaking), and stack-based hysteresis over 8-neighbors.
//! The Sobel and magnitude stages are separable slice loops that
//! auto-vectorize; NMS is inherently branchy and stays scalar.

const CANNY_SHIFT: i32 = 15;
/// tan(22.5 deg) in 15-bit fixed point, as in OpenCV.
const TG22: i32 = 13573;

/// Sobel 3x3 derivatives with `BORDER_REPLICATE` (the border OpenCV's Canny
/// uses internally). Values fit i16 but are produced as i32 for the NMS
/// arithmetic.
fn sobel3(src: &[u8], width: usize, height: usize) -> (Vec<i32>, Vec<i32>) {
    let mut dx = vec![0i32; width * height];
    let mut dy = vec![0i32; width * height];
    let mut smooth = vec![0i32; width]; // [1 2 1] vertical for dx
    let mut diff = vec![0i32; width]; // [-1 0 1] vertical for dy

    for y in 0..height {
        let prev = &src[y.saturating_sub(1) * width..y.saturating_sub(1) * width + width];
        let cur = &src[y * width..y * width + width];
        let nrow = (y + 1).min(height - 1);
        let next = &src[nrow * width..nrow * width + width];

        // Vertical taps, elementwise over rows (vectorizes).
        for x in 0..width {
            let p = prev[x] as i32;
            let c = cur[x] as i32;
            let n = next[x] as i32;
            smooth[x] = p + 2 * c + n;
            diff[x] = n - p;
        }

        // Horizontal taps with replicated columns.
        let drow = &mut dx[y * width..(y + 1) * width];
        let erow = &mut dy[y * width..(y + 1) * width];
        if width >= 3 {
            for x in 1..width - 1 {
                drow[x] = smooth[x + 1] - smooth[x - 1];
                erow[x] = diff[x - 1] + 2 * diff[x] + diff[x + 1];
            }
        }
        let last = width - 1;
        drow[0] = smooth[1.min(last)] - smooth[0];
        drow[last] = smooth[last] - smooth[last.saturating_sub(1)];
        erow[0] = diff[0] + 2 * diff[0] + diff[1.min(last)];
        erow[last] = diff[last.saturating_sub(1)] + 2 * diff[last] + diff[last];
    }
    (dx, dy)
}

/// Canny edge detector. `low`/`high` are the hysteresis thresholds applied
/// to the L1 gradient magnitude. Returns a 0/255 edge map.
pub fn canny(src: &[u8], width: usize, height: usize, low: i32, high: i32) -> Vec<u8> {
    assert_eq!(src.len(), width * height);
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let (dx, dy) = sobel3(src, width, height);

    // Magnitude rows padded by one zero column on each side, plus zero rows
    // above and below the image, exactly like OpenCV's mag buffer layout.
    let mstep = width + 2;
    let mut mag = vec![0i32; mstep * (height + 2)];
    for y in 0..height {
        let mrow = &mut mag[(y + 1) * mstep + 1..(y + 1) * mstep + 1 + width];
        let dxr = &dx[y * width..(y + 1) * width];
        let dyr = &dy[y * width..(y + 1) * width];
        for ((m, &gx), &gy) in mrow.iter_mut().zip(dxr.iter()).zip(dyr.iter()) {
            *m = gx.abs() + gy.abs();
        }
    }

    // Edge map with a one-pixel border marked "not edge" (1).
    // 0 = weak candidate, 1 = not an edge, 2 = strong edge.
    let mut map = vec![0u8; mstep * (height + 2)];
    for x in 0..mstep {
        map[x] = 1;
        map[(height + 1) * mstep + x] = 1;
    }
    for y in 0..height + 2 {
        map[y * mstep] = 1;
        map[y * mstep + mstep - 1] = 1;
    }

    let mut stack: Vec<usize> = Vec::with_capacity(1024);

    for y in 0..height {
        let mrow = (y + 1) * mstep;
        for x in 0..width {
            let j = x + 1;
            let m = mag[mrow + j];
            let mut is_local_max = false;
            if m > low {
                let xs = dx[y * width + x];
                let ys = dy[y * width + x];
                let ax = xs.abs();
                let ay = ys.abs() << CANNY_SHIFT;
                let tg22x = ax * TG22;
                if ay < tg22x {
                    // Roughly horizontal gradient: compare left/right.
                    if m > mag[mrow + j - 1] && m >= mag[mrow + j + 1] {
                        is_local_max = true;
                    }
                } else {
                    let tg67x = tg22x + ((ax + ax) << CANNY_SHIFT);
                    if ay > tg67x {
                        // Roughly vertical: compare up/down.
                        if m > mag[mrow - mstep + j] && m >= mag[mrow + mstep + j] {
                            is_local_max = true;
                        }
                    } else {
                        // Diagonal.
                        let s: isize = if (xs ^ ys) < 0 { -1 } else { 1 };
                        let up = (mrow - mstep + j) as isize - s;
                        let down = (mrow + mstep + j) as isize + s;
                        if m > mag[up as usize] && m > mag[down as usize] {
                            is_local_max = true;
                        }
                    }
                }
            }
            let idx = mrow + j;
            if is_local_max {
                if m > high {
                    map[idx] = 2;
                    stack.push(idx);
                } else {
                    map[idx] = 0;
                }
            } else {
                map[idx] = 1;
            }
        }
    }

    // Hysteresis: grow strong edges into connected weak candidates.
    while let Some(idx) = stack.pop() {
        let neighbors = [
            idx - mstep - 1,
            idx - mstep,
            idx - mstep + 1,
            idx - 1,
            idx + 1,
            idx + mstep - 1,
            idx + mstep,
            idx + mstep + 1,
        ];
        for n in neighbors {
            if map[n] == 0 {
                map[n] = 2;
                stack.push(n);
            }
        }
    }

    let mut out = vec![0u8; width * height];
    for y in 0..height {
        let mrow = &map[(y + 1) * mstep + 1..(y + 1) * mstep + 1 + width];
        let orow = &mut out[y * width..(y + 1) * width];
        for (o, &m) in orow.iter_mut().zip(mrow.iter()) {
            *o = if m == 2 { 255 } else { 0 };
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_image_has_no_edges() {
        let img = vec![100u8; 100];
        let out = canny(&img, 10, 10, 10, 80);
        assert!(out.iter().all(|&v| v == 0));
    }

    #[test]
    fn step_edge_detected() {
        // Vertical step: left half 0, right half 200.
        let w = 16;
        let h = 8;
        let mut img = vec![0u8; w * h];
        for y in 0..h {
            for x in 8..w {
                img[y * w + x] = 200;
            }
        }
        let out = canny(&img, w, h, 10, 80);
        // An edge column exists near x = 7..8.
        let edge_cols: Vec<usize> = (0..w)
            .filter(|&x| (0..h).any(|y| out[y * w + x] == 255))
            .collect();
        assert!(!edge_cols.is_empty());
        assert!(edge_cols.iter().all(|&x| (7..=8).contains(&x)));
    }

    #[test]
    fn sobel_gradient_signs() {
        // Bright pixel right of center: dx positive at center.
        let img = vec![0, 0, 0, 0, 0, 10, 0, 0, 0];
        let (dx, dy) = sobel3(&img, 3, 3);
        assert!(dx[4] > 0);
        assert_eq!(dy[4], 0);
    }

    #[test]
    fn sobel_matches_naive() {
        let w = 13;
        let h = 9;
        let mut state = 42u64;
        let img: Vec<u8> = (0..w * h)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state >> 56) as u8
            })
            .collect();
        let (dx, dy) = sobel3(&img, w, h);
        let at = |y: isize, x: isize| -> i32 {
            let y = y.clamp(0, h as isize - 1) as usize;
            let x = x.clamp(0, w as isize - 1) as usize;
            img[y * w + x] as i32
        };
        for y in 0..h as isize {
            for x in 0..w as isize {
                let ex = (at(y - 1, x + 1) + 2 * at(y, x + 1) + at(y + 1, x + 1))
                    - (at(y - 1, x - 1) + 2 * at(y, x - 1) + at(y + 1, x - 1));
                let ey = (at(y + 1, x - 1) + 2 * at(y + 1, x) + at(y + 1, x + 1))
                    - (at(y - 1, x - 1) + 2 * at(y - 1, x) + at(y - 1, x + 1));
                let i = y as usize * w + x as usize;
                assert_eq!(dx[i], ex, "dx mismatch at ({x},{y})");
                assert_eq!(dy[i], ey, "dy mismatch at ({x},{y})");
            }
        }
    }
}
