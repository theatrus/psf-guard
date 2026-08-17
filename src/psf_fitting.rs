/// PSF (Point Spread Function) fitting module
/// Implements Gaussian and Moffat PSF models with Levenberg-Marquardt fitting
/// Based on HocusFocus implementation by George Hilios
use nalgebra::{DMatrix, DVector};
use std::f64::consts::PI;

/// PSF fitting type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PSFType {
    /// No PSF fitting, use simple metrics only
    None,
    /// Gaussian PSF model
    Gaussian,
    /// Moffat PSF with beta=4.0
    Moffat4,
}

impl std::str::FromStr for PSFType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(PSFType::None),
            "gaussian" => Ok(PSFType::Gaussian),
            "moffat" | "moffat4" | "moffat_4" => Ok(PSFType::Moffat4),
            _ => Err(format!("Unknown PSF type: {}", s)),
        }
    }
}

/// PSF model parameters after fitting
#[derive(Debug, Clone)]
pub struct PSFModel {
    /// PSF type used
    pub psf_type: PSFType,
    /// Amplitude (peak brightness above background)
    pub amplitude: f64,
    /// Background level
    pub background: f64,
    /// X offset from centroid
    pub x0: f64,
    /// Y offset from centroid
    pub y0: f64,
    /// Sigma in X direction
    pub sigma_x: f64,
    /// Sigma in Y direction
    pub sigma_y: f64,
    /// Rotation angle in radians
    pub theta: f64,
    /// R-squared goodness of fit
    pub r_squared: f64,
    /// Root mean square error
    pub rmse: f64,
    /// Full Width Half Maximum
    pub fwhm: f64,
    /// Eccentricity (0 = circular, 1 = line)
    pub eccentricity: f64,
}

impl PSFModel {
    /// Calculate FWHM from sigma values based on PSF type
    pub fn calculate_fwhm(&self) -> f64 {
        let avg_sigma = (self.sigma_x + self.sigma_y) / 2.0;
        match self.psf_type {
            PSFType::Gaussian => avg_sigma * 2.0 * (2.0 * 2.0_f64.ln()).sqrt(), // 2.354
            PSFType::Moffat4 => {
                // For Moffat with beta=4: FWHM = sigma * 2 * sqrt(2^(1/4) - 1)
                avg_sigma * 2.0 * (2.0_f64.powf(0.25) - 1.0).sqrt() // ≈ 1.1895
            }
            PSFType::None => 0.0,
        }
    }

    /// Calculate eccentricity from sigma values
    pub fn calculate_eccentricity(&self) -> f64 {
        let a = self.sigma_x.max(self.sigma_y);
        let b = self.sigma_x.min(self.sigma_y);
        if a > 0.0 {
            (1.0 - (b / a).powi(2)).sqrt()
        } else {
            0.0
        }
    }
}

/// Trait for PSF models
pub trait PSFFunction: Send + Sync {
    /// Evaluate PSF at given position with parameters
    /// parameters: [A, B, x0, y0, sigma_x, sigma_y, theta]
    fn value(&self, x: f64, y: f64, params: &[f64]) -> f64;

    /// Calculate gradient (Jacobian) of PSF with respect to parameters
    fn gradient(&self, x: f64, y: f64, params: &[f64], grad: &mut [f64]);

    /// Convert sigma to FWHM for this PSF type
    fn sigma_to_fwhm(&self, sigma: f64) -> f64;
}

/// Gaussian PSF model
pub struct GaussianPSF;

impl PSFFunction for GaussianPSF {
    fn value(&self, x: f64, y: f64, params: &[f64]) -> f64 {
        let a = params[0]; // Amplitude
        let b = params[1]; // Background
        let x0 = params[2]; // X offset
        let y0 = params[3]; // Y offset
        let sigma_x = params[4];
        let sigma_y = params[5];
        let theta = params[6]; // Rotation angle

        // Rotate coordinates
        let cos_t = theta.cos();
        let sin_t = theta.sin();
        let dx = x - x0;
        let dy = y - y0;
        let xp = dx * cos_t + dy * sin_t;
        let yp = -dx * sin_t + dy * cos_t;

        // Gaussian function
        let arg = -(xp * xp / (2.0 * sigma_x * sigma_x) + yp * yp / (2.0 * sigma_y * sigma_y));
        b + a * arg.exp()
    }

    fn gradient(&self, x: f64, y: f64, params: &[f64], grad: &mut [f64]) {
        let a = params[0];
        let x0 = params[2];
        let y0 = params[3];
        let sigma_x = params[4];
        let sigma_y = params[5];
        let theta = params[6];

        let cos_t = theta.cos();
        let sin_t = theta.sin();
        let dx = x - x0;
        let dy = y - y0;
        let xp = dx * cos_t + dy * sin_t;
        let yp = -dx * sin_t + dy * cos_t;

        let sx2 = sigma_x * sigma_x;
        let sy2 = sigma_y * sigma_y;
        let arg = -(xp * xp / (2.0 * sx2) + yp * yp / (2.0 * sy2));
        let exp_arg = arg.exp();

        // d/dA
        grad[0] = exp_arg;

        // d/dB
        grad[1] = 1.0;

        // d/dx0
        grad[2] = a * exp_arg * (xp * cos_t / sx2 - yp * sin_t / sy2);

        // d/dy0
        grad[3] = a * exp_arg * (xp * sin_t / sx2 + yp * cos_t / sy2);

        // d/dsigma_x
        grad[4] = a * exp_arg * xp * xp / (sx2 * sigma_x);

        // d/dsigma_y
        grad[5] = a * exp_arg * yp * yp / (sy2 * sigma_y);

        // d/dtheta
        grad[6] = a * exp_arg * xp * yp * (1.0 / sx2 - 1.0 / sy2);
    }

    fn sigma_to_fwhm(&self, sigma: f64) -> f64 {
        sigma * 2.0 * (2.0 * 2.0_f64.ln()).sqrt()
    }
}

/// Moffat PSF model with beta=4.0
pub struct Moffat4PSF;

impl PSFFunction for Moffat4PSF {
    fn value(&self, x: f64, y: f64, params: &[f64]) -> f64 {
        let a = params[0]; // Amplitude
        let b = params[1]; // Background
        let x0 = params[2]; // X offset
        let y0 = params[3]; // Y offset
        let u = params[4]; // Sigma X
        let v = params[5]; // Sigma Y
        let theta = params[6]; // Rotation angle

        // Rotate coordinates
        let cos_t = theta.cos();
        let sin_t = theta.sin();
        let dx = x - x0;
        let dy = y - y0;
        let xp = dx * cos_t + dy * sin_t;
        let yp = -dx * sin_t + dy * cos_t;

        // Moffat function with beta=4
        let d = 1.0 + (xp * xp) / (u * u) + (yp * yp) / (v * v);
        b + a / d.powi(4)
    }

    fn gradient(&self, x: f64, y: f64, params: &[f64], grad: &mut [f64]) {
        let a = params[0];
        let x0 = params[2];
        let y0 = params[3];
        let u = params[4];
        let v = params[5];
        let theta = params[6];

        let cos_t = theta.cos();
        let sin_t = theta.sin();
        let dx = x - x0;
        let dy = y - y0;
        let xp = dx * cos_t + dy * sin_t;
        let yp = -dx * sin_t + dy * cos_t;

        let u2 = u * u;
        let v2 = v * v;
        let u3 = u2 * u;
        let v3 = v2 * v;
        let xp2 = xp * xp;
        let yp2 = yp * yp;

        let d = 1.0 + xp2 / u2 + yp2 / v2;
        let beta = 4.0;

        // d/dA
        grad[0] = d.powf(-beta);

        // d/dB
        grad[1] = 1.0;

        // Common terms
        let factor = -a * beta * d.powf(-beta - 1.0);

        // d/dx0
        grad[2] = factor * ((2.0 * sin_t * yp / v2) - (2.0 * cos_t * xp / u2));

        // d/dy0
        grad[3] = factor * ((-2.0 * sin_t * xp / u2) - (2.0 * cos_t * yp / v2));

        // d/du (sigma_x)
        grad[4] = (2.0 * a * beta / u3) * xp2 * d.powf(-beta - 1.0);

        // d/dv (sigma_y)
        grad[5] = (2.0 * a * beta / v3) * yp2 * d.powf(-beta - 1.0);

        // d/dtheta
        grad[6] = factor * (2.0 * yp * xp * (1.0 / u2 - 1.0 / v2));
    }

    fn sigma_to_fwhm(&self, sigma: f64) -> f64 {
        // For Moffat with beta=4: FWHM = sigma * 2 * sqrt(2^(1/4) - 1)
        sigma * 2.0 * (2.0_f64.powf(0.25) - 1.0).sqrt()
    }
}

/// Bilinear interpolation for sub-pixel sampling
pub fn bilinear_sample(data: &[u16], width: usize, height: usize, x: f64, y: f64) -> f64 {
    // Clamp to image bounds
    let x = x.max(0.0).min((width - 1) as f64);
    let y = y.max(0.0).min((height - 1) as f64);

    // Get integer coordinates
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);

    // Get fractional parts
    let fx = x - x0 as f64;
    let fy = y - y0 as f64;

    // Get pixel values
    let p00 = data[y0 * width + x0] as f64;
    let p10 = data[y0 * width + x1] as f64;
    let p01 = data[y1 * width + x0] as f64;
    let p11 = data[y1 * width + x1] as f64;

    // Bilinear interpolation
    let p0 = p00 * (1.0 - fx) + p10 * fx;
    let p1 = p01 * (1.0 - fx) + p11 * fx;
    p0 * (1.0 - fy) + p1 * fy
}

/// Extract sub-pixel sampled ROI around a star
pub fn extract_roi(
    data: &[u16],
    width: usize,
    height: usize,
    center_x: f64,
    center_y: f64,
    roi_size: usize,
    sample_spacing: f64,
) -> (Vec<(f64, f64)>, Vec<f64>) {
    let half_size = roi_size as f64 / 2.0;
    let mut positions = Vec::new();
    let mut values = Vec::new();

    let mut y = -half_size;
    while y <= half_size {
        let mut x = -half_size;
        while x <= half_size {
            let sample_x = center_x + x;
            let sample_y = center_y + y;

            if sample_x >= 0.0
                && sample_x < width as f64
                && sample_y >= 0.0
                && sample_y < height as f64
            {
                positions.push((x, y)); // Relative to centroid
                values.push(bilinear_sample(data, width, height, sample_x, sample_y));
            }

            x += sample_spacing;
        }
        y += sample_spacing;
    }

    (positions, values)
}

/// Simple Levenberg-Marquardt optimizer for PSF fitting
pub struct LevenbergMarquardt {
    max_iterations: usize,
    tolerance: f64,
    lambda: f64,
    lambda_factor: f64,
}

impl Default for LevenbergMarquardt {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            tolerance: 1e-6,
            lambda: 0.01,
            lambda_factor: 10.0,
        }
    }
}

impl LevenbergMarquardt {
    /// Fit PSF model to data
    pub fn fit(
        &mut self,
        psf: &dyn PSFFunction,
        positions: &[(f64, f64)],
        values: &[f64],
        initial_params: &[f64],
        lower_bounds: &[f64],
        upper_bounds: &[f64],
    ) -> Result<Vec<f64>, String> {
        let n_params = initial_params.len();
        let n_points = positions.len();

        if n_points < n_params {
            return Err("Not enough data points for fitting".to_string());
        }

        let mut params = initial_params.to_vec();
        let mut best_params = params.clone();
        let mut best_error = f64::MAX;

        let mut jacobian = DMatrix::<f64>::zeros(n_points, n_params);
        let mut residuals = DVector::<f64>::zeros(n_points);
        let mut gradient = vec![0.0; n_params];

        for _iter in 0..self.max_iterations {
            // Calculate residuals and Jacobian
            let mut current_error = 0.0;
            for (i, ((x, y), observed)) in positions.iter().zip(values.iter()).enumerate() {
                let predicted = psf.value(*x, *y, &params);
                let residual = observed - predicted;
                residuals[i] = residual;
                current_error += residual * residual;

                psf.gradient(*x, *y, &params, &mut gradient);
                for (j, &grad) in gradient.iter().enumerate() {
                    jacobian[(i, j)] = -grad; // Negative because residual = observed - predicted
                }
            }

            if current_error < best_error {
                best_error = current_error;
                best_params = params.clone();
            }

            // Check convergence
            if current_error < self.tolerance {
                break;
            }

            // LM update. The Jacobian here is d(residual)/d(param), so the
            // descent step solves (JᵀJ + λI)·δ = −Jᵀr; solving for +Jᵀr
            // walks uphill, every step gets rejected, and the "fit" returns
            // the initial guess untouched.
            let jt = jacobian.transpose();
            let jtj = &jt * &jacobian;
            let jtr = -(&jt * &residuals);

            loop {
                // Add lambda to diagonal (LM modification)
                let mut h = jtj.clone();
                for i in 0..n_params {
                    h[(i, i)] += self.lambda;
                }

                // Solve for delta
                match h.lu().solve(&jtr) {
                    Some(delta) => {
                        // Apply bounds
                        let mut new_params = params.clone();
                        for i in 0..n_params {
                            new_params[i] = (params[i] + delta[i])
                                .max(lower_bounds[i])
                                .min(upper_bounds[i]);
                        }

                        // Evaluate new error
                        let mut new_error = 0.0;
                        for ((x, y), observed) in positions.iter().zip(values.iter()) {
                            let predicted = psf.value(*x, *y, &new_params);
                            let residual = observed - predicted;
                            new_error += residual * residual;
                        }

                        if new_error < current_error {
                            // Accept update
                            params = new_params;
                            self.lambda /= self.lambda_factor;
                            break;
                        } else {
                            // Reject update, increase lambda
                            self.lambda *= self.lambda_factor;
                            if self.lambda > 1e10 {
                                return Ok(best_params);
                            }
                        }
                    }
                    None => {
                        // Singular matrix, increase lambda
                        self.lambda *= self.lambda_factor;
                        if self.lambda > 1e10 {
                            return Ok(best_params);
                        }
                    }
                }
            }
        }

        Ok(best_params)
    }
}

/// Type alias for residual map data (observed, fitted, residuals)
pub type ResidualMaps = (Vec<Vec<f64>>, Vec<Vec<f64>>, Vec<Vec<f64>>);

/// PSF Fitter
pub struct PSFFitter {
    psf_type: PSFType,
    roi_size: usize,
    sample_spacing: f64,
}

impl PSFFitter {
    pub fn new(psf_type: PSFType) -> Self {
        Self {
            psf_type,
            roi_size: 32,        // Default ROI size
            sample_spacing: 0.5, // Sub-pixel sampling
        }
    }

    /// Fit PSF to a star
    #[allow(clippy::too_many_arguments)]
    pub fn fit_star(
        &self,
        data: &[u16],
        width: usize,
        height: usize,
        center_x: f64,
        center_y: f64,
        bbox_width: f64,
        bbox_height: f64,
        background: f64,
        peak_brightness: f64,
    ) -> Option<PSFModel> {
        if self.psf_type == PSFType::None {
            return None;
        }

        // Extract ROI with sub-pixel sampling
        let (positions, values) = extract_roi(
            data,
            width,
            height,
            center_x,
            center_y,
            self.roi_size,
            self.sample_spacing,
        );

        if positions.len() < 10 {
            return None; // Not enough points
        }

        // Set up PSF model
        let psf: Box<dyn PSFFunction> = match self.psf_type {
            PSFType::Gaussian => Box::new(GaussianPSF),
            PSFType::Moffat4 => Box::new(Moffat4PSF),
            PSFType::None => unreachable!(),
        };

        // Initial parameters: [A, B, x0, y0, sigma_x, sigma_y, theta]
        let initial_params = vec![
            peak_brightness - background, // Amplitude
            background,                   // Background
            0.0,                          // x0 (centered)
            0.0,                          // y0 (centered)
            bbox_width / 3.0,             // sigma_x
            bbox_height / 3.0,            // sigma_y
            0.0,                          // theta
        ];

        // Bounds
        let dx_limit = bbox_width / 8.0;
        let dy_limit = bbox_height / 8.0;
        let sigma_max = ((bbox_width * bbox_width + bbox_height * bbox_height).sqrt()) / 2.0;

        let lower_bounds = vec![
            0.0,       // A must be positive
            0.0,       // B must be positive
            -dx_limit, // x0
            -dy_limit, // y0
            0.1,       // sigma_x minimum
            0.1,       // sigma_y minimum
            -PI / 2.0, // theta
        ];

        let upper_bounds = vec![
            2.0 * (peak_brightness - background), // A max
            peak_brightness,                      // B max
            dx_limit,                             // x0
            dy_limit,                             // y0
            sigma_max,                            // sigma_x max
            sigma_max,                            // sigma_y max
            PI / 2.0,                             // theta
        ];

        // Fit the model
        let mut optimizer = LevenbergMarquardt::default();
        match optimizer.fit(
            &*psf,
            &positions,
            &values,
            &initial_params,
            &lower_bounds,
            &upper_bounds,
        ) {
            Ok(fitted_params) => {
                // Calculate goodness of fit
                let mut sum_squared_residuals = 0.0;
                let mut sum_squared_total = 0.0;
                let mean_value = values.iter().sum::<f64>() / values.len() as f64;

                for ((x, y), observed) in positions.iter().zip(values.iter()) {
                    let predicted = psf.value(*x, *y, &fitted_params);
                    sum_squared_residuals += (observed - predicted).powi(2);
                    sum_squared_total += (observed - mean_value).powi(2);
                }

                let r_squared = if sum_squared_total > 0.0 {
                    1.0 - sum_squared_residuals / sum_squared_total
                } else {
                    0.0
                };

                let rmse = (sum_squared_residuals / positions.len() as f64).sqrt();

                let mut model = PSFModel {
                    psf_type: self.psf_type,
                    amplitude: fitted_params[0],
                    background: fitted_params[1],
                    x0: fitted_params[2],
                    y0: fitted_params[3],
                    sigma_x: fitted_params[4].abs(),
                    sigma_y: fitted_params[5].abs(),
                    theta: fitted_params[6],
                    r_squared,
                    rmse,
                    fwhm: 0.0,
                    eccentricity: 0.0,
                };

                model.fwhm = model.calculate_fwhm();
                model.eccentricity = model.calculate_eccentricity();

                Some(model)
            }
            Err(_) => None,
        }
    }

    /// Generate residual map for visualization
    pub fn generate_residuals(
        &self,
        data: &[u16],
        width: usize,
        height: usize,
        center_x: f64,
        center_y: f64,
        model: &PSFModel,
    ) -> Option<ResidualMaps> {
        let roi_half = self.roi_size as f64 / 2.0;
        let mut observed = vec![vec![0.0; self.roi_size]; self.roi_size];
        let mut fitted = vec![vec![0.0; self.roi_size]; self.roi_size];
        let mut residuals = vec![vec![0.0; self.roi_size]; self.roi_size];

        let psf: Box<dyn PSFFunction> = match self.psf_type {
            PSFType::Gaussian => Box::new(GaussianPSF),
            PSFType::Moffat4 => Box::new(Moffat4PSF),
            PSFType::None => return None,
        };

        let params = vec![
            model.amplitude,
            model.background,
            model.x0,
            model.y0,
            model.sigma_x,
            model.sigma_y,
            model.theta,
        ];

        for i in 0..self.roi_size {
            for j in 0..self.roi_size {
                let rel_x = j as f64 - roi_half + 0.5;
                let rel_y = i as f64 - roi_half + 0.5;

                // Get observed value using bilinear interpolation
                let pixel_x = center_x + rel_x;
                let pixel_y = center_y + rel_y;

                if pixel_x >= 0.0
                    && pixel_x < width as f64
                    && pixel_y >= 0.0
                    && pixel_y < height as f64
                {
                    observed[i][j] = bilinear_sample(data, width, height, pixel_x, pixel_y);
                    fitted[i][j] = psf.value(rel_x, rel_y, &params);
                    residuals[i][j] = observed[i][j] - fitted[i][j];
                }
            }
        }

        Some((observed, fitted, residuals))
    }
}
