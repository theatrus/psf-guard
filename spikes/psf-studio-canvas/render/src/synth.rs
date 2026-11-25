//! A synthetic eight-band, 600-megapixel document.
//!
//! Tiles are computed on request from a deterministic function of document
//! coordinates, so the spike needs no multi-gigabyte file. The work per tile
//! stands in for decoding a tile from disk; it does not measure disk reads,
//! which belong to the tile-format investigation.

use half::f16;

pub const DOC_WIDTH: u32 = 30_000;
pub const DOC_HEIGHT: u32 = 20_000;
pub const BANDS: usize = 8;
pub const TILE: u32 = 512;

/// The coarsest pyramid level: the whole document fits in one tile.
pub const MAX_LEVEL: u8 = {
    let mut level = 0u8;
    let longest = if DOC_WIDTH > DOC_HEIGHT {
        DOC_WIDTH
    } else {
        DOC_HEIGHT
    };
    while (longest >> level) > TILE {
        level += 1;
    }
    level
};

/// One tile of the pyramid. Level 0 is full resolution; each level halves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileKey {
    pub level: u8,
    pub x: u16,
    pub y: u16,
}

impl TileKey {
    /// The tile's rectangle in document pixels, clipped to the document.
    pub fn doc_rect(self) -> [f32; 4] {
        let span = (TILE << self.level) as f32;
        let x0 = self.x as f32 * span;
        let y0 = self.y as f32 * span;
        [
            x0,
            y0,
            (x0 + span).min(DOC_WIDTH as f32),
            (y0 + span).min(DOC_HEIGHT as f32),
        ]
    }

    pub fn parent(self) -> Option<TileKey> {
        (self.level < MAX_LEVEL).then(|| TileKey {
            level: self.level + 1,
            x: self.x / 2,
            y: self.y / 2,
        })
    }
}

pub fn tiles_across(level: u8) -> (u32, u32) {
    let span = TILE << level;
    (DOC_WIDTH.div_ceil(span), DOC_HEIGHT.div_ceil(span))
}

fn hash(mut v: u64) -> u64 {
    v ^= v >> 33;
    v = v.wrapping_mul(0xff51_afd7_ed55_8ccd);
    v ^= v >> 33;
    v = v.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    v ^ (v >> 33)
}

fn unit(v: u64) -> f32 {
    (hash(v) >> 40) as f32 / (1u64 << 24) as f32
}

fn lattice(ix: i64, iy: i64, seed: u64) -> f32 {
    unit((ix as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (iy as u64).rotate_left(32) ^ seed)
}

fn value_noise(x: f32, y: f32, seed: u64) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let (ix, iy) = (fx as i64, fy as i64);
    let top = lattice(ix, iy, seed) * (1.0 - sx) + lattice(ix + 1, iy, seed) * sx;
    let bottom = lattice(ix, iy + 1, seed) * (1.0 - sx) + lattice(ix + 1, iy + 1, seed) * sx;
    top * (1.0 - sy) + bottom * sy
}

fn fbm(x: f32, y: f32, seed: u64) -> f32 {
    let mut sum = 0.0;
    let mut amplitude = 0.5;
    let mut frequency = 1.0;
    for octave in 0..4 {
        sum += amplitude * value_noise(x * frequency, y * frequency, seed + octave);
        amplitude *= 0.5;
        frequency *= 2.1;
    }
    sum
}

/// How strongly each band responds to the three nebula fields.
const BAND_MIX: [[f32; 3]; BANDS] = [
    [0.5, 0.3, 0.2],
    [0.9, 0.1, 0.0],
    [0.1, 0.8, 0.1],
    [0.1, 0.1, 0.8],
    [1.0, 0.0, 0.1],
    [0.0, 1.0, 0.2],
    [0.3, 0.0, 1.0],
    [0.6, 0.4, 0.0],
];

const STAR_CELL: u32 = 256;

/// Computes one tile: `BANDS` layers of `TILE` x `TILE` half-float samples,
/// layer after layer. Samples outside the document are zero.
pub fn generate_tile(key: TileKey) -> Vec<u16> {
    let tile = TILE as usize;
    let scale = (1u32 << key.level) as f32;
    let origin_x = key.x as f32 * TILE as f32 * scale;
    let origin_y = key.y as f32 * TILE as f32 * scale;
    let mut bands = vec![0f32; tile * tile * BANDS];

    for row in 0..tile {
        let doc_y = origin_y + (row as f32 + 0.5) * scale;
        if doc_y >= DOC_HEIGHT as f32 {
            break;
        }
        for column in 0..tile {
            let doc_x = origin_x + (column as f32 + 0.5) * scale;
            if doc_x >= DOC_WIDTH as f32 {
                break;
            }
            let (u, v) = (doc_x / 5000.0, doc_y / 5000.0);
            // Faint, sparse emission: most of the frame is near background,
            // as in a linear stack.
            let field = |value: f32| (value - 0.42).max(0.0).powi(2) * 0.6;
            let fields = [
                field(fbm(u, v, 11)),
                field(fbm(u + 7.3, v - 2.1, 23)),
                field(fbm(u - 4.4, v + 9.7, 37)),
            ];
            let pixel = ((doc_y as u64) << 32) | doc_x as u64;
            for band in 0..BANDS {
                let mix = BAND_MIX[band];
                let nebula = fields[0] * mix[0] + fields[1] * mix[1] + fields[2] * mix[2];
                // Read noise shrinks as levels average more pixels.
                let noise = (unit(pixel ^ ((band as u64) << 56)) - 0.5) * 0.008 / scale;
                bands[band * tile * tile + row * tile + column] = 0.02 + nebula + noise;
            }
        }
    }

    add_stars(&mut bands, origin_x, origin_y, scale);
    bands
        .into_iter()
        .map(|value| f16::from_f32(value).to_bits())
        .collect()
}

fn add_stars(bands: &mut [f32], origin_x: f32, origin_y: f32, scale: f32) {
    let tile = TILE as usize;
    let span = TILE as f32 * scale;
    let margin = 16.0 * scale.max(1.0);
    let cell = STAR_CELL as f32;
    let first_cx = ((origin_x - margin) / cell).floor().max(0.0) as u32;
    let first_cy = ((origin_y - margin) / cell).floor().max(0.0) as u32;
    let last_cx = (((origin_x + span + margin) / cell) as u32).min(DOC_WIDTH / STAR_CELL);
    let last_cy = (((origin_y + span + margin) / cell) as u32).min(DOC_HEIGHT / STAR_CELL);

    for cy in first_cy..=last_cy {
        for cx in first_cx..=last_cx {
            let seed = ((cy as u64) << 32 | cx as u64) ^ 0x5741_5253;
            let count = hash(seed) % 3;
            for index in 0..count {
                let star = seed.wrapping_mul(31).wrapping_add(index * 977);
                let doc_x = (cx as f32 + unit(star)) * cell;
                let doc_y = (cy as f32 + unit(star + 1)) * cell;
                let flux = unit(star + 2).powi(6) * 40.0 + 0.4;
                let sigma = 1.4 + unit(star + 3) * 1.2;
                // A star narrower than a level's pixel keeps its flux and
                // takes the pixel's width.
                let level_sigma = (sigma / scale).max(0.6);
                let amplitude = flux / (2.0 * std::f32::consts::PI * sigma * sigma)
                    * (sigma / scale / level_sigma).powi(2);
                if amplitude < 0.002 {
                    continue;
                }
                let center_x = (doc_x - origin_x) / scale;
                let center_y = (doc_y - origin_y) / scale;
                let reach = (level_sigma * 4.0).ceil();
                let x0 = (center_x - reach).floor().max(0.0) as usize;
                let y0 = (center_y - reach).floor().max(0.0) as usize;
                let x1 = ((center_x + reach).ceil().max(0.0) as usize).min(tile);
                let y1 = ((center_y + reach).ceil().max(0.0) as usize).min(tile);
                let tint = unit(star + 4);
                for row in y0..y1 {
                    for column in x0..x1 {
                        let dx = column as f32 + 0.5 - center_x;
                        let dy = row as f32 + 0.5 - center_y;
                        let value = amplitude
                            * (-(dx * dx + dy * dy) / (2.0 * level_sigma * level_sigma)).exp();
                        for band in 0..BANDS {
                            let response = 0.6 + 0.4 * (tint * (band as f32 + 1.0)).sin().abs();
                            bands[band * tile * tile + row * tile + column] += value * response;
                        }
                    }
                }
            }
        }
    }
}
