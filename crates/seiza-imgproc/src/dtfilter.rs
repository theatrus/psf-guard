//! Domain transform filter (Gastal & Oliveira 2011), normalized convolution
//! variant — the algorithm behind `cv::ximgproc::dtFilter` with `DTF_NC`.
//!
//! Edge-aware smoothing: distances between neighboring pixels are stretched
//! by the guide image's gradient, so averaging windows never cross strong
//! edges.

/// Apply the NC domain transform filter to a single-channel f32 image.
///
/// `guide` supplies the edge structure (pass `src` itself for self-guided
/// filtering). `sigma_spatial`/`sigma_color` control the spatial support and
/// the edge sensitivity; `num_iters` is the iteration count (OpenCV default
/// 3). Each iteration runs a horizontal then a vertical normalized box
/// filter in the transformed domain.
pub fn dt_filter_nc(
    guide: &[f32],
    src: &[f32],
    width: usize,
    height: usize,
    sigma_spatial: f64,
    sigma_color: f64,
    num_iters: usize,
) -> Vec<f32> {
    assert_eq!(guide.len(), width * height);
    assert_eq!(src.len(), width * height);
    assert!(num_iters >= 1);

    let ratio = sigma_spatial / sigma_color;

    // Cumulative domain-transform positions along rows and columns:
    // ct[0] = 0, ct[k] = ct[k-1] + 1 + ratio * |g[k] - g[k-1]|.
    let mut ct_h = vec![0f64; width * height];
    for y in 0..height {
        let row = y * width;
        for x in 1..width {
            let d = (guide[row + x] - guide[row + x - 1]).abs() as f64;
            ct_h[row + x] = ct_h[row + x - 1] + 1.0 + ratio * d;
        }
    }
    let mut ct_v = vec![0f64; width * height];
    for x in 0..width {
        for y in 1..height {
            let d = (guide[y * width + x] - guide[(y - 1) * width + x]).abs() as f64;
            ct_v[y * width + x] = ct_v[(y - 1) * width + x] + 1.0 + ratio * d;
        }
    }

    let mut res: Vec<f32> = src.to_vec();
    let n = num_iters as i32;
    for iter in 1..=n {
        // Per-iteration standard deviation from the paper (eq. 14).
        let sigma_h = sigma_spatial * (3.0f64).sqrt() * (2.0f64).powi(n - iter)
            / ((4.0f64).powi(n) - 1.0).sqrt();
        let radius = sigma_h * (3.0f64).sqrt();

        horizontal_pass(&mut res, &ct_h, width, height, radius);
        vertical_pass(&mut res, &ct_v, width, height, radius);
    }
    res
}

fn horizontal_pass(res: &mut [f32], ct: &[f64], width: usize, height: usize, radius: f64) {
    // f32 prefix sums, matching OpenCV's box accumulation precision.
    let mut prefix = vec![0f32; width + 1];
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            prefix[x + 1] = prefix[x] + res[row + x];
        }
        let ct_row = &ct[row..row + width];
        let mut lo = 0usize;
        let mut hi = 0usize;
        for x in 0..width {
            let left = ct_row[x] - radius;
            let right = ct_row[x] + radius;
            while ct_row[lo] < left {
                lo += 1;
            }
            if hi < x {
                hi = x;
            }
            while hi + 1 < width && ct_row[hi + 1] <= right {
                hi += 1;
            }
            let count = (hi - lo + 1) as f32;
            res[row + x] = (prefix[hi + 1] - prefix[lo]) / count;
        }
    }
}

fn vertical_pass(res: &mut [f32], ct: &[f64], width: usize, height: usize, radius: f64) {
    let mut ct_col = vec![0f64; height];
    let mut prefix = vec![0f32; height + 1];
    for x in 0..width {
        for y in 0..height {
            ct_col[y] = ct[y * width + x];
            prefix[y + 1] = prefix[y] + res[y * width + x];
        }
        let mut lo = 0usize;
        let mut hi = 0usize;
        for y in 0..height {
            let left = ct_col[y] - radius;
            let right = ct_col[y] + radius;
            while ct_col[lo] < left {
                lo += 1;
            }
            if hi < y {
                hi = y;
            }
            while hi + 1 < height && ct_col[hi + 1] <= right {
                hi += 1;
            }
            let count = (hi - lo + 1) as f32;
            res[y * width + x] = (prefix[hi + 1] - prefix[lo]) / count;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_image_unchanged() {
        let img = vec![5.0f32; 64];
        let out = dt_filter_nc(&img, &img, 8, 8, 10.0, 0.1, 1);
        for v in out {
            assert!((v - 5.0).abs() < 1e-6);
        }
    }

    #[test]
    fn smooths_noise_but_preserves_strong_edge() {
        // Two flat regions with small noise and a big step between them.
        let w = 32;
        let h = 4;
        let mut img = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let base = if x < 16 { 10.0 } else { 200.0 };
                let noise = if (x + y) % 2 == 0 { 0.5 } else { -0.5 };
                img[y * w + x] = base + noise;
            }
        }
        let guide = img.clone();
        let out = dt_filter_nc(&guide, &img, w, h, 8.0, 30.0, 3);
        // Noise reduced inside regions.
        assert!((out[w + 4] - 10.0).abs() < 0.4);
        assert!((out[w + 24] - 200.0).abs() < 0.4);
        // Step preserved: left side stays near 10, right near 200.
        assert!(out[w + 15] < 30.0);
        assert!(out[w + 16] > 180.0);
    }

    #[test]
    fn tiny_sigma_color_means_identity_on_noisy_data() {
        // With sigma_color far below the pixel differences, every window
        // collapses to the pixel itself.
        let w = 16;
        let img: Vec<f32> = (0..w * 4).map(|i| (i * 37 % 101) as f32 * 50.0).collect();
        let out = dt_filter_nc(&img, &img, w, 4, 10.0, 0.1, 1);
        for (a, b) in out.iter().zip(img.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}
