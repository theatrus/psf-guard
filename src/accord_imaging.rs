//! Rust implementation of Accord.NET imaging functions used by N.I.N.A.
//! Based on the exact algorithms from the Accord.NET framework

/// Detection utility functions
pub struct DetectionUtility;

impl DetectionUtility {
    /// Resize image for detection using bicubic interpolation
    pub fn resize_for_detection(
        image: &[u8],
        width: usize,
        height: usize,
        max_width: usize,
        resize_factor: f64,
    ) -> (Vec<u8>, usize, usize) {
        if width <= max_width {
            // No resizing needed
            return (image.to_vec(), width, height);
        }

        let new_width = (width as f64 * resize_factor).floor() as usize;
        let new_height = (height as f64 * resize_factor).floor() as usize;

        // Use image crate for bicubic interpolation
        let resized = resize_bicubic_image_crate(image, width, height, new_width, new_height);

        (resized, new_width, new_height)
    }
}

/// Catmull-Rom (bicubic) resize, bit-identical to
/// `image::imageops::resize(..., FilterType::CatmullRom)` for `Luma<u8>`
/// but several times faster: the vertical pass accumulates per kernel tap
/// across whole rows (each output pixel still receives its terms in tap
/// order, so sums match the per-pixel loop exactly), horizontal kernel
/// weights are computed once per output column instead of being
/// interleaved with the pixel loop, and the input is read in place. The
/// unit test below asserts exact equality against the image crate.
fn resize_bicubic_image_crate(
    image: &[u8],
    width: usize,
    height: usize,
    new_width: usize,
    new_height: usize,
) -> Vec<u8> {
    if width == 0 || height == 0 || new_width == 0 || new_height == 0 {
        return vec![0u8; new_width * new_height];
    }

    // Kernel weights and source bounds for one output coordinate, exactly
    // as image-0.25's horizontal_sample/vertical_sample compute them.
    fn taps(out: usize, in_len: usize, new_len: usize, ws: &mut Vec<f32>) -> usize {
        let ratio = in_len as f32 / new_len as f32;
        let sratio = if ratio < 1.0 { 1.0 } else { ratio };
        let src_support = 2.0 * sratio;

        let input = (out as f32 + 0.5) * ratio;
        let left = (input - src_support).floor() as i64;
        let left = left.clamp(0, in_len as i64 - 1) as usize;
        let right = (input + src_support).ceil() as i64;
        let right = right.clamp(left as i64 + 1, in_len as i64) as usize;
        let input = input - 0.5;

        ws.clear();
        let mut sum = 0.0f32;
        for i in left..right {
            let w = catmullrom_kernel((i as f32 - input) / sratio);
            ws.push(w);
            sum += w;
        }
        for w in ws.iter_mut() {
            *w /= sum;
        }
        left
    }

    // Vertical pass: u8 rows -> f32 intermediate of size width x new_height.
    let mut intermediate = vec![0f32; width * new_height];
    let mut ws = Vec::new();
    for outy in 0..new_height {
        let left = taps(outy, height, new_height, &mut ws);
        let acc = &mut intermediate[outy * width..(outy + 1) * width];
        acc.fill(0.0);
        for (i, &w) in ws.iter().enumerate() {
            let srow = &image[(left + i) * width..(left + i + 1) * width];
            for (a, &v) in acc.iter_mut().zip(srow.iter()) {
                *a += v as f32 * w;
            }
        }
    }

    // Horizontal pass: weights once per output column, then row-major
    // per-pixel tap sums (same term order as the reference), rounded the
    // way image's FloatNearest rounds.
    let mut columns: Vec<(usize, Vec<f32>)> = Vec::with_capacity(new_width);
    for outx in 0..new_width {
        let left = taps(outx, width, new_width, &mut ws);
        columns.push((left, ws.clone()));
    }
    let mut out = vec![0u8; new_width * new_height];
    for y in 0..new_height {
        let irow = &intermediate[y * width..(y + 1) * width];
        let orow = &mut out[y * new_width..(y + 1) * new_width];
        for (o, (left, ws)) in orow.iter_mut().zip(columns.iter()) {
            let mut t = 0.0f32;
            for (i, &w) in ws.iter().enumerate() {
                t += irow[left + i] * w;
            }
            *o = t.clamp(0.0, 255.0).round() as u8;
        }
    }
    out
}

/// The Catmull-Rom cubic spline, exactly as image-0.25 evaluates it
/// (`bc_cubic_spline(x, 0.0, 0.5)` in f32).
fn catmullrom_kernel(x: f32) -> f32 {
    let (b, c) = (0.0f32, 0.5f32);
    let a = x.abs();
    let k = if a < 1.0 {
        (12.0 - 9.0 * b - 6.0 * c) * a.powi(3)
            + (-18.0 + 12.0 * b + 6.0 * c) * a.powi(2)
            + (6.0 - 2.0 * b)
    } else if a < 2.0 {
        (-b - 6.0 * c) * a.powi(3)
            + (6.0 * b + 30.0 * c) * a.powi(2)
            + (-12.0 * b - 48.0 * c) * a
            + (8.0 * b + 24.0 * c)
    } else {
        0.0
    };
    k / 6.0
}

/// Blob representation
#[derive(Debug, Clone)]
pub struct Blob {
    pub rectangle: Rectangle,
}

#[derive(Debug, Clone, Copy)]
pub struct Rectangle {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Blob counter for connected component labeling
#[derive(Default)]
pub struct BlobCounter {
    blobs: Vec<Blob>,
}

impl BlobCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process_image(&mut self, image: &[u8], width: usize, height: usize) {
        self.blobs.clear();

        // Create label image
        let mut labels = vec![0u32; width * height];
        let mut next_label = 1u32;
        let mut equivalences = Vec::new();

        // First pass - assign temporary labels
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;

                if image[idx] > 0 {
                    let mut neighbors = Vec::new();

                    // Check left and top neighbors
                    if x > 0 && labels[idx - 1] > 0 {
                        neighbors.push(labels[idx - 1]);
                    }
                    if y > 0 && labels[idx - width] > 0 {
                        neighbors.push(labels[idx - width]);
                    }

                    if neighbors.is_empty() {
                        labels[idx] = next_label;
                        next_label += 1;
                    } else {
                        let min_label = *neighbors.iter().min().unwrap();
                        labels[idx] = min_label;

                        // Record equivalences
                        for &neighbor in &neighbors {
                            if neighbor != min_label {
                                equivalences.push((min_label, neighbor));
                            }
                        }
                    }
                }
            }
        }

        // Resolve equivalences
        let mut label_map = vec![0u32; next_label as usize];
        for i in 0..next_label {
            label_map[i as usize] = i;
        }

        for &(label1, label2) in &equivalences {
            let root1 = find_root(&mut label_map, label1);
            let root2 = find_root(&mut label_map, label2);
            if root1 != root2 {
                label_map[root2 as usize] = root1;
            }
        }

        // Second pass - relabel and collect blob info
        let mut blob_info: std::collections::HashMap<u32, (i32, i32, i32, i32, usize)> =
            std::collections::HashMap::new();

        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                if labels[idx] > 0 {
                    let final_label = find_root(&mut label_map, labels[idx]);
                    labels[idx] = final_label;

                    let entry = blob_info
                        .entry(final_label)
                        .or_insert((x as i32, y as i32, x as i32, y as i32, 0));
                    entry.0 = entry.0.min(x as i32); // min x
                    entry.1 = entry.1.min(y as i32); // min y
                    entry.2 = entry.2.max(x as i32); // max x
                    entry.3 = entry.3.max(y as i32); // max y
                    entry.4 += 1; // area
                }
            }
        }

        // Create blob objects
        for (_id, (min_x, min_y, max_x, max_y, _area)) in blob_info {
            self.blobs.push(Blob {
                rectangle: Rectangle {
                    x: min_x,
                    y: min_y,
                    width: max_x - min_x + 1,
                    height: max_y - min_y + 1,
                },
            });
        }
    }

    pub fn get_objects_information(&self) -> Vec<Blob> {
        self.blobs.clone()
    }
}

/// Simple shape checker for circle detection
pub struct SimpleShapeChecker;

impl SimpleShapeChecker {
    pub fn is_circle(
        &self,
        points: &[(i32, i32)],
        center_x: &mut f32,
        center_y: &mut f32,
        radius: &mut f32,
    ) -> bool {
        if points.len() < 3 {
            return false;
        }

        // Calculate center as mean of all points
        let sum_x: i32 = points.iter().map(|p| p.0).sum();
        let sum_y: i32 = points.iter().map(|p| p.1).sum();
        let cx = sum_x as f32 / points.len() as f32;
        let cy = sum_y as f32 / points.len() as f32;

        // Calculate mean radius
        let mut sum_radius = 0.0;
        for &(x, y) in points {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            sum_radius += (dx * dx + dy * dy).sqrt();
        }
        let mean_radius = sum_radius / points.len() as f32;

        // Check how well points fit the circle
        let mut max_deviation = 0.0f32;
        for &(x, y) in points {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r = (dx * dx + dy * dy).sqrt();
            let deviation = (r - mean_radius).abs();
            max_deviation = max_deviation.max(deviation);
        }

        // Consider it a circle if max deviation is less than 20% of radius
        let is_circle = max_deviation < mean_radius * 0.2;

        if is_circle {
            *center_x = cx;
            *center_y = cy;
            *radius = mean_radius;
        }

        is_circle
    }
}

// Helper functions

fn find_root(label_map: &mut [u32], label: u32) -> u32 {
    let mut current = label;
    while label_map[current as usize] != current {
        current = label_map[current as usize];
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blob_counter() {
        let image = vec![
            0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 255,
            255, 0, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];

        let mut counter = BlobCounter::new();
        counter.process_image(&image, 7, 7);

        let blobs = counter.get_objects_information();
        assert_eq!(blobs.len(), 2); // Should find 2 blobs

        // Check blob dimensions
        for blob in blobs {
            assert_eq!(blob.rectangle.width, 2);
            assert_eq!(blob.rectangle.height, 2);
        }
    }

    #[test]
    fn resize_is_bit_identical_to_image_crate_catmullrom() {
        use image::{ImageBuffer, Luma};
        let mut state = 0x1234_5678_9abc_def0u64;
        for (w, h, nw, nh) in [
            (97usize, 61usize, 16usize, 10usize), // ~6x downscale like N.I.N.A.
            (64, 64, 33, 21),
            (33, 47, 8, 40),  // downscale one axis, upscale the other
            (16, 16, 31, 31), // upscale
            (5, 3, 2, 2),
        ] {
            let data: Vec<u8> = (0..w * h)
                .map(|_| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    (state >> 56) as u8
                })
                .collect();
            let ours = resize_bicubic_image_crate(&data, w, h, nw, nh);
            let img = ImageBuffer::<Luma<u8>, Vec<u8>>::from_vec(w as u32, h as u32, data.clone())
                .unwrap();
            let reference = image::imageops::resize(
                &img,
                nw as u32,
                nh as u32,
                image::imageops::FilterType::CatmullRom,
            )
            .into_raw();
            assert_eq!(ours, reference, "{w}x{h} -> {nw}x{nh}");
        }
    }
}
