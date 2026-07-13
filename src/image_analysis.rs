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

impl FitsImage {
    /// Extract temperature from FITS headers
    pub fn extract_temperature(path: &Path) -> Option<f64> {
        let headers = seiza_fits::read_header(path).ok()?;
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
        let headers = seiza_fits::read_header(path).ok()?;
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
        let fits = seiza_fits::FitsImage::open(path)
            .map_err(|e| anyhow::anyhow!("Failed to open FITS file {}: {e:?}", path.display()))?;

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
