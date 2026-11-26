//! The histogram of the finished picture, as Camera Raw shows one: red,
//! green, blue and luminance, and how much of the picture is clipped.

/// The bins of each channel, across 0 to 1.
pub const BINS: usize = 128;

/// Rec. 709 luminance, as the canvas weighs it.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// A value at or past this is clipped to white; at or below its distance
/// from 0, to black.
const CLIPPED: f32 = 1.0 / 1024.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    /// Per bin: the pixels in red, green, blue and luminance.
    pub bins: [[u32; BINS]; 4],
    pub pixels: u32,
    /// The share of pixels with a channel at white.
    pub white: f32,
    /// The share of pixels with every channel at black.
    pub black: f32,
}

impl Histogram {
    /// The histogram of `rgb`, red, green and blue interleaved, from every
    /// `every`th pixel.
    pub fn of(rgb: &[f32], every: usize) -> Self {
        let mut bins = [[0_u32; BINS]; 4];
        let (mut pixels, mut white, mut black) = (0_u32, 0_u32, 0_u32);
        let bin = |value: f32| ((value.clamp(0.0, 1.0) * BINS as f32) as usize).min(BINS - 1);
        for pixel in rgb.as_chunks::<3>().0.iter().step_by(every.max(1)) {
            if pixel.iter().any(|value| !value.is_finite()) {
                continue;
            }
            pixels += 1;
            for (channel, value) in pixel.iter().enumerate() {
                bins[channel][bin(*value)] += 1;
            }
            let luminance = LUMA[0] * pixel[0] + LUMA[1] * pixel[1] + LUMA[2] * pixel[2];
            bins[3][bin(luminance)] += 1;
            if pixel.iter().any(|value| *value >= 1.0 - CLIPPED) {
                white += 1;
            }
            if pixel.iter().all(|value| *value <= CLIPPED) {
                black += 1;
            }
        }
        let share = |count: u32| count as f32 / pixels.max(1) as f32;
        Self {
            bins,
            pixels,
            white: share(white),
            black: share(black),
        }
    }

    /// The largest bin of any channel, leaving out the two end bins, so a
    /// clipped picture's spike at black or white does not flatten the rest.
    pub fn peak(&self) -> u32 {
        self.bins
            .iter()
            .flat_map(|channel| channel[1..BINS - 1].iter().copied())
            .max()
            .unwrap_or(0)
            .max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_land_in_their_bins_and_clipping_is_counted() {
        let rgb = [
            0.0, 0.0, 0.0, // black
            1.0, 0.5, 0.5, // red clipped
            0.25, 0.25, 0.25, // gray
            0.25, 0.25, 0.25,
        ];
        let histogram = Histogram::of(&rgb, 1);
        assert_eq!(histogram.pixels, 4);
        assert_eq!(histogram.black, 0.25);
        assert_eq!(histogram.white, 0.25);
        assert_eq!(histogram.bins[0][BINS - 1], 1);
        assert_eq!(histogram.bins[1][BINS / 4], 2);
        assert_eq!(histogram.bins[1][BINS / 2], 1);
        assert_eq!(histogram.peak(), 2);
    }

    #[test]
    fn every_nth_pixel_is_read() {
        let rgb = vec![0.5; 3 * 100];
        assert_eq!(Histogram::of(&rgb, 4).pixels, 25);
    }
}
