//! FITS and XISF frames as sources for PSF-Studio.
//!
//! A frame is read by region through Seiza's windowed reader, by way of
//! [`crate::image_io::read_window`], so a mosaic-sized master is never
//! decoded whole and a normalized XISF frame is put on the same scale as
//! everywhere else in PSF Guard. This module holds no decoding of its own.

use crate::image_steps::{Domain, ImageShape, Region};
use crate::studio::{RegionReader, StudioError};
use std::path::{Path, PathBuf};

/// The longest side Seiza reads in one window.
const WINDOW: usize = seiza_fits::MAX_WINDOW_EDGE;

/// A FITS or XISF frame on disk, read by region.
///
/// Rows count in the order the file stores them, as [`crate::image_io::open`]
/// keeps them.
pub struct FrameSource {
    path: PathBuf,
    shape: ImageShape,
}

fn frame_error(path: &Path, error: impl std::fmt::Display) -> StudioError {
    StudioError::Frame {
        path: path.to_path_buf(),
        reason: error.to_string(),
    }
}

impl FrameSource {
    /// Opens a frame. Only its header is read.
    pub fn open(path: &Path) -> Result<Self, StudioError> {
        // A one-pixel window gives the image's size and planes from the
        // header.
        let window = crate::image_io::read_window(path, 0, 0, 1, 1)
            .map_err(|error| frame_error(path, error))?;
        let shape = ImageShape::new(window.image_width, window.image_height, window.planes);
        if shape.samples().is_none() {
            return Err(frame_error(path, "the frame has no pixels"));
        }
        Ok(Self {
            path: path.to_path_buf(),
            shape,
        })
    }

    /// The frame's path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl RegionReader for FrameSource {
    fn shape(&self) -> ImageShape {
        self.shape
    }

    fn domain(&self) -> Domain {
        // A stack or a master is linear. A frame that was stretched before
        // it was saved carries no mark of it that PSF Guard reads yet.
        Domain::Linear
    }

    fn peak_bytes(&self, region: Region) -> u64 {
        // The result, and one window while it is copied in.
        let window = region.width.min(WINDOW) * region.height.min(WINDOW);
        ((region.width * region.height + window) * self.shape.channels * size_of::<f32>()) as u64
    }

    fn read(&self, region: Region) -> Result<Vec<f32>, StudioError> {
        region.check(self.shape)?;
        let channels = self.shape.channels;
        let mut out = vec![0f32; region.width * region.height * channels];
        for y0 in (region.y..region.bottom()).step_by(WINDOW) {
            let height = WINDOW.min(region.bottom() - y0);
            for x0 in (region.x..region.right()).step_by(WINDOW) {
                let width = WINDOW.min(region.right() - x0);
                let window = crate::image_io::read_window(&self.path, x0, y0, width, height)
                    .map_err(|error| frame_error(&self.path, error))?;
                let whole = (window.x, window.y, window.width, window.height)
                    == (x0, y0, width, height)
                    && window.planes == channels
                    && window.values.len() == width * height * channels;
                if !whole {
                    return Err(frame_error(
                        &self.path,
                        "the frame changed size while it was being read",
                    ));
                }
                // Seiza gives planes one after another; a region is
                // channel-interleaved.
                for plane in 0..channels {
                    for row in 0..height {
                        let from = (plane * height + row) * width;
                        let to = ((y0 - region.y + row) * region.width + (x0 - region.x))
                            * channels
                            + plane;
                        for (column, sample) in window.values[from..from + width].iter().enumerate()
                        {
                            out[to + column * channels] = *sample;
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::{SampleFormat, StoreOptions, TileStore};
    use seiza_fits::{write_f32_image, F32ImageData};

    // Larger than one window in both directions, and not a multiple of it.
    const WIDTH: usize = 1500;
    const HEIGHT: usize = 1100;

    fn samples(planes: usize) -> Vec<f32> {
        (0..WIDTH * HEIGHT * planes)
            .map(|index| {
                let pixel = index / planes;
                let (x, y) = (pixel % WIDTH, pixel / WIDTH);
                100.0 + x as f32 + 1000.0 * y as f32 + 0.25 * (index % planes) as f32
            })
            .collect()
    }

    fn regions() -> [Region; 4] {
        [
            Region::new(0, 0, 40, 30),
            // Across the edges of Seiza's 1024-pixel windows.
            Region::new(1000, 1000, 60, 60),
            Region::new(WIDTH - 20, HEIGHT - 15, 20, 15),
            Region::whole(WIDTH, HEIGHT),
        ]
    }

    #[test]
    fn a_mono_frame_is_read_by_region_as_it_was_written() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("master.fits");
        let written = samples(1);
        write_f32_image(&path, WIDTH, HEIGHT, F32ImageData::Mono(&written), &[]).unwrap();

        let frame = FrameSource::open(&path).unwrap();
        let shape = ImageShape::new(WIDTH, HEIGHT, 1);
        assert_eq!(frame.shape(), shape);
        assert_eq!(frame.domain(), Domain::Linear);
        for region in regions() {
            assert_eq!(
                frame.read(region).unwrap(),
                region.copy_from(&written, shape).unwrap(),
                "{region:?}"
            );
        }
        assert!(frame.read(Region::new(WIDTH - 5, 0, 10, 10)).is_err());
    }

    #[test]
    fn a_color_frame_comes_back_interleaved() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("osc.fits");
        let written = samples(3);
        write_f32_image(
            &path,
            WIDTH,
            HEIGHT,
            F32ImageData::RgbInterleaved(&written),
            &[],
        )
        .unwrap();

        let frame = FrameSource::open(&path).unwrap();
        let shape = ImageShape::new(WIDTH, HEIGHT, 3);
        assert_eq!(frame.shape(), shape);
        for region in regions() {
            assert_eq!(
                frame.read(region).unwrap(),
                region.copy_from(&written, shape).unwrap(),
                "{region:?}"
            );
        }
    }

    #[test]
    fn a_frame_becomes_a_store_with_the_same_pixels() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("master.fits");
        let written = samples(1);
        write_f32_image(&path, WIDTH, HEIGHT, F32ImageData::Mono(&written), &[]).unwrap();

        let frame = FrameSource::open(&path).unwrap();
        let store = TileStore::create(
            &folder.path().join("master.pgtiles"),
            &frame,
            &StoreOptions {
                sample: SampleFormat::F32,
                ..StoreOptions::new("master.fits")
            },
        )
        .unwrap();
        let whole = Region::whole(WIDTH, HEIGHT);
        assert_eq!(store.level(0).unwrap().read(whole).unwrap(), written);
        assert_eq!(store.levels(), 3);
    }

    #[test]
    fn a_file_that_is_not_a_frame_is_refused_by_name() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("notes.fits");
        std::fs::write(&path, b"not a FITS file").unwrap();
        let Err(StudioError::Frame { path: named, .. }) = FrameSource::open(&path) else {
            panic!("a text file was opened as a frame");
        };
        assert_eq!(named, path);
    }
}
