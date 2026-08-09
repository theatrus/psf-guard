use anyhow::Result;
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ImageStatistics {
    pub width: usize,
    pub height: usize,
    pub mean: f64,
    pub median: f64,
    pub std_dev: f64,
    pub min: f64,
    pub max: f64,
    pub star_count: Option<usize>,
    pub hfr: Option<f64>,
    pub fwhm: Option<f64>,
    pub mad: Option<f64>,
}

impl ImageStatistics {
    /// View as seiza-stretch's exact-histogram statistics for the N.I.N.A.
    /// u16 LUT stretch. Lossless: `calculate_statistics_with_mad` computed
    /// these fields with `seiza_fits::statistics_u16` in the first place.
    /// A missing MAD falls back to the normal-distribution approximation,
    /// matching the retired local stretch implementation.
    pub fn to_stretch_statistics(&self) -> seiza_stretch::Statistics {
        seiza_stretch::Statistics {
            min: self.min as u16,
            max: self.max as u16,
            mean: self.mean,
            std_dev: self.std_dev,
            median: self.median as u16,
            mad: self.mad.unwrap_or(self.std_dev * 0.6745),
            count: self.width * self.height,
        }
    }
}

/// FITS image data structure
pub struct FitsImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u16>, // Keep as 16-bit unsigned integers
    /// Minimum raw (pre-BZERO) value of the source data; `data` is rescaled
    /// so this maps to 0.
    pub raw_min: f64,
    /// Stored units per raw unit: `data = (raw - raw_min) * raw_scale`.
    pub raw_scale: f64,
    /// BZERO offset from the FITS header (0.0 when absent).
    pub bzero: f64,
}

/// A debayered one-shot-color frame, kept in colour.
///
/// [`FitsImage::from_file`] deliberately collapses a mosaic to luminance,
/// because that is what the measurements want. Display wants the opposite, so
/// this is the other half of the same decision rather than a replacement for
/// it: nothing that grades an image goes through here.
pub struct ColorFrame {
    pub width: usize,
    pub height: usize,
    /// `width * height * 3` samples, RGB interleaved, row-major.
    pub data: Vec<u16>,
}

impl ColorFrame {
    /// Load a frame as colour, or `None` when it is not a mosaic.
    ///
    /// A camera without a `BAYERPAT` header — a mono camera behind a filter
    /// wheel — has no colour to recover, and answering `None` lets the caller
    /// fall back to the ordinary greyscale rendition rather than inventing
    /// one.
    pub fn from_file(path: &Path) -> Result<Option<Self>> {
        let fits = crate::image_io::open(path)
            .map_err(|e| anyhow::anyhow!("Failed to open image file {}: {e:?}", path.display()))?;
        Ok(fits.debayer().map(|rgb| Self {
            width: rgb.width,
            height: rgb.height,
            data: rgb.data,
        }))
    }

    /// The luminance the stretch is measured on.
    ///
    /// One transfer derived from luminance and applied to all three channels
    /// keeps their ratios, and so the colour balance. Stretching each channel
    /// against its own statistics would drag the three towards a common
    /// median and grey the image out — which is the usual way a colour
    /// autostretch goes wrong.
    pub fn luminance(&self) -> Vec<u16> {
        self.data
            .chunks_exact(3)
            .map(|pixel| {
                ((u32::from(pixel[0]) + 2 * u32::from(pixel[1]) + u32::from(pixel[2])) / 4) as u16
            })
            .collect()
    }

    /// View one channel as a plane, for handing to a transfer that works on
    /// single-channel data.
    pub fn channel(&self, index: usize) -> Vec<u16> {
        self.data
            .chunks_exact(3)
            .map(|pixel| pixel[index])
            .collect()
    }
}

impl FitsImage {
    /// Extract temperature from FITS headers
    pub fn extract_temperature(path: &Path) -> Option<f64> {
        let headers = crate::image_io::read_header(path).ok()?;
        let temp_keywords = [
            "CCD-TEMP", "TEMP", "SET-TEMP", "CCD_TEMP", "TEMPERAT", "CCDTEMP",
        ];
        temp_keywords.iter().find_map(|keyword| {
            headers
                .iter()
                .find(|(k, _)| k == keyword)
                .and_then(|(_, v)| v.as_f64())
        })
    }

    /// Extract camera model from FITS headers
    pub fn extract_camera_model(path: &Path) -> Option<String> {
        let headers = crate::image_io::read_header(path).ok()?;
        let camera_keywords = ["INSTRUME", "CAMERA", "DETECTOR", "CCD_NAME", "CCDNAME"];
        camera_keywords.iter().find_map(|keyword| {
            headers
                .iter()
                .find(|(k, _)| k == keyword)
                .and_then(|(_, v)| v.as_str())
                .map(str::to_string)
        })
    }

    /// Load FITS image data from file.
    ///
    /// Raw one-shot-color mosaics (a `BAYERPAT` header) are debayered and
    /// collapsed to luminance before any measurement: star metrics (HFR,
    /// FWHM, eccentricity) on a bare color filter array are distorted by
    /// the per-channel sampling, and N.I.N.A. itself measures the
    /// debayered image, so this keeps numbers comparable.
    pub fn from_file(path: &Path) -> Result<Self> {
        let fits = crate::image_io::open(path)
            .map_err(|e| anyhow::anyhow!("Failed to open image file {}: {e:?}", path.display()))?;

        if let Some(rgb) = fits.debayer() {
            // Luminance of the debayered mosaic, already in physical ADU
            return Ok(FitsImage {
                width: rgb.width,
                height: rgb.height,
                data: rgb.to_luma_u16(),
                raw_min: 0.0,
                raw_scale: 1.0,
                bzero: 0.0,
            });
        }

        let (width, height) = (fits.width, fits.height);
        match &fits.pixels {
            // Integer camera data arrives BZERO-folded as physical ADU
            seiza_fits::Pixels::U16(_) | seiza_fits::Pixels::U8(_) => Ok(FitsImage {
                width,
                height,
                data: fits.to_u16().into_owned(),
                raw_min: 0.0,
                raw_scale: 1.0,
                bzero: 0.0,
            }),
            // Float and wide-integer data: min-max rescale into u16 and
            // keep the mapping so values can go back to physical units
            _ => {
                let data_f64: Vec<f64> = match &fits.pixels {
                    seiza_fits::Pixels::I32(data) => data.iter().map(|&v| v as f64).collect(),
                    seiza_fits::Pixels::F32(data) => data.iter().map(|&v| v as f64).collect(),
                    seiza_fits::Pixels::F64(data) => data.clone(),
                    _ => unreachable!(),
                };
                let min = data_f64.iter().copied().fold(f64::INFINITY, f64::min);
                let max = data_f64.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let scale = if max > min {
                    65535.0 / (max - min)
                } else {
                    1.0
                };
                let data = if max > min {
                    data_f64
                        .into_iter()
                        .map(|v| ((v - min) * scale).clamp(0.0, 65535.0) as u16)
                        .collect()
                } else {
                    vec![0u16; width * height]
                };
                Ok(FitsImage {
                    width,
                    height,
                    data,
                    raw_min: min,
                    raw_scale: scale,
                    bzero: 0.0,
                })
            }
        }
    }

    /// Map a value in stored (rescaled u16) units back to physical ADU.
    ///
    /// The stored data is per-frame min/max rescaled, so stored values are
    /// NOT comparable across frames; physical ADU values are. Use this for
    /// any cross-frame comparison of background or brightness levels.
    pub fn stored_to_adu(&self, stored: f64) -> f64 {
        stored / self.raw_scale + self.raw_min + self.bzero
    }

    /// Calculate basic statistics without star detection  
    pub fn calculate_basic_statistics(&self) -> ImageStatistics {
        self.calculate_statistics_with_mad()
    }

    /// Calculate statistics including MAD (single histogram pass)
    pub fn calculate_statistics_with_mad(&self) -> ImageStatistics {
        let stats = seiza_fits::statistics_u16(&self.data);
        ImageStatistics {
            width: self.width,
            height: self.height,
            mean: stats.mean,
            median: stats.median as f64,
            std_dev: stats.std_dev,
            min: stats.min as f64,
            max: stats.max as f64,
            star_count: None,
            hfr: None,
            fwhm: None,
            mad: Some(stats.mad),
        }
    }

    /// Calculate basic image statistics
    pub fn calculate_statistics(&self) -> ImageStatistics {
        let stats = self.calculate_basic_statistics();

        // Return statistics without star detection
        // (star detection is now handled by dedicated modules)
        ImageStatistics {
            width: self.width,
            height: self.height,
            mean: stats.mean,
            median: stats.median,
            std_dev: stats.std_dev,
            min: stats.min,
            max: stats.max,
            star_count: None,
            hfr: None,
            fwhm: None,
            mad: stats.mad,
        }
    }
}
