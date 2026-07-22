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

/// Use image crate for bicubic interpolation
fn resize_bicubic_image_crate(
    image: &[u8],
    width: usize,
    height: usize,
    new_width: usize,
    new_height: usize,
) -> Vec<u8> {
    use image::{ImageBuffer, Luma};

    // Create an ImageBuffer from our data
    let img =
        ImageBuffer::<Luma<u8>, Vec<u8>>::from_vec(width as u32, height as u32, image.to_vec())
            .expect("Failed to create image buffer");

    // Resize using bicubic interpolation
    let resized = image::imageops::resize(
        &img,
        new_width as u32,
        new_height as u32,
        image::imageops::FilterType::CatmullRom,
    );

    // Convert back to Vec<u8>
    resized.into_raw()
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
}
