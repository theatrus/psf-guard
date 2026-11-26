use super::{RegionReader, StudioError};
use crate::image_steps::Region;

/// The most sample memory one read may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryBudget {
    /// The limit in bytes.
    pub bytes: u64,
    /// The smallest tile side, in pixels, that a read may be cut down to.
    /// Below this a step's padding costs more than the tile holds.
    pub smallest_tile: usize,
}

impl MemoryBudget {
    /// A budget of `bytes`, with tiles no smaller than 256 pixels a side.
    pub fn new(bytes: u64) -> Self {
        Self {
            bytes,
            smallest_tile: 256,
        }
    }
}

/// Reads a region within a memory budget.
///
/// The budget covers what the reader needs while it works, not the result,
/// which the caller asked for and must hold. When the region as one read
/// would exceed the budget, it is cut into tiles. Each tile is read with its
/// own padding and written into the result, so the result equals an uncut
/// read. A tile of the smallest size that still exceeds the budget is
/// refused.
pub fn read_within(
    reader: &dyn RegionReader,
    region: Region,
    budget: MemoryBudget,
) -> Result<Vec<f32>, StudioError> {
    let shape = reader.shape();
    region.check(shape)?;
    if reader.peak_bytes(region) <= budget.bytes {
        return reader.read(region);
    }

    // Halve the tile side until every tile fits. A tile inside the image
    // costs more than one at its edge, where padding is cut off, so each is
    // priced.
    let mut side = region.width.max(region.height);
    let tiles = loop {
        let tiles = tiles_of(region, side);
        let needed = tiles
            .iter()
            .map(|tile| reader.peak_bytes(*tile))
            .max()
            .unwrap_or(0);
        if needed <= budget.bytes {
            break tiles;
        }
        if side <= budget.smallest_tile {
            return Err(StudioError::OverBudget {
                tile: side,
                needed,
                limit: budget.bytes,
            });
        }
        side = side.div_ceil(2).max(budget.smallest_tile);
    };

    let result_shape =
        crate::image_steps::ImageShape::new(region.width, region.height, shape.channels);
    let mut result = vec![0.0; region.width * region.height * shape.channels];
    for tile in tiles {
        let samples = reader.read(tile)?;
        Region::new(
            tile.x - region.x,
            tile.y - region.y,
            tile.width,
            tile.height,
        )
        .paste_into(&samples, &mut result, result_shape)?;
    }
    Ok(result)
}

/// `region` cut into tiles of at most `side` pixels a side, row by row.
fn tiles_of(region: Region, side: usize) -> Vec<Region> {
    let mut tiles = Vec::new();
    let mut y = region.y;
    while y < region.bottom() {
        let height = side.min(region.bottom() - y);
        let mut x = region.x;
        while x < region.right() {
            let width = side.min(region.right() - x);
            tiles.push(Region::new(x, y, width, height));
            x += width;
        }
        y += height;
    }
    tiles
}
