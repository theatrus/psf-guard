//! The PSF fitter must actually improve on its initial guess. A sign error
//! in the Levenberg-Marquardt step once made every step walk uphill, so the
//! "fit" returned the seed parameters (sigma = bounding box / 3, theta = 0)
//! and every eccentricity in the stars endpoint was really box aspect ratio.

use psf_guard::psf_fitting::{PSFFitter, PSFType};

fn rotated_gaussian_frame(
    width: usize,
    height: usize,
    cx: f64,
    cy: f64,
    sx: f64,
    sy: f64,
    theta: f64,
) -> Vec<u16> {
    let cos_t = theta.cos();
    let sin_t = theta.sin();
    let mut data = vec![100u16; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let xp = dx * cos_t + dy * sin_t;
            let yp = -dx * sin_t + dy * cos_t;
            let value =
                100.0 + 20000.0 * (-(xp * xp / (2.0 * sx * sx) + yp * yp / (2.0 * sy * sy))).exp();
            data[y * width + x] = value as u16;
        }
    }
    data
}

#[test]
fn moffat_fit_recovers_a_rotated_elliptical_star() {
    let (width, height) = (64usize, 64usize);
    let (cx, cy) = (32.0, 32.0);
    let true_theta = 0.6;
    let data = rotated_gaussian_frame(width, height, cx, cy, 4.0, 2.0, true_theta);

    let fitter = PSFFitter::new(PSFType::Moffat4);
    let model = fitter
        .fit_star(&data, width, height, cx, cy, 20.0, 20.0, 100.0, 20100.0)
        .expect("fit should converge");

    assert!(
        model.r_squared > 0.98,
        "fit barely improved on the seed: r²={}",
        model.r_squared
    );
    assert!(
        model.eccentricity > 0.5,
        "elongation lost: ecc={}",
        model.eccentricity
    );
    // Compare as axes (period π): the same ellipse can be reported with
    // either sigma larger and theta rotated a quarter turn.
    let major = if model.sigma_x >= model.sigma_y {
        model.theta
    } else {
        model.theta + std::f64::consts::FRAC_PI_2
    }
    .rem_euclid(std::f64::consts::PI);
    let diff = (major - true_theta).rem_euclid(std::f64::consts::PI);
    let axis_error = diff.min(std::f64::consts::PI - diff);
    assert!(
        axis_error < 0.05,
        "major axis off by {axis_error} rad (theta={}, sx={}, sy={})",
        model.theta,
        model.sigma_x,
        model.sigma_y
    );
}
