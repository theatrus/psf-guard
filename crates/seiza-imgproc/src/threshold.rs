//! Otsu thresholding, a direct port of OpenCV's `getThreshVal_Otsu_8u`.

/// Compute the Otsu threshold for an 8-bit image.
///
/// Matches OpenCV exactly: the loop structure, epsilon guards and
/// tie-breaking (first maximum wins) are ported from `thresh.cpp`.
pub fn otsu_threshold(src: &[u8]) -> u8 {
    let mut hist = [0u32; 256];
    for &v in src {
        hist[v as usize] += 1;
    }

    let n = src.len() as f64;
    let scale = 1.0 / n;
    let mu: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, &h)| i as f64 * h as f64 * scale)
        .sum();

    let mut q1 = 0.0f64;
    let mut mu1 = 0.0f64;
    let mut max_sigma = 0.0f64;
    let mut max_val = 0usize;
    for (i, &h) in hist.iter().enumerate() {
        let p_i = h as f64 * scale;
        mu1 *= q1;
        q1 += p_i;
        let q2 = 1.0 - q1;
        // OpenCV guards with FLT_EPSILON.
        const EPS: f64 = f32::EPSILON as f64;
        if q1.min(q2) < EPS || q1.max(q2) > 1.0 - EPS {
            continue;
        }
        mu1 = (mu1 + i as f64 * p_i) / q1;
        let mu2 = (mu - q1 * mu1) / q2;
        let sigma = q1 * q2 * (mu1 - mu2) * (mu1 - mu2);
        if sigma > max_sigma {
            max_sigma = sigma;
            max_val = i;
        }
    }

    max_val as u8
}

/// Binarize with the Otsu threshold: `v > threshold` becomes 255, else 0
/// (OpenCV `THRESH_BINARY | THRESH_OTSU`).
pub fn otsu_binary(src: &[u8], width: usize, height: usize) -> Vec<u8> {
    assert_eq!(src.len(), width * height);
    let t = otsu_threshold(src);
    src.iter().map(|&v| if v > t { 255 } else { 0 }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bimodal_split() {
        // Two clusters around 50 and 200: threshold must fall between them.
        let mut img = vec![48u8; 500];
        img.extend(vec![52u8; 500]);
        img.extend(vec![198u8; 500]);
        img.extend(vec![202u8; 500]);
        // OpenCV's strict `>` keeps the FIRST maximum: every split between
        // the clusters scores the same, so the last low-cluster bin wins.
        let t = otsu_threshold(&img);
        assert!((52..198).contains(&t), "threshold {t} not between clusters");
        let bin = otsu_binary(&img, 100, 20);
        assert_eq!(bin.iter().filter(|&&v| v == 255).count(), 1000);
    }
}
