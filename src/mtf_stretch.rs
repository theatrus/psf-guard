/// Midtone Transfer Function (MTF) stretching implementation
/// Based on N.I.N.A.'s image stretching algorithm
use crate::image_analysis::ImageStatistics;

/// Apply MTF stretch to image data using N.I.N.A.'s algorithm
pub fn stretch_image(
    data: &[u16],
    statistics: &ImageStatistics,
    factor: f64,
    black_clipping: f64,
) -> Vec<u16> {
    stretch_image_with_bit_depth(data, statistics, factor, black_clipping, 16)
}

/// Apply MTF stretch with explicit bit depth
pub fn stretch_image_with_bit_depth(
    data: &[u16],
    statistics: &ImageStatistics,
    factor: f64,
    black_clipping: f64,
    bit_depth: u8,
) -> Vec<u16> {
    // Calculate target histogram median percent from factor
    // N.I.N.A. default factor is 0.15
    let target_histogram_median_pct = factor;

    // Generate stretch mapping table
    let stretch_map = get_stretch_map_with_bit_depth(
        statistics,
        target_histogram_median_pct,
        black_clipping,
        bit_depth,
    );

    // Apply mapping to all pixels
    data.iter()
        .map(|&pixel| stretch_map[pixel as usize])
        .collect()
}

fn get_stretch_map_with_bit_depth(
    statistics: &ImageStatistics,
    target_histogram_median_pct: f64,
    shadows_clipping: f64,
    bit_depth: u8,
) -> Vec<u16> {
    let mut map = vec![0u16; 65536]; // Full 16-bit range

    // Normalize median and MAD to 0-1 range
    let normalized_median = normalize_u16(statistics.median as u16, bit_depth);
    let max_val = ((1u32 << bit_depth) - 1) as f64;
    let normalized_mad = calculate_mad(statistics) / max_val;

    let scale_factor = 1.4826; // MAD to sigma conversion factor

    let (shadows, midtones, highlights) = if normalized_median > 0.5 {
        // Image is inverted or overexposed
        let shadows = 0.0;
        let highlights = normalized_median - shadows_clipping * normalized_mad * scale_factor;
        let midtones = midtones_transfer_function(
            target_histogram_median_pct,
            1.0 - (highlights - normalized_median),
        );
        (shadows, midtones, highlights)
    } else {
        // Normal image
        let shadows = normalized_median + shadows_clipping * normalized_mad * scale_factor;
        let midtones =
            midtones_transfer_function(target_histogram_median_pct, normalized_median - shadows);
        let highlights = 1.0;

        tracing::debug!(
            "MTF: normalized_median={:.4}, normalized_mad={:.4}, shadows={:.4}, midtones={:.4}",
            normalized_median,
            normalized_mad,
            shadows,
            midtones
        );
        tracing::debug!(
            "  shadows_clipping={}, scale_factor={}",
            shadows_clipping,
            scale_factor
        );
        tracing::debug!(
            "  shadows calculation: {} + {} * {} * {} = {}",
            normalized_median,
            shadows_clipping,
            normalized_mad,
            scale_factor,
            shadows
        );
        tracing::debug!(
            "  midtones input: {} - {} = {}",
            normalized_median,
            shadows,
            normalized_median - shadows
        );

        (shadows, midtones, highlights)
    };

    // Generate mapping for each possible pixel value
    #[allow(clippy::needless_range_loop)]
    for i in 0..map.len() {
        let value = normalize_u16(i as u16, bit_depth);
        let input_value = 1.0 - highlights + value - shadows;
        let stretched = midtones_transfer_function(midtones, input_value);
        map[i] = denormalize_u16(stretched);

        // Debug first few values
        if i < 5 || i == 398 || i == 204 || i == 340 {
            tracing::debug!(
                "  Stretch map[{}]: normalized={:.6}, input={:.6}, stretched={:.6} -> {}",
                i,
                value,
                input_value,
                stretched,
                map[i]
            );
        }
    }

    map
}

/// Calculate Median Absolute Deviation (MAD) from statistics
fn calculate_mad(statistics: &ImageStatistics) -> f64 {
    // Use actual MAD if available, otherwise use approximation
    statistics.mad.unwrap_or(
        // MAD ≈ 0.6745 * σ for normal distribution
        statistics.std_dev * 0.6745,
    )
}

/// Normalize 16-bit value to 0-1 range considering bit depth
fn normalize_u16(value: u16, bit_depth: u8) -> f64 {
    let max_val = (1u32 << bit_depth) - 1;
    value as f64 / max_val as f64
}

/// Denormalize 0-1 value back to 16-bit range
/// Matches NINA's DenormalizeUShort exactly
fn denormalize_u16(value: f64) -> u16 {
    // NINA: return (ushort)(val * ushort.MaxValue + (val < 0.5 ? 0.5 : 0.0));
    let clamped = value.clamp(0.0, 1.0);
    let scaled = clamped * 65535.0;
    let rounded = if scaled < 32767.5 {
        scaled + 0.5
    } else {
        scaled
    };
    rounded as u16
}

/// Midtones Transfer Function (MTF)
/// This is the key stretching function used by N.I.N.A.
fn midtones_transfer_function(midtone_balance: f64, x: f64) -> f64 {
    // N.I.N.A.'s exact implementation
    if x > 0.0 {
        if x < 1.0 {
            return (midtone_balance - 1.0) * x
                / ((2.0 * midtone_balance - 1.0) * x - midtone_balance);
        }
        return 1.0;
    }
    0.0
}

/// Configuration for MTF stretching matching N.I.N.A. defaults
pub struct StretchParameters {
    pub factor: f64,         // Target histogram median position (default: 0.15)
    pub black_clipping: f64, // Shadow clipping in MAD units (default: -2.8)
}

impl Default for StretchParameters {
    fn default() -> Self {
        Self {
            factor: 0.2,          // N.I.N.A. default AutoStretchFactor
            black_clipping: -2.8, // N.I.N.A. default BlackClipping
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_midtones_transfer_function() {
        // Test boundary conditions
        assert_eq!(midtones_transfer_function(0.5, 0.0), 0.0);
        assert_eq!(midtones_transfer_function(0.5, 1.0), 1.0);
        assert_eq!(midtones_transfer_function(0.5, 0.5), 0.5);

        // Test typical values
        let mtf = midtones_transfer_function(0.5, 0.25);
        assert!(mtf > 0.0 && mtf < 0.5);

        let mtf = midtones_transfer_function(0.5, 0.75);
        assert!(mtf > 0.5 && mtf < 1.0);
    }

    #[test]
    fn test_normalize_denormalize() {
        assert_eq!(normalize_u16(0, 16), 0.0);
        assert_eq!(normalize_u16(65535, 16), 1.0);
        assert!((normalize_u16(32768, 16) - 0.5).abs() < 0.0001);

        assert_eq!(denormalize_u16(0.0), 0);
        assert_eq!(denormalize_u16(1.0), 65535);
        // 0.5 * 65535 = 32767.5, rounds to 32768
        assert!((denormalize_u16(0.5) as i32 - 32768).abs() <= 1);
    }
}
