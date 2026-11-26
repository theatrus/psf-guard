//! Where the canvas gets its tiles.

use crate::synth::{TileKey, TILE};

/// The size of a document and the number of levels in its pyramid. Level 0
/// is full resolution, and each further level halves the one before it,
/// rounding up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pyramid {
    pub width: u32,
    pub height: u32,
    /// The number of levels, counting level 0.
    pub levels: u8,
}

impl Pyramid {
    /// A pyramid whose last level fits in one tile.
    pub fn for_size(width: u32, height: u32) -> Self {
        let mut levels = 1u8;
        while (width.max(height) - 1) >> (levels - 1) >= TILE {
            levels += 1;
        }
        Self {
            width,
            height,
            levels,
        }
    }

    /// The coarsest level.
    pub fn max_level(&self) -> u8 {
        self.levels.saturating_sub(1)
    }

    /// How many tiles a level has across and down.
    pub fn tiles_across(&self, level: u8) -> (u32, u32) {
        let span = TILE << level;
        (self.width.div_ceil(span), self.height.div_ceil(span))
    }

    /// A tile's rectangle in document pixels, cut off at the document's
    /// edges.
    pub fn doc_rect(&self, key: TileKey) -> [f32; 4] {
        let span = (TILE << key.level) as f32;
        let x0 = key.x as f32 * span;
        let y0 = key.y as f32 * span;
        [
            x0,
            y0,
            (x0 + span).min(self.width as f32),
            (y0 + span).min(self.height as f32),
        ]
    }

    /// The tile one level coarser that covers this one.
    pub fn parent(&self, key: TileKey) -> Option<TileKey> {
        (key.level < self.max_level()).then(|| TileKey {
            level: key.level + 1,
            x: key.x / 2,
            y: key.y / 2,
        })
    }
}

/// A document the canvas can draw: its pyramid, its bands, and any tile of
/// either. Tiles are asked for on worker threads.
pub trait TileSource: Send + Sync {
    /// The document's size and levels.
    fn pyramid(&self) -> Pyramid;

    /// The number of bands. Every tile has this many layers.
    fn bands(&self) -> usize;

    /// One tile: `bands` layers of `TILE` by `TILE` half-float samples, layer
    /// after layer. Samples outside the document are zero.
    fn tile(&self, key: TileKey) -> Vec<u16>;
}
