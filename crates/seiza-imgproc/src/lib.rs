//! Pure-Rust image processing primitives with OpenCV-compatible semantics.
//!
//! This crate reimplements the small set of OpenCV routines PSF Guard's star
//! detectors rely on, so the binary carries no native OpenCV dependency. Each
//! operation matches the corresponding OpenCV function closely enough that
//! detection pipelines produce the same results:
//!
//! - [`blur`]: separable Gaussian blur (OpenCV kernel derivation and border
//!   semantics) and 3x3 median blur.
//! - [`canny`]: Canny edge detection, ported from OpenCV's scalar
//!   implementation (Sobel aperture 3, L1 gradient, TG22 fixed-point sector
//!   non-maximum suppression, stack-based hysteresis).
//! - [`threshold`]: Otsu binary thresholding.
//! - [`morphology`]: binary erosion/dilation with OpenCV's rectangular,
//!   elliptical and cross structuring elements.
//! - [`contours`]: external contour extraction (Suzuki-Abe border following,
//!   as in `findContours` with `RETR_EXTERNAL`), plus polygon area, arc
//!   length, convex hull, moments and bounding rectangles computed the way
//!   OpenCV computes them for contours.
//! - [`dtfilter`]: the Gastal-Oliveira domain transform filter (normalized
//!   convolution variant), as in `cv::ximgproc::dtFilter` with `DTF_NC`.
//! - [`wavelets`]: à trous B3-spline wavelet decomposition and the
//!   structure-removal pipeline used by the HocusFocus detector.
//!
//! All functions operate on plain slices in row-major order; there is no
//! image type to construct.

pub mod blur;
pub mod border;
pub mod canny;
pub mod contours;
pub mod dtfilter;
pub mod morphology;
pub mod threshold;
pub mod wavelets;

pub use border::BorderMode;
