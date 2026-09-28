//! N.I.N.A.'s offline sky maps as a survey source. The framing assistant's
//! `FramingAssistantCache` folders, and the whole-sky sets N.I.N.A. offers
//! for download, hold a `CacheInfo.xml` index of 5° tiles (gnomonic, north
//! up, east left, from HiPS2FITS) with a JPEG per tile at full size and at
//! 500, 150 and 75 px. This module reads such a folder and renders any view
//! from its tiles, so the framing view works with no network at all.

use image::{codecs::jpeg::JpegEncoder, ColorType, ImageEncoder, RgbImage};
use serde::Serialize;
use std::{
    collections::HashMap,
    fmt::Write as _,
    path::{Path, PathBuf},
};

/// Survey layer id prefix; the rest is the folder name.
pub const ID_PREFIX: &str = "nina:";

/// Where the maps live for a server cache root.
pub fn root_for(cache_root: &Path) -> PathBuf {
    cache_root.join("director").join("sky-maps")
}
const VARIANTS: [u32; 3] = [75, 150, 500];
const MAX_TILES_PER_RENDER: usize = 48;
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
    pub dir: PathBuf,
    pub tiles: Vec<Tile>,
    /// Unit vectors per tile: center, east, north.
    frames: Vec<[[f64; 3]; 3]>,
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
            dir: dir.to_path_buf(),
            tiles,
            frames,
        })
    }

    /// Render the view from the tiles around it. Black where no tile
    /// reaches. Runs on a blocking thread; a 2048 × 1536 view takes a few
    /// hundred milliseconds and reads a handful of tiles.
    pub fn render(&self, view: &View) -> Result<Vec<u8>, String> {
        let (w, h) = (view.width_px as usize, view.height_px as usize);
        if w == 0 || h == 0 {
            return Err("empty view".to_owned());
        }
        let scale = view.fov_degrees / w as f64; // degrees per pixel
        let view_radius = (view.fov_degrees.hypot(scale * h as f64) / 2.0).to_radians();
        let frame = unit(view.ra_degrees, view.dec_degrees);
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
        // Pick each tile's size: the smallest whose pixels are at least as fine as the view's.
        let mut images: HashMap<usize, RgbImage> = HashMap::new();
        for (index, _) in &candidates {
            let tile = &self.tiles[*index];
            let needed = (tile.fov_w_degrees / scale).ceil() as u32;
            let stem = tile.file_name.trim_end_matches(".jpg");
            let path = VARIANTS
                .iter()
                .filter(|v| **v >= needed)
                .map(|v| self.dir.join(format!("{stem}_{v}px.jpg")))
                .find(|p| p.exists())
                .unwrap_or_else(|| self.dir.join(&tile.file_name));
            let decoded = image::open(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?
                .to_rgb8();
            images.insert(*index, decoded);
        }
        let (sin_r, cos_r) = view.rotation_degrees.to_radians().sin_cos();
        let mut out = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                // Stage convention: east to the left, north up; then the view's turn.
                let xi0 = (w as f64 / 2.0 - (x as f64 + 0.5)) * scale;
                let eta0 = (h as f64 / 2.0 - (y as f64 + 0.5)) * scale;
                let xi = (xi0 * cos_r + eta0 * sin_r).to_radians();
                let eta = (-xi0 * sin_r + eta0 * cos_r).to_radians();
                let v = [
                    frame[0][0] + xi * frame[1][0] + eta * frame[2][0],
                    frame[0][1] + xi * frame[1][1] + eta * frame[2][1],
                    frame[0][2] + xi * frame[1][2] + eta * frame[2][2],
                ];
                let mut best: Option<(usize, f64, f64, f64)> = None;
                for (index, _) in &candidates {
                    let t = &self.frames[*index];
                    let d = dot(&v, &t[0]);
                    if d <= 1e-9 {
                        continue;
                    }
                    let tile = &self.tiles[*index];
                    let img = &images[index];
                    let (tw, th) = (img.width() as f64, img.height() as f64);
                    let xt = (dot(&v, &t[1]) / d).to_degrees();
                    let yt = (dot(&v, &t[2]) / d).to_degrees();
                    let px = tw / 2.0 - xt / (tile.fov_w_degrees / tw) - 0.5;
                    let py = th / 2.0 - yt / (tile.fov_h_degrees / th) - 0.5;
                    if px < -0.5 || py < -0.5 || px > tw - 0.5 || py > th - 0.5 {
                        continue;
                    }
                    if best.is_none_or(|(_, bd, _, _)| d > bd) {
                        best = Some((*index, d, px, py));
                    }
                }
                if let Some((index, _, px, py)) = best {
                    let rgb = sample(&images[&index], px, py);
                    let at = (y * w + x) * 3;
                    out[at..at + 3].copy_from_slice(&rgb);
                }
            }
        }
        let mut bytes = Vec::with_capacity(w * h / 4);
        JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
            .write_image(&out, w as u32, h as u32, ColorType::Rgb8.into())
            .map_err(|error| format!("encoding the view: {error}"))?;
        Ok(bytes)
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
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
}
