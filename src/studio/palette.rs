//! Palettes: how a document's inputs become the layers of the picture.
//!
//! An input is a channel stack, such as Ha or R. A layer shows one input in
//! one color, and one input may feed several layers. A palette is a starting
//! set of layers for the inputs a document has: RGB, LRGB, SHO and so on.
//! After that every layer's color is the person's to change, to any hue and
//! saturation, not only to pure red, green or blue.

use super::{BlendMode, Document, LayerRecord, LayerSource};

/// One layer a palette makes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaletteLayer {
    /// The input the layer shows, by PSF Guard's role name, such as `ha`.
    pub channel: &'static str,
    /// The layer's hue in degrees: 0 is red, 120 green, 240 blue.
    pub hue: f32,
    /// From 0, gray, to 1, the pure hue.
    pub saturation: f32,
    pub blend: BlendMode,
}

/// A starting set of layers, bottom first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The palette's name, as a person reads it.
    pub name: &'static str,
    pub layers: &'static [PaletteLayer],
}

const fn tint(channel: &'static str, hue: f32, saturation: f32) -> PaletteLayer {
    PaletteLayer {
        channel,
        hue,
        saturation,
        blend: BlendMode::Add,
    }
}

const LUMINANCE: PaletteLayer = PaletteLayer {
    channel: "luminance",
    hue: 0.0,
    saturation: 0.0,
    blend: BlendMode::Luminance,
};

/// The palettes Studio offers. A document is offered those whose inputs it
/// has.
pub const PALETTES: [Palette; 7] = [
    Palette {
        name: "RGB",
        layers: &[
            tint("red", 0.0, 1.0),
            tint("green", 120.0, 1.0),
            tint("blue", 240.0, 1.0),
        ],
    },
    Palette {
        name: "LRGB",
        layers: &[
            tint("red", 0.0, 1.0),
            tint("green", 120.0, 1.0),
            tint("blue", 240.0, 1.0),
            LUMINANCE,
        ],
    },
    Palette {
        name: "SHO",
        layers: &[
            tint("sii", 0.0, 1.0),
            tint("ha", 120.0, 1.0),
            tint("oiii", 240.0, 1.0),
        ],
    },
    Palette {
        name: "SHO, gold and teal",
        layers: &[
            tint("sii", 18.0, 1.0),
            tint("ha", 45.0, 0.9),
            tint("oiii", 195.0, 1.0),
        ],
    },
    Palette {
        name: "HSO",
        layers: &[
            tint("ha", 0.0, 1.0),
            tint("sii", 120.0, 1.0),
            tint("oiii", 240.0, 1.0),
        ],
    },
    Palette {
        name: "HOO",
        layers: &[tint("ha", 0.0, 1.0), tint("oiii", 180.0, 1.0)],
    },
    Palette {
        name: "HOO, red and teal",
        layers: &[tint("ha", 8.0, 1.0), tint("oiii", 190.0, 0.9)],
    },
];

/// The palettes whose inputs are all among `channels`.
pub fn palettes_for(channels: &[String]) -> Vec<&'static Palette> {
    PALETTES
        .iter()
        .filter(|palette| {
            palette
                .layers
                .iter()
                .all(|layer| channels.iter().any(|channel| channel == layer.channel))
        })
        .collect()
}

/// The palette a new document starts with: the one that uses the most of
/// its inputs, and the first of those in [`PALETTES`].
pub fn default_palette(channels: &[String]) -> Option<&'static Palette> {
    let offered = palettes_for(channels);
    let most = offered.iter().map(|palette| palette.layers.len()).max()?;
    offered
        .into_iter()
        .find(|palette| palette.layers.len() == most)
}

/// An input's name as a person reads it: `Ha` for `ha`, `R` for `red`.
pub fn channel_label(channel: &str) -> String {
    match channel {
        "luminance" => "L".to_string(),
        "red" => "R".to_string(),
        "green" => "G".to_string(),
        "blue" => "B".to_string(),
        "ha" => "Ha".to_string(),
        "oiii" => "OIII".to_string(),
        "sii" => "SII".to_string(),
        "nii" => "NII".to_string(),
        other => other.to_string(),
    }
}

/// The color with this hue in degrees, saturation and brightest component.
pub fn color_from_hue(hue: f32, saturation: f32, value: f32) -> [f32; 3] {
    let sector = hue.rem_euclid(360.0) / 60.0;
    let chroma = value * saturation.clamp(0.0, 1.0);
    let second = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let [r, g, b] = match sector as u32 {
        0 => [chroma, second, 0.0],
        1 => [second, chroma, 0.0],
        2 => [0.0, chroma, second],
        3 => [0.0, second, chroma],
        4 => [second, 0.0, chroma],
        _ => [chroma, 0.0, second],
    };
    let floor = value - chroma;
    [r + floor, g + floor, b + floor]
}

/// A color's hue in degrees, its saturation, and its brightest component.
/// A gray has hue 0.
pub fn hue_of(color: [f32; 3]) -> (f32, f32, f32) {
    let value = color.iter().copied().fold(0.0, f32::max);
    let least = color.iter().copied().fold(f32::INFINITY, f32::min);
    let chroma = value - least;
    if value <= 0.0 || chroma <= value * 1e-4 {
        return (0.0, 0.0, value.max(0.0));
    }
    let [r, g, b] = color;
    let sector = if value == r {
        ((g - b) / chroma).rem_euclid(6.0)
    } else if value == g {
        (b - r) / chroma + 2.0
    } else {
        (r - g) / chroma + 4.0
    };
    (sector * 60.0, chroma / value, value)
}

/// The everyday word for a color, for a layer's name.
pub fn color_word(color: [f32; 3]) -> &'static str {
    let (hue, saturation, _) = hue_of(color);
    if saturation < 0.15 {
        return "gray";
    }
    match hue {
        hue if hue < 12.0 => "red",
        hue if hue < 35.0 => "orange",
        hue if hue < 52.0 => "gold",
        hue if hue < 70.0 => "yellow",
        hue if hue < 160.0 => "green",
        hue if hue < 200.0 => "teal",
        hue if hue < 255.0 => "blue",
        hue if hue < 290.0 => "violet",
        hue if hue < 345.0 => "magenta",
        _ => "red",
    }
}

/// The name a layer gets from what it shows: "Ha as gold", "L as
/// luminance".
pub fn layer_name(channel: &str, color: [f32; 3], blend: BlendMode) -> String {
    let label = channel_label(channel);
    if blend == BlendMode::Luminance {
        format!("{label} as luminance")
    } else {
        format!("{label} as {}", color_word(color))
    }
}

/// A new layer that shows `channel` in the color of this hue and saturation.
pub fn band_layer(channel: &str, hue: f32, saturation: f32, blend: BlendMode) -> LayerRecord {
    let color = color_from_hue(hue, saturation, 1.0);
    let mut layer = LayerRecord::new(
        &layer_name(channel, color, blend),
        LayerSource::Band {
            band: channel.to_string(),
        },
    );
    layer.blend = blend;
    layer.color = Some(color);
    layer
}

impl Document {
    /// Replaces the layers that show an input with the palette's, and keeps
    /// every other layer above them in its order.
    ///
    /// A palette layer takes over the mask and the own steps of the first
    /// replaced layer that showed the same input, so choosing another
    /// palette does not discard that work.
    ///
    /// The added layers' colors are scaled together so that their sum is
    /// full scale in its brightest component.
    pub fn apply_palette(&mut self, palette: &Palette) {
        let old: Vec<LayerRecord> = self.layers().to_vec();
        for layer in &old {
            self.remove_layer(layer.id);
        }
        let (mut replaced, kept): (Vec<_>, Vec<_>) = old
            .into_iter()
            .partition(|layer| matches!(layer.source, LayerSource::Band { .. }));
        let mut layers: Vec<LayerRecord> = palette
            .layers
            .iter()
            .map(|entry| {
                let mut layer = band_layer(entry.channel, entry.hue, entry.saturation, entry.blend);
                let earlier = replaced.iter().position(|old| {
                    matches!(&old.source, LayerSource::Band { band } if band == entry.channel)
                });
                if let Some(at) = earlier {
                    let old = replaced.remove(at);
                    layer.mask = old.mask;
                    layer.steps = old.steps;
                    layer.group = old.group;
                    layer.clipped = old.clipped;
                }
                layer
            })
            .collect();
        balance_colors(&mut layers);
        for layer in layers.into_iter().chain(kept) {
            self.insert_layer(usize::MAX, layer)
                .expect("a palette's layers and a document's own layers are valid");
        }
    }

    /// Whether the document's layers that show an input are the palette's,
    /// in its order, with its hues and blend modes.
    pub fn uses_palette(&self, palette: &Palette) -> bool {
        let shown: Vec<&LayerRecord> = self
            .layers()
            .iter()
            .filter(|layer| matches!(layer.source, LayerSource::Band { .. }))
            .collect();
        shown.len() == palette.layers.len()
            && shown.iter().zip(palette.layers).all(|(layer, entry)| {
                let (hue, saturation, _) = hue_of(layer.color.unwrap_or([1.0; 3]));
                let turn = (hue - entry.hue).rem_euclid(360.0);
                matches!(&layer.source, LayerSource::Band { band } if band == entry.channel)
                    && layer.blend == entry.blend
                    && (saturation - entry.saturation).abs() < 0.02
                    && (entry.saturation == 0.0 || turn.min(360.0 - turn) < 1.0)
            })
    }
}

/// Scales the colors of the layers that add together, so that their sum is
/// full scale in its brightest component.
pub fn balance_colors(layers: &mut [LayerRecord]) {
    let adds = |layer: &LayerRecord| layer.blend == BlendMode::Add && layer.color.is_some();
    let brightest = (0..3)
        .map(|component| {
            layers
                .iter()
                .filter(|layer| adds(layer))
                .map(|layer| layer.color.expect("checked by adds")[component])
                .sum::<f32>()
        })
        .fold(0.0, f32::max);
    if brightest <= 0.0 {
        return;
    }
    for layer in layers.iter_mut().filter(|layer| adds(layer)) {
        layer.color = layer
            .color
            .map(|color| color.map(|component| component / brightest));
    }
}
