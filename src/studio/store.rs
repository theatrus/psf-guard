//! An image on disk, in tiles, with a pyramid of reduced levels.
//!
//! A store is written once, tile by tile, from any [`RegionReader`], and
//! then read by region at any level. Neither side holds the image: writing
//! holds one tile, and reading holds the region asked for and one tile's
//! rows.
//!
//! # File layout
//!
//! 1. Eight bytes: `PGTILES1`.
//! 2. Four bytes: the header's length, little-endian.
//! 3. The header, as JSON: the image's shape and domain, the tile size, the
//!    sample format, the key of what the store was made from, and each
//!    level's size and the offset of its samples.
//! 4. Zero bytes up to the next multiple of 4096.
//! 5. Each level's samples, level 0 first.
//!
//! A level's samples are stored tile by tile, in rows of tiles. A tile is
//! `tile` pixels on a side, less at the right and bottom edges, and holds its
//! samples row-major and channel-interleaved, little-endian. Every tile's
//! offset follows from the level's size, so the file has no index.
//!
//! Level 0 is the image. Each further level halves the one before it: a
//! pixel is the mean of the two-by-two pixels it covers, or of those that
//! exist at an odd edge, leaving out samples that are not finite. The last
//! level fits in one tile.

use super::{read_within, MemoryBudget, RegionReader, StudioError};
use crate::image_steps::{Domain, ImageShape, Region};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAGIC: &[u8; 8] = b"PGTILES1";
const VERSION: u32 = 1;
const ALIGNMENT: u64 = 4096;

/// How a store keeps each sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SampleFormat {
    /// 32-bit floats: every sample exactly as it was given.
    F32,
    /// 16-bit floats: half the size, with about three significant digits.
    /// For display-referred results. Linear data loses faint signal.
    F16,
}

impl SampleFormat {
    fn bytes(self) -> usize {
        match self {
            SampleFormat::F32 => 4,
            SampleFormat::F16 => 2,
        }
    }

    fn encode(self, samples: &[f32], out: &mut Vec<u8>) {
        out.clear();
        match self {
            SampleFormat::F32 => {
                out.extend(samples.iter().flat_map(|sample| sample.to_le_bytes()));
            }
            SampleFormat::F16 => out.extend(
                samples
                    .iter()
                    .flat_map(|sample| half::f16::from_f32(*sample).to_bits().to_le_bytes()),
            ),
        }
    }

    fn decode(self, bytes: &[u8], out: &mut [f32]) {
        match self {
            SampleFormat::F32 => {
                for (sample, bytes) in out.iter_mut().zip(bytes.as_chunks::<4>().0) {
                    *sample = f32::from_le_bytes(*bytes);
                }
            }
            SampleFormat::F16 => {
                for (sample, bytes) in out.iter_mut().zip(bytes.as_chunks::<2>().0) {
                    *sample = half::f16::from_bits(u16::from_le_bytes(*bytes)).to_f32();
                }
            }
        }
    }
}

/// How to write a store.
#[derive(Clone, Debug)]
pub struct StoreOptions {
    /// The side of a tile, in pixels.
    pub tile: usize,
    /// How each sample is kept.
    pub sample: SampleFormat,
    /// The most memory the source may use to give one tile.
    pub budget: MemoryBudget,
    /// A key for what the store is made from: the source's fingerprint and
    /// every setting and algorithm version that shaped it. A reader compares
    /// it to know whether the store is still current.
    pub key: String,
}

impl StoreOptions {
    /// 512-pixel tiles of 32-bit floats within a 1 GiB budget.
    pub fn new(key: &str) -> Self {
        Self {
            tile: 512,
            sample: SampleFormat::F32,
            budget: MemoryBudget::new(1 << 30),
            key: key.to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct LevelRecord {
    width: usize,
    height: usize,
    /// Where the level's samples start in the file.
    offset: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Header {
    version: u32,
    shape: ImageShape,
    domain: Domain,
    tile: usize,
    sample: SampleFormat,
    key: String,
    levels: Vec<LevelRecord>,
}

impl Header {
    fn sample_bytes(&self) -> u64 {
        (self.shape.channels * self.sample.bytes()) as u64
    }

    fn level_bytes(&self, level: &LevelRecord) -> u64 {
        level.width as u64 * level.height as u64 * self.sample_bytes()
    }

    /// The tile that holds a pixel's column or row, and where it starts.
    fn tile_of(&self, pixel: usize) -> (usize, usize) {
        (pixel / self.tile, pixel / self.tile * self.tile)
    }

    /// Where a tile's samples start, for the tile whose first pixel is
    /// `(x0, y0)`.
    fn tile_offset(&self, level: &LevelRecord, x0: usize, y0: usize) -> u64 {
        let height = self.tile.min(level.height - y0);
        // Every row of tiles above holds whole rows of pixels, and every
        // tile to the left in this row is this tile's height.
        let pixels = y0 as u64 * level.width as u64 + x0 as u64 * height as u64;
        level.offset + pixels * self.sample_bytes()
    }
}

/// The sizes of a pyramid's levels: the image, then halves, until one fits
/// in a tile.
fn pyramid(shape: ImageShape, tile: usize) -> Vec<(usize, usize)> {
    let mut levels = vec![(shape.width, shape.height)];
    while let Some(&(width, height)) = levels.last() {
        if width <= tile && height <= tile {
            break;
        }
        levels.push((width.div_ceil(2), height.div_ceil(2)));
    }
    levels
}

fn file_error(path: &Path) -> impl FnOnce(std::io::Error) -> StudioError + '_ {
    move |source| StudioError::File {
        path: path.to_path_buf(),
        source,
    }
}

/// Reads exactly `buffer.len()` bytes at `offset`, without moving a shared
/// file position, so several threads can read one store.
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(file, buffer, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buffer.len() {
            match file.seek_read(&mut buffer[done..], offset + done as u64)? {
                0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                read => done += read,
            }
        }
        Ok(())
    }
}

/// An image on disk in tiles, with its pyramid.
pub struct TileStore {
    path: PathBuf,
    file: File,
    header: Header,
}

impl TileStore {
    /// Writes `source` to a new store at `path`, replacing a file that is
    /// there, and opens it.
    ///
    /// The store is written beside `path` and moved into place once it is
    /// complete, so a reader never sees part of one. If writing fails, the
    /// partial file is removed and `path` is left as it was.
    pub fn create(
        path: &Path,
        source: &dyn RegionReader,
        options: &StoreOptions,
    ) -> Result<Arc<Self>, StudioError> {
        let partial = path.with_extension(format!("partial-{}", uuid::Uuid::new_v4().simple()));
        let written = write_store(&partial, source, options)
            .and_then(|()| std::fs::rename(&partial, path).map_err(file_error(path)));
        if let Err(error) = written {
            let _ = std::fs::remove_file(&partial);
            return Err(error);
        }
        Self::open(path)
    }

    /// Opens a store.
    pub fn open(path: &Path) -> Result<Arc<Self>, StudioError> {
        let refuse = |reason: String| StudioError::Store {
            path: path.to_path_buf(),
            reason,
        };
        let file = File::open(path).map_err(file_error(path))?;
        let length = file.metadata().map_err(file_error(path))?.len();
        let mut start = [0u8; 12];
        if length < 12 || read_at(&file, &mut start, 0).is_err() || &start[..8] != MAGIC {
            return Err(refuse("it does not start as a tile store does".to_string()));
        }
        let header_length = u64::from(u32::from_le_bytes([
            start[8], start[9], start[10], start[11],
        ]));
        if 12 + header_length > length {
            return Err(refuse("its header is cut short".to_string()));
        }
        let mut bytes = vec![0u8; header_length as usize];
        read_at(&file, &mut bytes, 12).map_err(file_error(path))?;
        let header: Header = serde_json::from_slice(&bytes)
            .map_err(|error| refuse(format!("its header cannot be read: {error}")))?;
        if header.version != VERSION {
            return Err(refuse(format!(
                "it is format {} and this build reads format {VERSION}",
                header.version
            )));
        }
        if header.tile == 0 || header.shape.samples().is_none() {
            return Err(refuse("its header describes no image".to_string()));
        }
        let expected = pyramid(header.shape, header.tile);
        let sizes: Vec<(usize, usize)> = header
            .levels
            .iter()
            .map(|level| (level.width, level.height))
            .collect();
        if sizes != expected {
            return Err(refuse("its levels do not match its image".to_string()));
        }
        let end = header
            .levels
            .iter()
            .map(|level| level.offset.saturating_add(header.level_bytes(level)))
            .max()
            .unwrap_or(0);
        if end > length {
            return Err(refuse(format!(
                "it is {length} bytes and its levels need {end}: the file is cut short"
            )));
        }
        Ok(Arc::new(Self {
            path: path.to_path_buf(),
            file,
            header,
        }))
    }

    /// The shape of the image at full resolution.
    pub fn shape(&self) -> ImageShape {
        self.header.shape
    }

    /// The number of levels, counting the image itself.
    pub fn levels(&self) -> u8 {
        self.header.levels.len() as u8
    }

    /// The key the store was written with.
    pub fn key(&self) -> &str {
        &self.header.key
    }

    /// How the store keeps each sample.
    pub fn sample(&self) -> SampleFormat {
        self.header.sample
    }

    /// A reader for one level. Level 0 is the image.
    pub fn level(self: &Arc<Self>, level: u8) -> Result<StoreLevel, StudioError> {
        let record =
            *self
                .header
                .levels
                .get(usize::from(level))
                .ok_or_else(|| StudioError::Store {
                    path: self.path.clone(),
                    reason: format!(
                        "it has {} levels and level {level} was asked for",
                        self.levels()
                    ),
                })?;
        Ok(StoreLevel {
            store: self.clone(),
            record,
        })
    }
}

/// One level of a store, read by region.
pub struct StoreLevel {
    store: Arc<TileStore>,
    record: LevelRecord,
}

impl RegionReader for StoreLevel {
    fn shape(&self) -> ImageShape {
        ImageShape::new(
            self.record.width,
            self.record.height,
            self.store.header.shape.channels,
        )
    }

    fn domain(&self) -> Domain {
        self.store.header.domain
    }

    fn peak_bytes(&self, region: Region) -> u64 {
        // The result, and the bytes of one tile's rows while they are
        // decoded.
        let header = &self.store.header;
        let tile = (header.tile as u64).pow(2) * header.sample_bytes();
        super::source::region_bytes(region, header.shape.channels).saturating_add(tile)
    }

    fn read(&self, region: Region) -> Result<Vec<f32>, StudioError> {
        let header = &self.store.header;
        let channels = header.shape.channels;
        region.check(self.shape())?;
        let mut out = vec![0f32; region.width * region.height * channels];
        let mut bytes = Vec::new();
        let mut row = Vec::new();

        let (_, mut y0) = header.tile_of(region.y);
        while y0 < region.bottom() {
            let tile_height = header.tile.min(self.record.height - y0);
            let rows = region.y.max(y0)..region.bottom().min(y0 + tile_height);
            let (_, mut x0) = header.tile_of(region.x);
            while x0 < region.right() {
                let tile_width = header.tile.min(self.record.width - x0);
                let columns = region.x.max(x0)..region.right().min(x0 + tile_width);
                // The rows of this tile that the region touches, at the
                // tile's full width.
                let row_bytes = tile_width * channels * header.sample.bytes();
                bytes.resize(row_bytes * rows.len(), 0);
                let offset = header.tile_offset(&self.record, x0, y0)
                    + ((rows.start - y0) * row_bytes) as u64;
                read_at(&self.store.file, &mut bytes, offset)
                    .map_err(file_error(&self.store.path))?;
                row.resize(tile_width * channels, 0.0);
                for (index, y) in rows.clone().enumerate() {
                    header
                        .sample
                        .decode(&bytes[index * row_bytes..(index + 1) * row_bytes], &mut row);
                    let from = (columns.start - x0) * channels;
                    let to =
                        ((y - region.y) * region.width + (columns.start - region.x)) * channels;
                    let count = columns.len() * channels;
                    out[to..to + count].copy_from_slice(&row[from..from + count]);
                }
                x0 += header.tile;
            }
            y0 += header.tile;
        }
        Ok(out)
    }
}

/// Halves a block: each pixel is the mean of the two-by-two pixels it
/// covers, or of those that exist at an odd edge, leaving out samples that
/// are not finite. A pixel with no finite sample is not a number.
fn halve(samples: &[f32], width: usize, height: usize, channels: usize) -> Vec<f32> {
    let (half_width, half_height) = (width.div_ceil(2), height.div_ceil(2));
    let mut out = Vec::with_capacity(half_width * half_height * channels);
    for y in 0..half_height {
        for x in 0..half_width {
            for channel in 0..channels {
                let (mut sum, mut count) = (0f32, 0u32);
                for row in y * 2..(y * 2 + 2).min(height) {
                    for column in x * 2..(x * 2 + 2).min(width) {
                        let sample = samples[(row * width + column) * channels + channel];
                        if sample.is_finite() {
                            sum += sample;
                            count += 1;
                        }
                    }
                }
                out.push(if count == 0 {
                    f32::NAN
                } else {
                    sum / count as f32
                });
            }
        }
    }
    out
}

fn write_store(
    path: &Path,
    source: &dyn RegionReader,
    options: &StoreOptions,
) -> Result<(), StudioError> {
    let shape = source.shape();
    let refuse = |reason: &str| StudioError::Store {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };
    if shape.samples().is_none() {
        return Err(refuse("the source has no pixels"));
    }
    if options.tile == 0 {
        return Err(refuse("a tile needs at least one pixel a side"));
    }

    // The header's length does not depend on the offsets' values only if
    // they are written at a fixed width, so the levels are placed after a
    // first pass that measures the header.
    let mut header = Header {
        version: VERSION,
        shape,
        domain: source.domain(),
        tile: options.tile,
        sample: options.sample,
        key: options.key.clone(),
        levels: pyramid(shape, options.tile)
            .into_iter()
            .map(|(width, height)| LevelRecord {
                width,
                height,
                offset: u64::MAX,
            })
            .collect(),
    };
    let encode = |header: &Header| serde_json::to_vec(header).expect("a header is valid JSON");
    // u64::MAX is as wide as an offset can be written, so the real header
    // is no longer than this one.
    let reserved = (12 + encode(&header).len() as u64).next_multiple_of(ALIGNMENT);
    let mut offset = reserved;
    for index in 0..header.levels.len() {
        header.levels[index].offset = offset;
        offset += header.level_bytes(&header.levels[index]);
    }
    let header_bytes = encode(&header);

    let file = File::create(path).map_err(file_error(path))?;
    let mut writer = BufWriter::with_capacity(1 << 20, file);
    {
        let mut write = |bytes: &[u8]| writer.write_all(bytes).map_err(file_error(path));
        write(MAGIC)?;
        write(&(header_bytes.len() as u32).to_le_bytes())?;
        write(&header_bytes)?;
        write(&vec![
            0u8;
            (reserved - 12 - header_bytes.len() as u64) as usize
        ])?;
    }

    // The levels above 0 are read back from this same file, through a second
    // handle, once the level beneath is on disk.
    let reading = File::open(path).map_err(file_error(path))?;
    let mut encoded = Vec::new();
    for (index, level) in header.levels.iter().enumerate() {
        let below = match index.checked_sub(1) {
            None => None,
            Some(below) => Some(StoreLevel {
                store: Arc::new(TileStore {
                    path: path.to_path_buf(),
                    file: reading.try_clone().map_err(file_error(path))?,
                    header: header.clone(),
                }),
                record: header.levels[below],
            }),
        };
        for y0 in (0..level.height).step_by(options.tile) {
            let height = options.tile.min(level.height - y0);
            for x0 in (0..level.width).step_by(options.tile) {
                let width = options.tile.min(level.width - x0);
                let samples = match &below {
                    None => {
                        read_within(source, Region::new(x0, y0, width, height), options.budget)?
                    }
                    Some(below) => {
                        let from = Region::new(
                            x0 * 2,
                            y0 * 2,
                            (width * 2).min(below.record.width - x0 * 2),
                            (height * 2).min(below.record.height - y0 * 2),
                        );
                        halve(&below.read(from)?, from.width, from.height, shape.channels)
                    }
                };
                options.sample.encode(&samples, &mut encoded);
                writer.write_all(&encoded).map_err(file_error(path))?;
            }
        }
        // The next level reads this one back.
        writer.flush().map_err(file_error(path))?;
    }
    writer
        .into_inner()
        .map_err(|error| error.into_error())
        .and_then(|file| file.sync_all())
        .map_err(file_error(path))
}
