//! N.I.N.A.'s offline sky maps as a survey source. The framing assistant's
//! `FramingAssistantCache` folders, and the whole-sky sets N.I.N.A. offers
//! for download, hold a `CacheInfo.xml` index of 5° tiles (gnomonic, north
//! up, east left, from HiPS2FITS) with a JPEG per tile at full size and at
//! 500, 150 and 75 px. This module reads such a folder and renders any view
//! from its tiles, so the framing view works with no network at all.

use image::{codecs::jpeg::JpegEncoder, ColorType, ImageEncoder, RgbImage};
use rayon::prelude::*;
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Survey layer id prefix; the rest is the folder name.
pub const ID_PREFIX: &str = "nina:";

/// Where the maps live for a server cache root.
pub fn root_for(cache_root: &Path) -> PathBuf {
    cache_root.join("director").join("sky-maps")
}
const VARIANTS: [u32; 3] = [75, 150, 500];
/// A hemisphere composites every tile of a set; the sets hold about two thousand.
const MAX_TILES_PER_RENDER: usize = 4096;
/// Decoded tiles kept between renders, so a pan or a zoom step reads no file twice.
const TILE_CACHE_BYTES: usize = 256 * 1024 * 1024;
/// Decoded tiles one render may hold; a wide view steps down to smaller
/// variants to stay under it. A framing view holds about twenty full tiles.
const RENDER_BUDGET_BYTES: usize = 320 * 1024 * 1024;
const JPEG_QUALITY: u8 = 88;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Broadband,
    Narrowband,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub name: String,
    pub file_name: String,
    pub ra_degrees: f64,
    pub dec_degrees: f64,
    pub fov_w_degrees: f64,
    pub fov_h_degrees: f64,
    pub rotation_degrees: f64,
    pub source: String,
}

#[derive(Debug)]
pub struct SkyMap {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub bandpass: String,
    pub attribution: String,
    /// The online survey this map is a local copy of, when it is one:
    /// `dss2_color` for N.I.N.A.'s DSS set, `nsns_ohs` for its Northern Sky
    /// Narrowband Survey SHO set. The framing view then prefers this map.
    pub stands_in_for: Option<String>,
    pub dir: PathBuf,
    pub tiles: Vec<Tile>,
    /// Unit vectors per tile: center, east, north.
    frames: Vec<[[f64; 3]; 3]>,
    /// Decoded tile images from earlier renders, bounded by bytes.
    cache: Mutex<TileCache>,
}

/// A file's size and modification time: a tile that changed on disk is
/// read again rather than served from memory.
type Stamp = (u64, Option<std::time::SystemTime>);

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// Recently decoded tiles, oldest out first once the budget is spent.
#[derive(Debug, Default)]
struct TileCache {
    images: HashMap<PathBuf, (Stamp, Arc<RgbImage>)>,
    order: VecDeque<PathBuf>,
    bytes: usize,
}

impl TileCache {
    fn get(&self, path: &Path, current: &Stamp) -> Option<Arc<RgbImage>> {
        self.images
            .get(path)
            .filter(|(saved, _)| saved == current)
            .map(|(_, image)| image.clone())
    }

    fn insert(&mut self, path: PathBuf, current: Stamp, image: Arc<RgbImage>) {
        if let Some((_, gone)) = self.images.remove(&path) {
            self.bytes -= gone.as_raw().len();
            self.order.retain(|p| *p != path);
        }
        let size = image.as_raw().len();
        while self.bytes + size > TILE_CACHE_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some((_, gone)) = self.images.remove(&oldest) {
                self.bytes -= gone.as_raw().len();
            }
        }
        self.bytes += size;
        self.order.push_back(path.clone());
        self.images.insert(path, (current, image));
    }
}

/// One requested view, the same shape as a survey cutout.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub ra_degrees: f64,
    pub dec_degrees: f64,
    pub fov_degrees: f64,
    pub width_px: u32,
    pub height_px: u32,
    pub rotation_degrees: f64,
}

fn unit(ra_degrees: f64, dec_degrees: f64) -> [[f64; 3]; 3] {
    let (sin_a, cos_a) = ra_degrees.to_radians().sin_cos();
    let (sin_d, cos_d) = dec_degrees.to_radians().sin_cos();
    [
        [cos_d * cos_a, cos_d * sin_a, sin_d],
        [-sin_a, cos_a, 0.0],
        [-sin_d * cos_a, -sin_d * sin_a, cos_d],
    ]
}

fn dot(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Read the framing assistant's index. RA is in hours, fields in
/// arcminutes; both come out in degrees.
pub fn parse_index(xml: &str) -> Result<Vec<Tile>, String> {
    let mut tiles = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<Image") {
        let after = &rest[start + 6..];
        if !after.starts_with(char::is_whitespace) {
            rest = after;
            continue;
        }
        let end = after
            .find("/>")
            .or_else(|| after.find('>'))
            .ok_or_else(|| "unterminated <Image> element".to_owned())?;
        let body = &after[..end];
        let attr = |key: &str| -> Option<String> {
            let needle = format!("{key}=\"");
            let at = body.find(&needle)? + needle.len();
            let value = &body[at..body[at..].find('"')? + at];
            Some(
                value
                    .replace("&amp;", "&")
                    .replace("&quot;", "\"")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">"),
            )
        };
        let number = |key: &str| -> Result<f64, String> {
            attr(key)
                .ok_or_else(|| format!("tile without {key}"))?
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("tile with an unreadable {key}"))
        };
        let file_name = attr("FileName").ok_or("tile without FileName")?;
        if file_name.contains('/') || file_name.contains('\\') || file_name.starts_with('.') {
            return Err(format!("tile file name '{file_name}' leaves the folder"));
        }
        let ra_hours = number("RA")?;
        let dec_degrees = number("Dec")?;
        let fov_w = number("FoVW")?;
        let fov_h = number("FoVH")?;
        if !(0.0..24.0).contains(&ra_hours)
            || !(-90.0..=90.0).contains(&dec_degrees)
            || !(fov_w > 0.0 && fov_w <= 60.0 * 60.0)
            || !(fov_h > 0.0 && fov_h <= 60.0 * 60.0)
        {
            return Err(format!("tile {file_name} has coordinates out of range"));
        }
        tiles.push(Tile {
            name: attr("Name").unwrap_or_else(|| file_name.trim_end_matches(".jpg").to_owned()),
            file_name,
            ra_degrees: ra_hours * 15.0,
            dec_degrees,
            fov_w_degrees: fov_w / 60.0,
            fov_h_degrees: fov_h / 60.0,
            rotation_degrees: attr("Rotation")
                .and_then(|r| r.trim().parse().ok())
                .unwrap_or(0.0),
            source: attr("Source").unwrap_or_default(),
        });
        rest = &after[end..];
    }
    if tiles.is_empty() {
        return Err("no tiles in the index".to_owned());
    }
    Ok(tiles)
}

/// Every offline map under `root`: one folder each, holding `CacheInfo.xml`.
pub fn discover(root: &Path) -> Vec<SkyMap> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut maps: Vec<SkyMap> = entries
        .flatten()
        .filter_map(|entry| SkyMap::open(&entry.path()).ok())
        .collect();
    maps.sort_by(|a, b| a.name.cmp(&b.name));
    maps
}

impl SkyMap {
    pub fn open(dir: &Path) -> Result<Self, String> {
        let index = dir.join("CacheInfo.xml");
        let xml = std::fs::read_to_string(&index)
            .map_err(|_| format!("{}: no CacheInfo.xml", dir.display()))?;
        let tiles = parse_index(&xml).map_err(|error| format!("{}: {error}", dir.display()))?;
        let folder = dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "map folder has no name".to_owned())?
            .to_owned();
        let source = tiles[0].source.clone();
        let (name, kind, bandpass) = describe(&folder, &source);
        let stands_in_for = stand_in(&folder, &source);
        let attribution = std::fs::read_to_string(dir.join("licence.txt"))
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| !line.trim().is_empty())
                    .map(|l| l.trim().to_owned())
            })
            .filter(|line| line.len() <= 240)
            .unwrap_or_else(|| "N.I.N.A. offline sky map".to_owned());
        let frames = tiles
            .iter()
            .map(|t| unit(t.ra_degrees, t.dec_degrees))
            .collect();
        Ok(Self {
            id: format!("{ID_PREFIX}{folder}"),
            name,
            kind,
            bandpass,
            attribution,
            stands_in_for,
            dir: dir.to_path_buf(),
            tiles,
            frames,
            cache: Mutex::new(TileCache::default()),
        })
    }

    /// Render the view from the tiles around it. Black where no tile
    /// reaches. Runs on a blocking thread. Each tile is composited through
    /// its projected bounding box, rows in parallel, and decoded tiles are
    /// kept between renders, so a framing view takes a fraction of a second
    /// and a hemisphere composites the whole set.
    pub fn render(&self, view: &View) -> Result<Vec<u8>, String> {
        let (w, h) = (view.width_px as usize, view.height_px as usize);
        if w == 0 || h == 0 {
            return Err("empty view".to_owned());
        }
        let scale = view.fov_degrees / w as f64; // degrees per pixel
                                                 // The plane radius bounds the sky radius, so this reaches every tile.
        let view_radius = (view.fov_degrees.hypot(scale * h as f64) / 2.0).to_radians();
        let frame = unit(view.ra_degrees, view.dec_degrees);
        let (sin_r, cos_r) = view.rotation_degrees.to_radians().sin_cos();
        // Tiles that can reach the view, nearest first.
        let mut candidates: Vec<(usize, f64)> = self
            .frames
            .iter()
            .enumerate()
            .filter_map(|(index, tile)| {
                let tile_radius = self.tiles[index]
                    .fov_w_degrees
                    .hypot(self.tiles[index].fov_h_degrees)
                    .to_radians()
                    / 2.0;
                let cos = dot(&frame[0], &tile[0]).clamp(-1.0, 1.0);
                (cos.acos() <= view_radius + tile_radius).then_some((index, cos))
            })
            .collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        candidates.truncate(MAX_TILES_PER_RENDER);
        // Where each tile lands on the output: its corners, edge middles and
        // center through the view's projection, with a margin for the bow of
        // its edges. A tile that reaches behind the view takes the whole
        // output as its box.
        let project = |v: &[f64; 3]| -> Option<(f64, f64)> {
            let d = dot(v, &frame[0]);
            if d <= -1.0 + 1e-9 {
                return None;
            }
            let k = 2.0 / (1.0 + d);
            let xi = k * dot(v, &frame[1]);
            let eta = k * dot(v, &frame[2]);
            let xi0 = (xi * cos_r - eta * sin_r).to_degrees();
            let eta0 = (xi * sin_r + eta * cos_r).to_degrees();
            Some((
                w as f64 / 2.0 - xi0 / scale - 0.5,
                h as f64 / 2.0 - eta0 / scale - 0.5,
            ))
        };
        struct Placed {
            index: usize,
            x0: usize,
            x1: usize,
            y0: usize,
            y1: usize,
            image: Arc<RgbImage>,
        }
        let mut boxes: Vec<(usize, usize, usize, usize, usize)> =
            Vec::with_capacity(candidates.len());
        for (index, _) in &candidates {
            let t = &self.frames[*index];
            let tile = &self.tiles[*index];
            let hw = tile.fov_w_degrees.to_radians() / 2.0;
            let hh = tile.fov_h_degrees.to_radians() / 2.0;
            let mut lo = (f64::INFINITY, f64::INFINITY);
            let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            let mut whole = false;
            for sy in -1..=1 {
                for sx in -1..=1 {
                    let mut v = [0.0; 3];
                    for (axis, slot) in v.iter_mut().enumerate() {
                        *slot =
                            t[0][axis] + sx as f64 * hw * t[1][axis] + sy as f64 * hh * t[2][axis];
                    }
                    let norm = dot(&v, &v).sqrt();
                    v.iter_mut().for_each(|c| *c /= norm);
                    match project(&v) {
                        Some((x, y)) => {
                            lo = (lo.0.min(x), lo.1.min(y));
                            hi = (hi.0.max(x), hi.1.max(y));
                        }
                        None => whole = true,
                    }
                }
            }
            let (x0, x1, y0, y1) = if whole {
                (0, w, 0, h)
            } else {
                let margin_x = 3.0 + (hi.0 - lo.0) * 0.04;
                let margin_y = 3.0 + (hi.1 - lo.1) * 0.04;
                (
                    (lo.0 - margin_x).floor().max(0.0) as usize,
                    ((hi.0 + margin_x).ceil().max(0.0) as usize).min(w),
                    (lo.1 - margin_y).floor().max(0.0) as usize,
                    ((hi.1 + margin_y).ceil().max(0.0) as usize).min(h),
                )
            };
            if x0 < x1 && y0 < y1 {
                boxes.push((*index, x0, x1, y0, y1));
            }
        }
        // Pick each tile's size: the smallest whose pixels are at least as
        // fine as the view's, stepped down together when the render would
        // otherwise hold too much. N.I.N.A.'s sets mix formats behind the
        // `.jpg` name (some small versions are PNGs), so decode by content,
        // and step up to the next size when a file is unreadable.
        let mut choice: Vec<usize> = boxes
            .iter()
            .map(|(index, ..)| {
                let needed = (self.tiles[*index].fov_w_degrees / scale).ceil() as u32;
                VARIANTS
                    .iter()
                    .position(|v| *v >= needed)
                    .unwrap_or(VARIANTS.len())
            })
            .collect();
        let bytes_of = |variant: usize| -> usize {
            let side = VARIANTS.get(variant).copied().unwrap_or(2000) as usize;
            side * side * 3
        };
        loop {
            let total: usize = choice.iter().map(|v| bytes_of(*v)).sum();
            if total <= RENDER_BUDGET_BYTES || choice.iter().all(|v| *v == 0) {
                break;
            }
            for v in &mut choice {
                *v = v.saturating_sub(1);
            }
        }
        let decoded: Vec<Result<Arc<RgbImage>, String>> = boxes
            .par_iter()
            .zip(choice.par_iter())
            .map(|((index, ..), variant)| {
                let tile = &self.tiles[*index];
                let stem = tile.file_name.trim_end_matches(".jpg");
                let mut paths: Vec<PathBuf> = VARIANTS
                    .iter()
                    .skip(*variant)
                    .map(|v| self.dir.join(format!("{stem}_{v}px.jpg")))
                    .collect();
                paths.push(self.dir.join(&tile.file_name));
                let mut last_error = String::new();
                for path in &paths {
                    let current = stamp(path);
                    if let Some(image) = current.as_ref().and_then(|current| {
                        self.cache
                            .lock()
                            .ok()
                            .and_then(|cache| cache.get(path, current))
                    }) {
                        return Ok(image);
                    }
                    match std::fs::read(path) {
                        Ok(bytes) => match image::load_from_memory(&bytes) {
                            Ok(img) => {
                                let image = Arc::new(img.to_rgb8());
                                if let (Some(current), Ok(mut cache)) = (current, self.cache.lock())
                                {
                                    cache.insert(path.clone(), current, image.clone());
                                }
                                return Ok(image);
                            }
                            Err(error) => last_error = format!("{}: {error}", path.display()),
                        },
                        Err(error) => {
                            if last_error.is_empty() {
                                last_error = format!("{}: {error}", path.display());
                            }
                        }
                    }
                }
                Err(last_error)
            })
            .collect();
        let mut placed = Vec::with_capacity(boxes.len());
        for ((index, x0, x1, y0, y1), image) in boxes.into_iter().zip(decoded) {
            placed.push(Placed {
                index,
                x0,
                x1,
                y0,
                y1,
                image: image?,
            });
        }
        // Where each output pixel looks on the sky, once.
        let dirs: Vec<[f32; 3]> = (0..w * h)
            .into_par_iter()
            .map(|at| {
                let (x, y) = (at % w, at / w);
                // Stage convention: east to the left, north up; then the view's turn.
                let xi0 = (w as f64 / 2.0 - (x as f64 + 0.5)) * scale;
                let eta0 = (h as f64 / 2.0 - (y as f64 + 0.5)) * scale;
                let xi = (xi0 * cos_r + eta0 * sin_r).to_radians();
                let eta = (-xi0 * sin_r + eta0 * cos_r).to_radians();
                // Stereographic view, as the framing stage draws and the
                // online provider is asked for: a plane point at distance
                // rho is the sky direction 2·atan(rho / 2) from the center.
                let rho = xi.hypot(eta);
                let v = if rho < 1e-12 {
                    frame[0]
                } else {
                    let (sin_c, cos_c) = (2.0 * (rho / 2.0).atan()).sin_cos();
                    let (ux, uy) = (xi / rho, eta / rho);
                    [
                        cos_c * frame[0][0] + sin_c * (ux * frame[1][0] + uy * frame[2][0]),
                        cos_c * frame[0][1] + sin_c * (ux * frame[1][1] + uy * frame[2][1]),
                        cos_c * frame[0][2] + sin_c * (ux * frame[1][2] + uy * frame[2][2]),
                    ]
                };
                [v[0] as f32, v[1] as f32, v[2] as f32]
            })
            .collect();
        // Rows in parallel: each pixel takes the tile whose center is nearest
        // among those that hold it.
        let mut out = vec![0u8; w * h * 3];
        out.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
            let mut best = vec![-1.0f32; w];
            for tile in placed.iter().filter(|p| p.y0 <= y && y < p.y1) {
                let t = &self.frames[tile.index];
                let t0 = [t[0][0] as f32, t[0][1] as f32, t[0][2] as f32];
                let t1 = [t[1][0] as f32, t[1][1] as f32, t[1][2] as f32];
                let t2 = [t[2][0] as f32, t[2][1] as f32, t[2][2] as f32];
                let info = &self.tiles[tile.index];
                let img = &tile.image;
                let (tw, th) = (img.width() as f64, img.height() as f64);
                let (sx, sy) = (info.fov_w_degrees / tw, info.fov_h_degrees / th);
                for x in tile.x0..tile.x1 {
                    let v = &dirs[y * w + x];
                    let d = v[0] * t0[0] + v[1] * t0[1] + v[2] * t0[2];
                    if d <= 1e-9 || d <= best[x] {
                        continue;
                    }
                    let xt = ((v[0] * t1[0] + v[1] * t1[1] + v[2] * t1[2]) / d) as f64;
                    let yt = ((v[0] * t2[0] + v[1] * t2[1] + v[2] * t2[2]) / d) as f64;
                    let px = tw / 2.0 - xt.to_degrees() / sx - 0.5;
                    let py = th / 2.0 - yt.to_degrees() / sy - 0.5;
                    if px < -0.5 || py < -0.5 || px > tw - 0.5 || py > th - 0.5 {
                        continue;
                    }
                    best[x] = d;
                    let rgb = sample(img, px, py);
                    row[x * 3..x * 3 + 3].copy_from_slice(&rgb);
                }
            }
        });
        let mut bytes = Vec::with_capacity(w * h / 4);
        JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
            .write_image(&out, w as u32, h as u32, ColorType::Rgb8.into())
            .map_err(|error| format!("encoding the view: {error}"))?;
        Ok(bytes)
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Decode the smallest version of every tile into memory, so the first
    /// wide view renders at once instead of reading two thousand files. The
    /// small versions of a whole set take a few tens of megabytes; framing
    /// widths read their full tiles on demand as before.
    pub fn warm(&self) {
        for tile in &self.tiles {
            let stem = tile.file_name.trim_end_matches(".jpg");
            let path = self.dir.join(format!("{stem}_{}px.jpg", VARIANTS[0]));
            let Some(current) = stamp(&path) else {
                continue;
            };
            if self
                .cache
                .lock()
                .ok()
                .and_then(|cache| cache.get(&path, &current))
                .is_some()
            {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(img) = image::load_from_memory(&bytes) else {
                continue;
            };
            if let Ok(mut cache) = self.cache.lock() {
                cache.insert(path, current, Arc::new(img.to_rgb8()));
            }
        }
    }
}

fn sample(img: &RgbImage, px: f64, py: f64) -> [u8; 3] {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let x0 = px.floor() as i64;
    let y0 = py.floor() as i64;
    let fx = px - x0 as f64;
    let fy = py - y0 as f64;
    let at = |x: i64, y: i64| -> [f64; 3] {
        let p = img
            .get_pixel(x.clamp(0, w - 1) as u32, y.clamp(0, h - 1) as u32)
            .0;
        [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
    };
    let (a, b, c, d) = (
        at(x0, y0),
        at(x0 + 1, y0),
        at(x0, y0 + 1),
        at(x0 + 1, y0 + 1),
    );
    let mut out = [0u8; 3];
    for (i, slot) in out.iter_mut().enumerate() {
        let top = a[i] * (1.0 - fx) + b[i] * fx;
        let bottom = c[i] * (1.0 - fx) + d[i] * fx;
        *slot = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// A readable name, band kind and bandpass for a map, from the folder N.I.N.A.
/// ships and the tiles' `Source`.
/// The online layer a map replaces. The starless narrowband set is a layer
/// of its own, since the online survey has stars.
fn stand_in(folder: &str, source: &str) -> Option<String> {
    let (name, _, _) = describe(folder, source);
    match name.as_str() {
        "DSS (offline)" => Some("dss2_color".to_owned()),
        "NSNS SHO (offline)" => Some("nsns_ohs".to_owned()),
        _ => None,
    }
}

fn describe(folder: &str, source: &str) -> (String, Kind, String) {
    let lower = folder.to_ascii_lowercase();
    let narrowband =
        source.to_ascii_uppercase().starts_with("NSNBS") || lower.contains("narrowband");
    let dss = !narrowband
        && (lower == "framingassistantcache"
            || lower == "framingassistantcache_full"
            || lower.contains("full")
            || source.to_ascii_uppercase().contains("DSS"));
    let name = if dss {
        "DSS (offline)".to_owned()
    } else if lower.contains("ohs_starless") {
        "NSNS SHO starless (offline)".to_owned()
    } else if lower.contains("ohs") {
        "NSNS SHO (offline)".to_owned()
    } else {
        let mut label = folder
            .trim_start_matches("FramingAssistantCache_")
            .replace('_', " ");
        if label.is_empty() {
            label = folder.to_owned();
        }
        let _ = write!(label, " (offline)");
        label
    };
    let bandpass = if narrowband {
        "O III, H-alpha and S II as colour, from N.I.N.A.'s offline sky map".to_owned()
    } else {
        "Blue, red and near infrared plates, from N.I.N.A.'s offline sky map".to_owned()
    };
    (
        name,
        if narrowband {
            Kind::Narrowband
        } else {
            Kind::Broadband
        },
        bandpass,
    )
}

#[cfg(test)]
mod tests {
    /// Timing over a real N.I.N.A. set: `PSF_GUARD_SKY_MAPS_BENCH=<dir holding the
    /// map folder> cargo test --lib sky_maps::tests::render_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn render_timing() {
        let Some(root) = std::env::var_os("PSF_GUARD_SKY_MAPS_BENCH") else {
            return;
        };
        let maps = super::discover(std::path::Path::new(&root));
        let map = maps.first().expect("a map under the bench root");
        for (label, fov, w, h) in [
            ("framing 12°", 12.0, 2048, 1536),
            ("wide 60°", 60.0, 2048, 1536),
            ("hemisphere 180°", 180.0, 2048, 1536),
            ("framing 12° again", 12.0, 2048, 1536),
        ] {
            let view = super::View {
                ra_degrees: 38.2,
                dec_degrees: 61.45,
                fov_degrees: fov,
                width_px: w,
                height_px: h,
                rotation_degrees: 0.0,
            };
            let started = std::time::Instant::now();
            let bytes = map.render(&view).expect("render");
            println!(
                "{label}: {:?} ({} KB, {} tiles in set)",
                started.elapsed(),
                bytes.len() / 1024,
                map.tile_count()
            );
        }
    }

    use super::*;

    fn solid(dir: &Path, name: &str, rgb: [u8; 3], size: u32) {
        let mut img = RgbImage::new(size, size);
        for p in img.pixels_mut() {
            p.0 = rgb;
        }
        img.save(dir.join(format!("{name}.jpg"))).unwrap();
    }

    /// Two 4° tiles side by side on the equator: the eastern one blue, the
    /// western one red, in the shape N.I.N.A. writes.
    fn map(dir: &Path) -> SkyMap {
        std::fs::write(
            dir.join("CacheInfo.xml"),
            r#"<?xml version="1.0" encoding="utf-8"?>
<ImageCacheInfo>
  <Image Id="a" RA="0.8000" Dec="0.0000" Rotation="0" FoVW="240" FoVH="240" FileName="east.jpg" Source="NSNBS_DR0.2_ohs8" Name="east" />
  <Image Id="b" RA="0.5333" Dec="0.0000" Rotation="0" FoVW="240" FoVH="240" FileName="west.jpg" Source="NSNBS_DR0.2_ohs8" Name="west" />
</ImageCacheInfo>"#,
        )
        .unwrap();
        std::fs::write(dir.join("licence.txt"), "Test tiles, CC-BY.\n").unwrap();
        solid(dir, "east", [0, 0, 255], 64);
        solid(dir, "east_150px", [0, 0, 255], 150);
        solid(dir, "west", [255, 0, 0], 64);
        SkyMap::open(dir).unwrap()
    }

    #[test]
    fn offline_sets_name_the_online_survey_they_stand_in_for() {
        assert_eq!(
            super::stand_in("FramingAssistantCache", "Hips2FitsSurvey").as_deref(),
            Some("dss2_color")
        );
        assert_eq!(
            super::stand_in(
                "FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_withStars",
                "NSNBS"
            )
            .as_deref(),
            Some("nsns_ohs")
        );
        // The starless set is a layer of its own: the online survey has stars.
        assert_eq!(
            super::stand_in(
                "FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_starless",
                "NSNBS"
            ),
            None
        );
        assert_eq!(super::stand_in("SomethingElse", "Custom"), None);
    }

    #[test]
    fn warming_fills_the_cache_with_small_tiles_and_a_render_then_reads_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let map = map(dir.path());
        // The fixture has full tiles only; a real set has small versions.
        let mut small = RgbImage::new(16, 16);
        for p in small.pixels_mut() {
            p.0 = [200, 30, 30];
        }
        small.save(dir.path().join("west_75px.jpg")).unwrap();
        map.warm();
        let cached = map.cache.lock().unwrap().images.len();
        assert_eq!(
            cached, 1,
            "the one small tile is decoded, the missing one skipped"
        );
        let view = View {
            ra_degrees: 8.0,
            dec_degrees: 0.0,
            fov_degrees: 30.0,
            width_px: 96,
            height_px: 48,
            rotation_degrees: 0.0,
        };
        assert!(!map.render(&view).unwrap().is_empty());
    }

    #[test]
    fn the_index_reads_hours_and_arcminutes_into_degrees() {
        let dir = tempfile::tempdir().unwrap();
        let map = map(dir.path());
        assert_eq!(
            map.id,
            format!("nina:{}", dir.path().file_name().unwrap().to_str().unwrap())
        );
        assert_eq!(map.kind, Kind::Narrowband);
        assert_eq!(map.attribution, "Test tiles, CC-BY.");
        assert_eq!(map.tiles.len(), 2);
        assert!((map.tiles[0].ra_degrees - 12.0).abs() < 1e-9);
        assert!((map.tiles[0].fov_w_degrees - 4.0).abs() < 1e-9);
        assert!(parse_index("<ImageCacheInfo></ImageCacheInfo>").is_err());
        assert!(parse_index(
            r#"<Image RA="1" Dec="0" FoVW="300" FoVH="300" FileName="../x.jpg" />"#
        )
        .is_err());
        assert!(
            parse_index(r#"<Image RA="25" Dec="0" FoVW="300" FoVH="300" FileName="x.jpg" />"#)
                .is_err()
        );
        assert!(discover(dir.path().parent().unwrap())
            .iter()
            .any(|m| m.id == map.id));
        assert!(discover(&dir.path().join("nowhere")).is_empty());
    }

    #[test]
    fn a_view_across_the_seam_takes_each_side_from_its_tile_and_black_beyond() {
        let dir = tempfile::tempdir().unwrap();
        let map = map(dir.path());
        let view = View {
            ra_degrees: 10.0,
            dec_degrees: 0.0,
            fov_degrees: 6.0,
            width_px: 96,
            height_px: 48,
            rotation_degrees: 0.0,
        };
        let jpeg = map.render(&view).unwrap();
        let img = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (96, 48));
        // East is to the left: the blue tile centred at RA 12° covers the left.
        let left = img.get_pixel(10, 24).0;
        let right = img.get_pixel(85, 24).0;
        assert!(left[2] > 200 && left[0] < 60, "left {left:?}");
        assert!(right[0] > 200 && right[2] < 60, "right {right:?}");
        // Above the tiles' 4° height nothing is drawn.
        let far = View {
            dec_degrees: 20.0,
            ..view
        };
        let black = image::load_from_memory(&map.render(&far).unwrap())
            .unwrap()
            .to_rgb8();
        assert!(black.pixels().all(|p| p.0.iter().all(|c| *c < 8)));
        // A narrow, fine view picks the finer variant when one exists and still renders.
        let fine = View {
            ra_degrees: 12.0,
            fov_degrees: 1.0,
            width_px: 200,
            height_px: 100,
            ..view
        };
        let detail = image::load_from_memory(&map.render(&fine).unwrap())
            .unwrap()
            .to_rgb8();
        assert!(detail.get_pixel(100, 50).0[2] > 200);
    }

    #[test]
    fn a_png_behind_a_jpg_name_decodes_and_a_broken_file_falls_back_to_the_next_size() {
        let dir = tempfile::tempdir().unwrap();
        let map = map(dir.path());
        // N.I.N.A.'s DSS set has PNG bytes in some `_500px.jpg` files.
        let mut png = RgbImage::new(64, 64);
        for p in png.pixels_mut() {
            p.0 = [0, 255, 0];
        }
        png.save_with_format(dir.path().join("west_75px.jpg"), image::ImageFormat::Png)
            .unwrap();
        // A wide view of the western tile wants the smallest version: the PNG.
        let wide = View {
            ra_degrees: 8.0,
            dec_degrees: 0.0,
            fov_degrees: 30.0,
            width_px: 96,
            height_px: 48,
            rotation_degrees: 0.0,
        };
        let img = image::load_from_memory(&map.render(&wide).unwrap())
            .unwrap()
            .to_rgb8();
        let at_west = img.get_pixel(48, 24).0;
        assert!(at_west[1] > 200 && at_west[0] < 60, "{at_west:?}");
        // Garbage in the small file: the next size up (the full tile) is used instead.
        std::fs::write(dir.path().join("west_75px.jpg"), b"not an image").unwrap();
        let img = image::load_from_memory(&map.render(&wide).unwrap())
            .unwrap()
            .to_rgb8();
        let at_west = img.get_pixel(48, 24).0;
        assert!(at_west[0] > 200 && at_west[2] < 60, "{at_west:?}");
        // Every version unreadable names the file.
        std::fs::write(dir.path().join("west.jpg"), b"still not an image").unwrap();
        let error = map.render(&wide).unwrap_err();
        assert!(error.contains("west.jpg"), "{error}");
    }
}
