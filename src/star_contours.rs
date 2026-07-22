//! Star contour analysis on top of seiza-imgproc's contour extraction.
//!
//! Replaces the former OpenCV-based blob detector with identical semantics:
//! external contours of the binary structure map, filtered by area,
//! circularity and convexity, with centroids from contour moments. The
//! underlying contour routines are verified bit-compatible with OpenCV's
//! `findContours`/`contourArea`/`arcLength`/`convexHull`/`moments`.

use crate::accord_imaging::{Blob, Rectangle};
use seiza_imgproc::contours;

/// Advanced star contour analysis
#[derive(Debug, Clone)]
pub struct StarContour {
    pub contour_points: Vec<(i32, i32)>,
    pub area: f64,
    pub perimeter: f64,
    pub circularity: f64,
    pub convexity: f64,
    pub bounding_rect: Rectangle,
    pub centroid: (f64, f64),
}

/// Blob detector using contour analysis with shape filtering.
pub struct StarBlobDetector {
    pub min_area: f64,
    pub max_area: f64,
    pub min_circularity: f64,
    pub min_convexity: f64,
}

impl Default for StarBlobDetector {
    fn default() -> Self {
        Self {
            min_area: 10.0,
            max_area: 10000.0,
            min_circularity: 0.3, // More lenient than perfect circle
            min_convexity: 0.5,   // Allow some concavity for realistic stars
        }
    }
}

impl StarBlobDetector {
    /// Analyze star contours with shape analysis.
    pub fn analyze_star_contours(
        &self,
        binary_image: &[u8],
        width: usize,
        height: usize,
    ) -> Vec<StarContour> {
        let found = contours::find_external_contours(binary_image, width, height);

        let mut stars = Vec::new();
        for contour in found {
            let area = contours::contour_area(&contour);
            let perimeter = contours::arc_length_closed(&contour);

            // Skip tiny or huge contours
            if area < self.min_area || area > self.max_area {
                continue;
            }

            // Circularity: 4 pi * Area / Perimeter^2
            let circularity = if perimeter > 0.0 {
                4.0 * std::f64::consts::PI * area / (perimeter * perimeter)
            } else {
                0.0
            };
            if circularity < self.min_circularity {
                continue;
            }

            // Convexity: Area / ConvexArea
            let hull = contours::convex_hull(&contour);
            let convex_area = contours::contour_area(&hull);
            let convexity = if convex_area > 0.0 {
                area / convex_area
            } else {
                0.0
            };
            if convexity < self.min_convexity {
                continue;
            }

            let (bx, by, bw, bh) = contours::bounding_rect(&contour);
            let rect = Rectangle {
                x: bx,
                y: by,
                width: bw,
                height: bh,
            };

            // Centroid from contour moments, bounding-box center fallback.
            let m = contours::contour_moments(&contour);
            let centroid = if m.m00 > 0.0 {
                (m.m10 / m.m00, m.m01 / m.m00)
            } else {
                (bx as f64 + bw as f64 / 2.0, by as f64 + bh as f64 / 2.0)
            };

            stars.push(StarContour {
                contour_points: contour,
                area,
                perimeter,
                circularity,
                convexity,
                bounding_rect: rect,
                centroid,
            });
        }

        // Sort by area (largest first) for consistent ordering
        stars.sort_by(|a, b| {
            b.area
                .partial_cmp(&a.area)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        stars
    }

    /// Convert StarContour results back to simple Blob format for compatibility
    pub fn star_contours_to_blobs(contours: &[StarContour]) -> Vec<Blob> {
        contours
            .iter()
            .map(|star| Blob {
                rectangle: star.bounding_rect,
            })
            .collect()
    }

    /// Enhanced star quality assessment based on shape analysis
    pub fn assess_star_quality(&self, contour: &StarContour) -> f64 {
        // Quality score based on multiple criteria
        let circularity_score = contour.circularity;
        let convexity_score = contour.convexity;

        // Penalize very elongated stars (likely double stars or artifacts)
        let aspect_ratio = contour.bounding_rect.width as f64 / contour.bounding_rect.height as f64;
        let aspect_score = if aspect_ratio > 1.0 {
            1.0 / aspect_ratio
        } else {
            aspect_ratio
        };

        // Area-based score (favor medium-sized stars)
        let area_score = if contour.area > 50.0 && contour.area < 500.0 {
            1.0
        } else if contour.area > 500.0 {
            500.0 / contour.area // Penalize too large
        } else {
            contour.area / 50.0 // Penalize too small
        };

        // Combined quality score (0.0 to 1.0)
        (circularity_score * 0.4 + convexity_score * 0.3 + aspect_score * 0.2 + area_score * 0.1)
            .min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blob_detector_creation() {
        let detector = StarBlobDetector::default();
        assert_eq!(detector.min_area, 10.0);
        assert_eq!(detector.min_circularity, 0.3);
    }

    #[test]
    fn test_empty_image_has_no_contours() {
        let detector = StarBlobDetector::default();
        let binary_image = vec![0u8; 100]; // 10x10 empty image
        let contours = detector.analyze_star_contours(&binary_image, 10, 10);
        assert_eq!(contours.len(), 0);
    }

    #[test]
    fn test_round_blob_detected() {
        let detector = StarBlobDetector::default();
        let (w, h) = (20usize, 20usize);
        let mut img = vec![0u8; w * h];
        // Filled disc of radius 4 at (10, 10)
        for y in 0..h {
            for x in 0..w {
                let dx = x as f64 - 10.0;
                let dy = y as f64 - 10.0;
                if dx * dx + dy * dy <= 16.0 {
                    img[y * w + x] = 255;
                }
            }
        }
        let contours = detector.analyze_star_contours(&img, w, h);
        assert_eq!(contours.len(), 1);
        let c = &contours[0];
        assert!(c.circularity > 0.7, "disc circularity {}", c.circularity);
        assert!(c.convexity > 0.9, "disc convexity {}", c.convexity);
        assert!((c.centroid.0 - 10.0).abs() < 0.3);
        assert!((c.centroid.1 - 10.0).abs() < 0.3);
        let q = detector.assess_star_quality(c);
        assert!(q > 0.5 && q <= 1.0);
    }

    #[test]
    fn test_star_quality_assessment() {
        let detector = StarBlobDetector::default();
        let star = StarContour {
            contour_points: vec![],
            area: 100.0,
            perimeter: 35.4, // Roughly circular
            circularity: 0.8,
            convexity: 0.9,
            bounding_rect: Rectangle {
                x: 10,
                y: 10,
                width: 10,
                height: 10,
            },
            centroid: (15.0, 15.0),
        };
        let quality = detector.assess_star_quality(&star);
        assert!(quality > 0.0 && quality <= 1.0);
    }
}
