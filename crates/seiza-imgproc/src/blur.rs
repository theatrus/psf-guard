//! Separable Gaussian blur and 3x3 median blur.
//!
//! Kernel derivation and border handling follow OpenCV's `GaussianBlur` and
//! `medianBlur`. The 8-bit path uses OpenCV's bit-exact fixed-point scheme
//! (Q8 kernel, u16 horizontal pass, u32 vertical accumulation with
//! round-half-up), which both matches OpenCV output and keeps the inner
//! loops on integer lanes the compiler auto-vectorizes. All inner loops are
//! written axis-swapped over contiguous slices for that reason; borders are
//! handled by scalar edge code.

use crate::border::BorderMode;

/// 1-D Gaussian kernel matching OpenCV's `getGaussianKernel` for `sigma > 0`:
/// `k[i] = exp(-((i - (n-1)/2)^2) / (2 sigma^2))`, normalized to sum 1.
pub fn gaussian_kernel(n: usize, sigma: f64) -> Vec<f64> {
    assert!(n > 0 && sigma > 0.0);
    let scale_2x = -0.5 / (sigma * sigma);
    let center = (n - 1) as f64 * 0.5;
    let mut kernel: Vec<f64> = (0..n)
        .map(|i| {
            let x = i as f64 - center;
            (scale_2x * x * x).exp()
        })
        .collect();
    let sum: f64 = kernel.iter().sum();
    for k in kernel.iter_mut() {
        *k /= sum;
    }
    kernel
}

/// Kernel size OpenCV derives from sigma for 8-bit images when the caller
/// passes `ksize = 0`: `round(sigma * 3 * 2 + 1) | 1`.
pub fn auto_ksize_u8(sigma: f64) -> usize {
    let k = (sigma * 6.0 + 1.0).round_ties_even() as usize;
    k | 1
}

/// Quantize a normalized kernel to Q8 fixed point the way OpenCV builds its
/// `ufixedpoint16` Gaussian kernels: each tap is the difference of rounded
/// (half-even) cumulative sums. This distributes rounding error across taps
/// and makes the quantized sum exactly 256; verified bit-exact against
/// OpenCV 4.13 for every kernel size the auto-size rule produces.
fn quantize_kernel_q8(kernel: &[f64]) -> Vec<u16> {
    let mut q = Vec::with_capacity(kernel.len());
    let mut cum = 0.0f64;
    let mut prev = 0i64;
    for &k in kernel {
        cum += k * 256.0;
        let r = cum.round_ties_even() as i64;
        q.push((r - prev).max(0) as u16);
        prev = r;
    }
    q
}

/// Separable Gaussian blur on an 8-bit image.
///
/// `ksize = 0` derives the kernel size from sigma (see [`auto_ksize_u8`]).
/// Fixed-point arithmetic reproduces OpenCV's bit-exact `CV_8U` path.
pub fn gaussian_blur_u8(
    src: &[u8],
    width: usize,
    height: usize,
    ksize: usize,
    sigma: f64,
    border: BorderMode,
) -> Vec<u8> {
    let ksize = if ksize == 0 {
        auto_ksize_u8(sigma)
    } else {
        ksize
    };
    assert!(ksize % 2 == 1, "kernel size must be odd");
    let kq = quantize_kernel_q8(&gaussian_kernel(ksize, sigma));
    let radius = ksize / 2;

    // Horizontal pass: u8 -> Q8 u16. Sum of taps is exactly 256, so the
    // worst case 255 * 256 fits u16 with no saturation.
    let mut tmp = vec![0u16; width * height];
    for y in 0..height {
        let row = &src[y * width..(y + 1) * width];
        let out = &mut tmp[y * width..(y + 1) * width];
        if width > 2 * radius {
            // Interior: axis-swapped accumulation over shifted slices; the
            // widening u8 * u16 multiply-add vectorizes.
            let ilen = width - 2 * radius;
            for (i, &kv) in kq.iter().enumerate() {
                let s = &row[i..i + ilen];
                let o = &mut out[radius..radius + ilen];
                for (ov, &sv) in o.iter_mut().zip(s.iter()) {
                    *ov += kv * sv as u16;
                }
            }
        }
        // Borders: scalar with coordinate mapping.
        let edge = radius.min(width);
        for x in (0..edge).chain(width.saturating_sub(radius).max(edge)..width) {
            let mut acc = 0u16;
            for (i, &kv) in kq.iter().enumerate() {
                let sx = border.map(x as isize + i as isize - radius as isize, width);
                acc += kv * row[sx] as u16;
            }
            out[x] = acc;
        }
    }

    // Vertical pass: Q8 u16 rows * Q8 kernel -> Q16 u32, round half up.
    let mut dst = vec![0u8; width * height];
    let mut acc = vec![0u32; width];
    for y in 0..height {
        acc.fill(0);
        let all_interior = y >= radius && y + radius < height;
        for (i, &kv) in kq.iter().enumerate() {
            let sy = if all_interior {
                y + i - radius
            } else {
                border.map(y as isize + i as isize - radius as isize, height)
            };
            let srow = &tmp[sy * width..(sy + 1) * width];
            let kv = kv as u32;
            for (a, &sv) in acc.iter_mut().zip(srow.iter()) {
                *a += kv * sv as u32;
            }
        }
        let drow = &mut dst[y * width..(y + 1) * width];
        for (d, &a) in drow.iter_mut().zip(acc.iter()) {
            *d = (((a + 0x8000) >> 16).min(255)) as u8;
        }
    }
    dst
}

/// Separable Gaussian blur on a 32-bit float image, reproducing OpenCV's
/// `CV_32F` separable filter engine bit-for-bit as built for modern x86-64
/// (AVX2/FMA dispatch, 8-float lanes):
///
/// - The "body" region (`x < width - width % 8`) uses fused multiply-adds
///   with OpenCV's per-kernel-size term order: 3-tap rows accumulate
///   inner pair then center, 5-tap rows pair-center-pair, larger rows
///   sequentially; columns always run center first, then symmetric pairs
///   inner to outer.
/// - The scalar tail columns use the same term shapes without fusion
///   (except the 3-tap column tail, which keeps the body formula).
///
/// FMA is dispatched at runtime on x86-64 (pre-FMA CPUs fall back to
/// unfused arithmetic, exactly as OpenCV's own SSE builds diverged) and is
/// native on aarch64. Verified bit-exact against OpenCV 4.13 for the 3, 5,
/// 9 and 17-tap kernels the detection pipelines use.
pub fn gaussian_blur_f32(
    src: &[f32],
    width: usize,
    height: usize,
    ksize: usize,
    sigma: f64,
    border: BorderMode,
) -> Vec<f32> {
    assert_eq!(src.len(), width * height);
    assert!(ksize % 2 == 1 && ksize > 0, "kernel size must be odd");
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("fma") {
            // SAFETY: FMA support was just verified.
            return unsafe { blur_f32_fma(src, width, height, ksize, sigma, border) };
        }
        blur_f32_impl::<false>(src, width, height, ksize, sigma, border)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        blur_f32_impl::<true>(src, width, height, ksize, sigma, border)
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma,avx2")]
unsafe fn blur_f32_fma(
    src: &[f32],
    width: usize,
    height: usize,
    ksize: usize,
    sigma: f64,
    border: BorderMode,
) -> Vec<f32> {
    blur_f32_impl::<true>(src, width, height, ksize, sigma, border)
}

/// OpenCV's SIMD lane width for f32 on AVX2 builds; determines where the
/// scalar tail begins.
const F32_LANES: usize = 8;

#[inline(always)]
fn blur_f32_impl<const FMA: bool>(
    src: &[f32],
    width: usize,
    height: usize,
    ksize: usize,
    sigma: f64,
    border: BorderMode,
) -> Vec<f32> {
    let kernel: Vec<f32> = gaussian_kernel(ksize, sigma)
        .into_iter()
        .map(|k| k as f32)
        .collect();
    let radius = ksize / 2;
    let body_end = width - width % F32_LANES;

    #[inline(always)]
    fn fmadd<const FMA: bool>(a: f32, b: f32, c: f32) -> f32 {
        if FMA {
            a.mul_add(b, c)
        } else {
            a * b + c
        }
    }

    let mut tmp = vec![0f32; width * height];
    let mut ext = vec![0f32; width + 2 * radius];
    for y in 0..height {
        let row = &src[y * width..(y + 1) * width];
        // Materialize the border-extended row, like FilterEngine does.
        for (i, e) in ext.iter_mut().enumerate() {
            *e = row[border.map(i as isize - radius as isize, width)];
        }
        let out = &mut tmp[y * width..(y + 1) * width];
        // The small symmetric row filters' vector epilogue narrows down to
        // pairs, so only one trailing column (odd widths) is scalar.
        let small_body_end = width - (width & 1);
        match ksize {
            3 => {
                let (k0, k1) = (kernel[1], kernel[2]);
                for (x, o) in out.iter_mut().enumerate().take(small_body_end) {
                    let c = x + radius;
                    let acc = k1 * (ext[c - 1] + ext[c + 1]);
                    *o = fmadd::<FMA>(k0, ext[c], acc);
                }
                for (x, o) in out.iter_mut().enumerate().skip(small_body_end) {
                    let c = x + radius;
                    *o = k0 * ext[c] + k1 * (ext[c - 1] + ext[c + 1]);
                }
            }
            5 => {
                let (k0, k1, k2) = (kernel[2], kernel[3], kernel[4]);
                for (x, o) in out.iter_mut().enumerate().take(small_body_end) {
                    let c = x + radius;
                    let acc = k1 * (ext[c - 1] + ext[c + 1]);
                    let acc = fmadd::<FMA>(k0, ext[c], acc);
                    *o = fmadd::<FMA>(k2, ext[c - 2] + ext[c + 2], acc);
                }
                for (x, o) in out.iter_mut().enumerate().skip(small_body_end) {
                    let c = x + radius;
                    let mut acc = k0 * ext[c];
                    acc += k1 * (ext[c - 1] + ext[c + 1]);
                    acc += k2 * (ext[c - 2] + ext[c + 2]);
                    *o = acc;
                }
            }
            _ => {
                for (x, o) in out.iter_mut().enumerate().take(body_end) {
                    let mut acc = kernel[0] * ext[x];
                    for (k, &kv) in kernel.iter().enumerate().skip(1) {
                        acc = fmadd::<FMA>(kv, ext[x + k], acc);
                    }
                    *o = acc;
                }
                for (x, o) in out.iter_mut().enumerate().skip(body_end) {
                    let mut acc = 0f32;
                    for (k, &kv) in kernel.iter().enumerate() {
                        acc += kv * ext[x + k];
                    }
                    *o = acc;
                }
            }
        }
    }

    // Vertical pass: center-first symmetric pairs, fused in the body,
    // unfused in the tail columns (3-tap tails keep the body formula).
    let mut dst = vec![0f32; width * height];
    let mut rows: Vec<&[f32]> = Vec::with_capacity(ksize);
    for y in 0..height {
        rows.clear();
        for k in 0..ksize {
            let sy = border.map(y as isize + k as isize - radius as isize, height);
            rows.push(&tmp[sy * width..(sy + 1) * width]);
        }
        let drow = &mut dst[y * width..(y + 1) * width];
        let kc = kernel[radius];
        for (x, d) in drow.iter_mut().enumerate().take(body_end) {
            let mut acc = kc * rows[radius][x];
            for i in 1..=radius {
                acc = fmadd::<FMA>(
                    kernel[radius + i],
                    rows[radius - i][x] + rows[radius + i][x],
                    acc,
                );
            }
            *d = acc;
        }
        for (x, d) in drow.iter_mut().enumerate().skip(body_end) {
            if ksize == 3 {
                // The 3-tap column filter is fused across the whole row.
                let acc = kc * rows[1][x];
                *d = fmadd::<FMA>(kernel[2], rows[0][x] + rows[2][x], acc);
            } else {
                let mut acc = kc * rows[radius][x];
                for i in 1..=radius {
                    acc += kernel[radius + i] * (rows[radius - i][x] + rows[radius + i][x]);
                }
                *d = acc;
            }
        }
    }
    dst
}

/// 3x3 median blur with replicated borders, matching OpenCV's `medianBlur`
/// with `ksize = 3`.
///
/// Implemented as a 19-operation min/max median network applied elementwise
/// over shifted row slices, so it compiles to packed u8 min/max.
pub fn median_blur3_u8(src: &[u8], width: usize, height: usize) -> Vec<u8> {
    assert_eq!(src.len(), width * height);
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut out = vec![0u8; width * height];
    if width == 1 || height == 1 {
        // Degenerate sizes: fall back to a scalar 9-sort.
        let b = BorderMode::Replicate;
        for y in 0..height {
            for x in 0..width {
                let mut v = [0u8; 9];
                let mut n = 0;
                for dy in -1isize..=1 {
                    let sy = b.map(y as isize + dy, height);
                    for dx in -1isize..=1 {
                        let sx = b.map(x as isize + dx, width);
                        v[n] = src[sy * width + sx];
                        n += 1;
                    }
                }
                v.sort_unstable();
                out[y * width + x] = v[4];
            }
        }
        return out;
    }

    // Nine working rows: the 3x3 neighborhood as shifted copies of the
    // previous/current/next source rows (replicated at the borders).
    let mut p: Vec<Vec<u8>> = vec![vec![0u8; width]; 9];
    for y in 0..height {
        let ym = if y == 0 { 0 } else { y - 1 };
        let yp = if y + 1 == height { y } else { y + 1 };
        for (r, &sy) in [ym, y, yp].iter().enumerate() {
            let srow = &src[sy * width..(sy + 1) * width];
            // Left-shifted (x-1, replicate), centered, right-shifted (x+1).
            let (l, c, rr) = {
                let base = r * 3;
                (base, base + 1, base + 2)
            };
            p[l][0] = srow[0];
            p[l][1..width].copy_from_slice(&srow[..width - 1]);
            p[c].copy_from_slice(srow);
            p[rr][..width - 1].copy_from_slice(&srow[1..]);
            p[rr][width - 1] = srow[width - 1];
        }

        // Devillard's median-of-9 network: 19 elementwise sort pairs.
        const NET: [(usize, usize); 19] = [
            (1, 2),
            (4, 5),
            (7, 8),
            (0, 1),
            (3, 4),
            (6, 7),
            (1, 2),
            (4, 5),
            (7, 8),
            (0, 3),
            (5, 8),
            (4, 7),
            (3, 6),
            (1, 4),
            (2, 5),
            (4, 7),
            (4, 2),
            (6, 4),
            (4, 2),
        ];
        for &(a, b) in NET.iter() {
            let (lo, hi) = (a.min(b), a.max(b));
            let (head, tail) = p.split_at_mut(hi);
            let (pa, pb) = (&mut head[lo], &mut tail[0]);
            if a < b {
                for (x, y) in pa.iter_mut().zip(pb.iter_mut()) {
                    let (mn, mx) = ((*x).min(*y), (*x).max(*y));
                    *x = mn;
                    *y = mx;
                }
            } else {
                for (y, x) in pa.iter_mut().zip(pb.iter_mut()) {
                    let (mn, mx) = ((*x).min(*y), (*x).max(*y));
                    *x = mn;
                    *y = mx;
                }
            }
        }
        out[y * width..(y + 1) * width].copy_from_slice(&p[4]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_normalized_and_symmetric() {
        let k = gaussian_kernel(7, 1.0);
        let sum: f64 = k.iter().sum();
        assert!((sum - 1.0).abs() < 1e-12);
        for i in 0..3 {
            assert!((k[i] - k[6 - i]).abs() < 1e-15);
        }
        assert!(k[3] > k[2]);
    }

    #[test]
    fn quantized_kernel_sums_to_256() {
        for &(n, s) in &[(7usize, 1.0f64), (13, 2.0), (19, 3.0), (5, 1.4), (9, 1.4)] {
            let q = quantize_kernel_q8(&gaussian_kernel(n, s));
            assert_eq!(q.iter().map(|&v| v as i32).sum::<i32>(), 256);
        }
    }

    #[test]
    fn auto_ksize_matches_opencv() {
        assert_eq!(auto_ksize_u8(1.0), 7);
        assert_eq!(auto_ksize_u8(2.0), 13);
        assert_eq!(auto_ksize_u8(3.0), 19);
        assert_eq!(auto_ksize_u8(1.4), 9);
    }

    #[test]
    fn blur_preserves_flat_image() {
        let img = vec![100u8; 64];
        let out = gaussian_blur_u8(&img, 8, 8, 0, 2.0, BorderMode::Reflect101);
        assert_eq!(out, img);
        let img255 = vec![255u8; 64];
        let out = gaussian_blur_u8(&img255, 8, 8, 0, 1.0, BorderMode::Reflect101);
        assert_eq!(out, img255);
    }

    #[test]
    fn blur_f32_preserves_flat_image() {
        let img = vec![42.0f32; 100];
        let out = gaussian_blur_f32(&img, 10, 10, 5, 0.8, BorderMode::Reflect);
        for v in out {
            assert!((v - 42.0).abs() < 1e-4);
        }
    }

    #[test]
    fn median_removes_single_hot_pixel() {
        let mut img = vec![10u8; 25];
        img[12] = 255;
        let out = median_blur3_u8(&img, 5, 5);
        assert_eq!(out[12], 10);
    }

    #[test]
    fn median_matches_naive_on_random_data() {
        // Deterministic LCG noise; compare the network against a 9-sort.
        let w = 23;
        let h = 17;
        let mut state = 0x12345678u64;
        let img: Vec<u8> = (0..w * h)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 56) as u8
            })
            .collect();
        let fast = median_blur3_u8(&img, w, h);
        let b = BorderMode::Replicate;
        for y in 0..h {
            for x in 0..w {
                let mut v = [0u8; 9];
                let mut n = 0;
                for dy in -1isize..=1 {
                    let sy = b.map(y as isize + dy, h);
                    for dx in -1isize..=1 {
                        let sx = b.map(x as isize + dx, w);
                        v[n] = img[sy * w + sx];
                        n += 1;
                    }
                }
                v.sort_unstable();
                assert_eq!(fast[y * w + x], v[4], "mismatch at ({x},{y})");
            }
        }
    }
}
