//! Binary erosion and dilation with OpenCV-compatible structuring elements
//! and border semantics.

use crate::border::BorderMode;

/// Structuring element shape, as in OpenCV's `getStructuringElement`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelShape {
    Rect,
    Ellipse,
    Cross,
}

/// A structuring element with its anchor at the center.
#[derive(Debug, Clone)]
pub struct StructuringElement {
    pub width: usize,
    pub height: usize,
    pub mask: Vec<bool>,
}

impl StructuringElement {
    /// Build a `ksize x ksize` element. The ellipse rasterization is ported
    /// from OpenCV's `getStructuringElement` (a 3x3 ellipse is a cross).
    pub fn new(shape: KernelShape, ksize: usize) -> Self {
        assert!(ksize >= 1);
        let mut mask = vec![false; ksize * ksize];
        let r = ksize / 2;
        let c = ksize / 2;
        match shape {
            KernelShape::Rect => mask.fill(true),
            KernelShape::Cross => {
                for i in 0..ksize {
                    mask[r * ksize + i] = true;
                    mask[i * ksize + c] = true;
                }
            }
            KernelShape::Ellipse => {
                let inv_r2 = if r > 0 { 1.0 / (r * r) as f64 } else { 0.0 };
                for i in 0..ksize {
                    let dy = i as isize - r as isize;
                    if dy.unsigned_abs() <= r {
                        let dx = (c as f64 * (((r * r) as f64 - (dy * dy) as f64) * inv_r2).sqrt())
                            .round_ties_even() as usize;
                        let j1 = c.saturating_sub(dx);
                        let j2 = (c + dx + 1).min(ksize);
                        for j in j1..j2 {
                            mask[i * ksize + j] = true;
                        }
                    }
                }
            }
        }
        Self {
            width: ksize,
            height: ksize,
            mask,
        }
    }

    fn offsets(&self) -> Vec<(isize, isize)> {
        let ar = (self.height / 2) as isize;
        let ac = (self.width / 2) as isize;
        let mut offs = Vec::new();
        for i in 0..self.height {
            for j in 0..self.width {
                if self.mask[i * self.width + j] {
                    offs.push((i as isize - ar, j as isize - ac));
                }
            }
        }
        offs
    }
}

/// Border handling for morphology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MorphBorder {
    /// OpenCV's `BORDER_CONSTANT` with `morphologyDefaultBorderValue()`:
    /// outside pixels never win the min/max, i.e. they are ignored.
    Ignore,
    Replicate,
    Reflect,
    Reflect101,
}

impl MorphBorder {
    fn as_mode(self) -> Option<BorderMode> {
        match self {
            MorphBorder::Ignore => None,
            MorphBorder::Replicate => Some(BorderMode::Replicate),
            MorphBorder::Reflect => Some(BorderMode::Reflect),
            MorphBorder::Reflect101 => Some(BorderMode::Reflect101),
        }
    }
}

/// Grayscale/binary dilation: max over the structuring element.
pub fn dilate(
    src: &[u8],
    width: usize,
    height: usize,
    se: &StructuringElement,
    border: MorphBorder,
) -> Vec<u8> {
    morph(src, width, height, se, border, true)
}

/// Grayscale/binary erosion: min over the structuring element.
pub fn erode(
    src: &[u8],
    width: usize,
    height: usize,
    se: &StructuringElement,
    border: MorphBorder,
) -> Vec<u8> {
    morph(src, width, height, se, border, false)
}

fn morph(
    src: &[u8],
    width: usize,
    height: usize,
    se: &StructuringElement,
    border: MorphBorder,
    is_dilate: bool,
) -> Vec<u8> {
    assert_eq!(src.len(), width * height);
    let offs = se.offsets();
    let mode = border.as_mode();
    let mut out = vec![0u8; width * height];

    // Interior fast path: every kernel offset stays in bounds, so each
    // offset is one elementwise min/max over shifted row slices (compiles
    // to packed u8 min/max).
    let ar = se.height / 2;
    let ac = se.width / 2;
    if height > 2 * ar && width > 2 * ac && !offs.is_empty() {
        let ilen = width - 2 * ac;
        for y in ar..height - ar {
            let (first_dy, first_dx) = offs[0];
            let sy = (y as isize + first_dy) as usize;
            let sx = (ac as isize + first_dx) as usize;
            out[y * width + ac..y * width + ac + ilen]
                .copy_from_slice(&src[sy * width + sx..sy * width + sx + ilen]);
            for &(dy, dx) in &offs[1..] {
                let sy = (y as isize + dy) as usize;
                let sx = (ac as isize + dx) as usize;
                let srow = &src[sy * width + sx..sy * width + sx + ilen];
                let orow = &mut out[y * width + ac..y * width + ac + ilen];
                if is_dilate {
                    for (o, &s) in orow.iter_mut().zip(srow.iter()) {
                        *o = (*o).max(s);
                    }
                } else {
                    for (o, &s) in orow.iter_mut().zip(srow.iter()) {
                        *o = (*o).min(s);
                    }
                }
            }
        }
    }

    // Border ring (and degenerate sizes): scalar with border mapping.
    let interior_y = |y: usize| height > 2 * ar && y >= ar && y < height - ar;
    let interior_x = |x: usize| width > 2 * ac && x >= ac && x < width - ac;
    for y in 0..height {
        for x in 0..width {
            if interior_y(y) && interior_x(x) {
                continue;
            }
            let mut acc: Option<u8> = None;
            for &(dy, dx) in &offs {
                let sy = y as isize + dy;
                let sx = x as isize + dx;
                let v = if sy >= 0 && sy < height as isize && sx >= 0 && sx < width as isize {
                    src[sy as usize * width + sx as usize]
                } else if let Some(m) = mode {
                    src[m.map(sy, height) * width + m.map(sx, width)]
                } else {
                    continue; // Ignore border: outside pixels never contribute
                };
                acc = Some(match acc {
                    None => v,
                    Some(a) => {
                        if is_dilate {
                            a.max(v)
                        } else {
                            a.min(v)
                        }
                    }
                });
            }
            out[y * width + x] = acc.unwrap_or(src[y * width + x]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipse_3x3_is_cross() {
        let se = StructuringElement::new(KernelShape::Ellipse, 3);
        let cross = StructuringElement::new(KernelShape::Cross, 3);
        assert_eq!(se.mask, cross.mask);
    }

    #[test]
    fn ellipse_5x5_shape() {
        let se = StructuringElement::new(KernelShape::Ellipse, 5);
        // OpenCV's 5x5 ellipse: rows 1..=3 full, rows 0 and 4 are 5 wide?
        // getStructuringElement gives: row 0 -> dy=-2, dx=0 -> only center;
        // row 1 -> dy=-1, dx = 2*sqrt(3)/2 = 1.73.. rounds to 2 -> full row.
        let rows: Vec<usize> = (0..5)
            .map(|i| (0..5).filter(|&j| se.mask[i * 5 + j]).count())
            .collect();
        assert_eq!(rows, vec![1, 5, 5, 5, 1]);
    }

    #[test]
    fn dilate_single_pixel_rect3() {
        let mut img = vec![0u8; 25];
        img[12] = 255;
        let se = StructuringElement::new(KernelShape::Rect, 3);
        let out = dilate(&img, 5, 5, &se, MorphBorder::Ignore);
        // 3x3 block around center is set
        for dy in -1isize..=1 {
            for dx in -1isize..=1 {
                assert_eq!(out[((2 + dy) * 5 + (2 + dx)) as usize], 255);
            }
        }
        assert_eq!(out.iter().filter(|&&v| v == 255).count(), 9);
    }

    #[test]
    fn erode_removes_single_pixel() {
        let mut img = vec![0u8; 25];
        img[12] = 255;
        let se = StructuringElement::new(KernelShape::Ellipse, 3);
        let out = erode(&img, 5, 5, &se, MorphBorder::Reflect);
        assert!(out.iter().all(|&v| v == 0));
    }

    #[test]
    fn fast_path_matches_naive_on_random_data() {
        let w = 21;
        let h = 15;
        let mut state = 7u64;
        let img: Vec<u8> = (0..w * h)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                if (state >> 60) > 7 {
                    255
                } else {
                    0
                }
            })
            .collect();
        for shape in [KernelShape::Rect, KernelShape::Ellipse, KernelShape::Cross] {
            for ksize in [3usize, 5] {
                let se = StructuringElement::new(shape, ksize);
                let offs = se.offsets();
                for border in [
                    MorphBorder::Ignore,
                    MorphBorder::Reflect,
                    MorphBorder::Replicate,
                ] {
                    for dil in [true, false] {
                        let fast = if dil {
                            dilate(&img, w, h, &se, border)
                        } else {
                            erode(&img, w, h, &se, border)
                        };
                        // Naive reference.
                        for y in 0..h {
                            for x in 0..w {
                                let mut acc: Option<u8> = None;
                                for &(dy, dx) in &offs {
                                    let sy = y as isize + dy;
                                    let sx = x as isize + dx;
                                    let v =
                                        if sy >= 0 && sy < h as isize && sx >= 0 && sx < w as isize
                                        {
                                            img[sy as usize * w + sx as usize]
                                        } else if let Some(m) = border.as_mode() {
                                            img[m.map(sy, h) * w + m.map(sx, w)]
                                        } else {
                                            continue;
                                        };
                                    acc = Some(match acc {
                                        None => v,
                                        Some(a) => {
                                            if dil {
                                                a.max(v)
                                            } else {
                                                a.min(v)
                                            }
                                        }
                                    });
                                }
                                let expect = acc.unwrap_or(img[y * w + x]);
                                assert_eq!(
                                    fast[y * w + x],
                                    expect,
                                    "{shape:?} k{ksize} {border:?} dil={dil} at ({x},{y})"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn erode_border_reflect_keeps_solid_image() {
        let img = vec![255u8; 25];
        let se = StructuringElement::new(KernelShape::Ellipse, 3);
        let out = erode(&img, 5, 5, &se, MorphBorder::Reflect);
        assert!(out.iter().all(|&v| v == 255));
    }
}
