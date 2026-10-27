//! How much memory a color composition needs at its largest step.
//!
//! A composition holds each source channel as a plane on the reference's
//! grid from loading until it writes the result. Its steps run one after
//! another and free what they make, so the estimate is the planes plus the
//! largest step, not the sum of every step. An RC-Astro tool that runs on
//! this machine needs its own process's memory beside PSF Guard's.
//!
//! The figures are bytes per reference pixel, measured on a four-panel
//! mosaic of 88 to 90 megapixels and rounded up. They are a planning
//! limit, not a limit on what the process uses: allocator overhead, sources
//! larger than the reference and other jobs can need more.

/// Each source channel's plane of `f32` samples.
const PLANE_BYTES_PER_PIXEL: u64 = 4;
/// Registration, alignment, the three-channel composition, its stretch,
/// and the FITS and PNG writes. Measured at 19 to 24.
const BASE_BYTES_PER_PIXEL: u64 = 36;
/// Color calibration's interleaved copy, and the clip map with its crop,
/// beside the composition.
const CALIBRATION_BYTES_PER_PIXEL: u64 = 20;
/// Deconvolution of one channel at a time. Measured at 32.
const DECONVOLUTION_BYTES_PER_PIXEL: u64 = 40;
/// Continuum subtraction reads each channel again, raw, with a one-byte
/// clip mask.
const EMISSION_BYTES_PER_PLANE: u64 = 5;
/// The line contribution continuum subtraction builds. With six channels,
/// the step measured 42 in all.
const EMISSION_BYTES_PER_PIXEL: u64 = 16;
/// The three-channel line contribution, kept from continuum subtraction
/// through the tool steps until it is added to the color channels.
const EMISSION_HELD_BYTES_PER_PIXEL: u64 = 12;
/// The three-channel image a step combines the channels into.
const COLOR_IMAGE_BYTES_PER_PIXEL: u64 = 12;
/// An RC-Astro step's input copy, exchange buffers and result in PSF
/// Guard, per sample of its input.
const TOOL_HOST_BYTES_PER_SAMPLE: u64 = 16;
/// The RC-Astro process itself, per sample of its input. StarXTerminator
/// peaked at 37 to 45 on a three-channel image.
const TOOL_PROCESS_BYTES_PER_SAMPLE: u64 = 48;
/// The original, starless and star-layer displays that RGB stars keep for
/// the composition while the nebula's stars are removed.
const RGB_STARS_HELD_BYTES_PER_PIXEL: u64 = 36;
/// RGB star replacement once both separations are done: the displays, the
/// starless nebula, and their crops while the composition screens the
/// stars in. Measured at 108 without a star layer.
const RGB_STARS_BYTES_PER_PIXEL: u64 = 120;

/// Where an RC-Astro step's tool runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolRun {
    /// On another machine, or not at all because its result is cached.
    Elsewhere,
    /// On this machine, beside PSF Guard.
    Here,
}

/// Star replacement, for its memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StarsMemory {
    /// RGB stars replace the nebula's, so their displays stay in memory
    /// until the composition.
    pub rgb: bool,
    pub tool: ToolRun,
}

/// The steps a color composition runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ColorMemoryPlan {
    /// Source channels.
    pub channels: usize,
    pub calibration: bool,
    pub input_deconvolution: bool,
    pub input_rc_astro: Option<ToolRun>,
    pub emission: bool,
    pub color_rc_astro: Option<ToolRun>,
    pub stars: Option<StarsMemory>,
}

/// A composition's estimated memory and the step that sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ColorMemoryEstimate {
    pub bytes: u64,
    pub bytes_per_pixel: u64,
    pub step: &'static str,
}

impl ColorMemoryPlan {
    /// The estimate for a reference of `pixels`.
    pub fn estimate(&self, pixels: usize) -> ColorMemoryEstimate {
        let channels = self.channels as u64;
        let mut steps = vec![(
            "composition",
            BASE_BYTES_PER_PIXEL
                + if self.calibration {
                    CALIBRATION_BYTES_PER_PIXEL
                } else {
                    0
                },
        )];
        if self.input_deconvolution {
            steps.push(("input deconvolution", DECONVOLUTION_BYTES_PER_PIXEL));
        }
        if let Some(tool) = self.input_rc_astro {
            steps.push(("RC-Astro on the input channels", tool_step(1, tool)));
        }
        if self.emission {
            steps.push((
                "continuum subtraction",
                EMISSION_BYTES_PER_PLANE * channels + EMISSION_BYTES_PER_PIXEL,
            ));
        }
        let contribution = if self.emission {
            EMISSION_HELD_BYTES_PER_PIXEL
        } else {
            0
        };
        if let Some(tool) = self.color_rc_astro {
            steps.push((
                "RC-Astro on the color channels",
                contribution + COLOR_IMAGE_BYTES_PER_PIXEL + tool_step(3, tool),
            ));
        }
        if let Some(stars) = self.stars {
            let separation = COLOR_IMAGE_BYTES_PER_PIXEL + tool_step(3, stars.tool);
            steps.push((
                "star replacement",
                contribution
                    + if stars.rgb {
                        RGB_STARS_BYTES_PER_PIXEL.max(RGB_STARS_HELD_BYTES_PER_PIXEL + separation)
                    } else {
                        separation
                    },
            ));
        }
        let (step, largest) = steps
            .into_iter()
            .reduce(|largest, step| if step.1 > largest.1 { step } else { largest })
            .expect("the composition is always a step");
        let bytes_per_pixel = PLANE_BYTES_PER_PIXEL * channels + largest;
        ColorMemoryEstimate {
            bytes: (pixels as u64).saturating_mul(bytes_per_pixel),
            bytes_per_pixel,
            step,
        }
    }
}

/// An RC-Astro step over `samples` samples a pixel.
fn tool_step(samples: u64, tool: ToolRun) -> u64 {
    samples
        * (TOOL_HOST_BYTES_PER_SAMPLE
            + match tool {
                ToolRun::Elsewhere => 0,
                ToolRun::Here => TOOL_PROCESS_BYTES_PER_SAMPLE,
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb() -> ColorMemoryPlan {
        ColorMemoryPlan {
            channels: 3,
            ..ColorMemoryPlan::default()
        }
    }

    /// SHO with RGB stars: three narrowband and three color channels.
    fn sho_with_rgb_stars(tool: ToolRun) -> ColorMemoryPlan {
        ColorMemoryPlan {
            channels: 6,
            stars: Some(StarsMemory { rgb: true, tool }),
            ..ColorMemoryPlan::default()
        }
    }

    #[test]
    fn the_largest_step_sets_the_estimate_not_the_sum_of_steps() {
        let stars = sho_with_rgb_stars(ToolRun::Elsewhere);
        let with_deconvolution = ColorMemoryPlan {
            input_deconvolution: true,
            ..stars
        };
        assert_eq!(with_deconvolution.estimate(1), stars.estimate(1));
        assert_eq!(stars.estimate(1).step, "star replacement");

        let deconvolved = ColorMemoryPlan {
            input_deconvolution: true,
            ..rgb()
        };
        assert_eq!(deconvolved.estimate(1).step, "input deconvolution");
        assert_eq!(
            deconvolved.estimate(1).bytes_per_pixel,
            3 * PLANE_BYTES_PER_PIXEL + DECONVOLUTION_BYTES_PER_PIXEL
        );
    }

    #[test]
    fn every_channel_adds_a_plane() {
        let six = ColorMemoryPlan {
            channels: 6,
            ..rgb()
        };
        assert_eq!(
            six.estimate(1).bytes_per_pixel - rgb().estimate(1).bytes_per_pixel,
            3 * PLANE_BYTES_PER_PIXEL
        );
        // Continuum subtraction reads every channel again.
        let emission = |channels| ColorMemoryPlan {
            channels,
            emission: true,
            ..rgb()
        };
        assert_eq!(
            emission(6).estimate(1).bytes_per_pixel - emission(4).estimate(1).bytes_per_pixel,
            2 * (PLANE_BYTES_PER_PIXEL + EMISSION_BYTES_PER_PLANE)
        );
    }

    #[test]
    fn the_line_contribution_stays_through_the_tool_steps() {
        let color_tool = ColorMemoryPlan {
            channels: 6,
            color_rc_astro: Some(ToolRun::Elsewhere),
            ..ColorMemoryPlan::default()
        };
        let with_lines = ColorMemoryPlan {
            emission: true,
            ..color_tool
        };
        assert_eq!(
            with_lines.estimate(1).step,
            "RC-Astro on the color channels"
        );
        assert_eq!(
            with_lines.estimate(1).bytes_per_pixel - color_tool.estimate(1).bytes_per_pixel,
            EMISSION_HELD_BYTES_PER_PIXEL
        );
        let stars = sho_with_rgb_stars(ToolRun::Here);
        assert_eq!(
            ColorMemoryPlan {
                emission: true,
                ..stars
            }
            .estimate(1)
            .bytes_per_pixel
                - stars.estimate(1).bytes_per_pixel,
            EMISSION_HELD_BYTES_PER_PIXEL
        );
    }

    #[test]
    fn a_tool_on_this_machine_adds_its_process() {
        let here = sho_with_rgb_stars(ToolRun::Here)
            .estimate(1)
            .bytes_per_pixel;
        let elsewhere = sho_with_rgb_stars(ToolRun::Elsewhere)
            .estimate(1)
            .bytes_per_pixel;
        assert!(here > elsewhere);
        // While the nebula's stars are removed, PSF Guard holds the planes,
        // the RGB star displays and the tool's input beside the process.
        assert_eq!(
            here,
            6 * PLANE_BYTES_PER_PIXEL
                + RGB_STARS_HELD_BYTES_PER_PIXEL
                + COLOR_IMAGE_BYTES_PER_PIXEL
                + 3 * (TOOL_HOST_BYTES_PER_SAMPLE + TOOL_PROCESS_BYTES_PER_SAMPLE)
        );
        let color_tool = |tool| ColorMemoryPlan {
            color_rc_astro: Some(tool),
            ..rgb()
        };
        assert_eq!(
            color_tool(ToolRun::Here).estimate(1).bytes_per_pixel
                - color_tool(ToolRun::Elsewhere).estimate(1).bytes_per_pixel,
            3 * TOOL_PROCESS_BYTES_PER_SAMPLE
        );
    }

    /// Peaks measured on a four-panel mosaic, in bytes per reference pixel:
    /// SHO with RGB stars held 132 in PSF Guard, and 108 while
    /// StarXTerminator, at 136, ran beside it; RGB with continuum
    /// subtraction from three lines held 66.
    #[test]
    fn measured_compositions_fit_their_estimates_closely() {
        for (plan, measured) in [
            (sho_with_rgb_stars(ToolRun::Elsewhere), 132),
            (sho_with_rgb_stars(ToolRun::Here), 108 + 136),
            (
                ColorMemoryPlan {
                    channels: 6,
                    emission: true,
                    ..ColorMemoryPlan::default()
                },
                66,
            ),
        ] {
            let estimate = plan.estimate(1).bytes_per_pixel;
            assert!(
                estimate >= measured && estimate * 10 <= measured * 12,
                "{plan:?}: {estimate} bytes a pixel estimated, {measured} measured"
            );
        }
    }

    #[test]
    fn the_estimate_scales_with_the_reference() {
        let estimate = sho_with_rgb_stars(ToolRun::Here).estimate(89_631_126);
        assert_eq!(estimate.bytes, 89_631_126 * estimate.bytes_per_pixel);
        assert_eq!(rgb().estimate(usize::MAX).bytes, u64::MAX);
    }
}
