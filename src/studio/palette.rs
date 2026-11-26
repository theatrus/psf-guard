//! Palettes: how a document's inputs become the layers of the picture.
//!
//! An input is a channel stack, such as Ha or R. A layer shows one input in
//! one color, and one input may feed several layers. A palette is a starting
//! set of layers for the inputs a document has: RGB, LRGB, SHO and so on.
//! After that every layer's color is the person's to change, to any hue and
//! saturation, not only to pure red, green or blue.

use super::{BlendMode, Document, LayerRecord, LayerSource, MaskRecord, PaletteUse, SharedMask};
use std::collections::BTreeMap;

/// One layer a palette makes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaletteLayer {
    /// The slot the layer shows. A slot is named after the input it shows
    /// unless a person gives it another: PSF Guard's role name, such as
    /// `ha`.
    pub channel: &'static str,
    /// The layer's hue in degrees: 0 is red, 120 green, 240 blue.
    pub hue: f32,
    /// From 0, gray, to 1, the pure hue.
    pub saturation: f32,
    pub blend: BlendMode,
    /// Where the layer shows, when that follows the inputs' signal. `None`
    /// is everywhere.
    pub mask: Option<PaletteMask>,
}

/// A palette layer's mask: the shared mask of these slots, computed at each
/// pixel as the power of inverted pixels of the product of the slots'
/// stretched samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaletteMask {
    /// The slots multiplied.
    pub inputs: &'static [&'static str],
    /// Whether the layer shows where the product is weak.
    pub inverted: bool,
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
        mask: None,
    }
}

/// A tint that shows where the product of `inputs` is strong, or where it
/// is weak when `inverted`.
const fn where_strong(
    channel: &'static str,
    hue: f32,
    inputs: &'static [&'static str],
    inverted: bool,
) -> PaletteLayer {
    PaletteLayer {
        channel,
        hue,
        saturation: 1.0,
        blend: BlendMode::Add,
        mask: Some(PaletteMask { inputs, inverted }),
    }
}

const LUMINANCE: PaletteLayer = PaletteLayer {
    channel: "luminance",
    hue: 0.0,
    saturation: 0.0,
    blend: BlendMode::Luminance,
    mask: None,
};

/// The palettes Studio offers. A document is offered those whose inputs it
/// has.
pub const PALETTES: [Palette; 9] = [
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
    // The Foraxx palettes mix inputs by their signal at each pixel. Red of
    // SHO is SII where OIII is strong and Ha elsewhere; green is Ha where
    // both Ha and OIII are strong and OIII elsewhere; blue is OIII. They
    // are meant for stretched, starless inputs.
    Palette {
        name: "SHO, Foraxx",
        layers: &[
            where_strong("sii", 0.0, &["oiii"], false),
            where_strong("ha", 0.0, &["oiii"], true),
            where_strong("ha", 120.0, &["ha", "oiii"], false),
            where_strong("oiii", 120.0, &["ha", "oiii"], true),
            tint("oiii", 240.0, 1.0),
        ],
    },
    Palette {
        name: "HOO, Foraxx",
        layers: &[
            tint("ha", 0.0, 1.0),
            where_strong("ha", 120.0, &["ha", "oiii"], false),
            where_strong("oiii", 120.0, &["ha", "oiii"], true),
            tint("oiii", 240.0, 1.0),
        ],
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

impl Palette {
    /// The palette's slots: the inputs its layers show and its masks read,
    /// before a person gives a slot another input.
    pub fn slots(&self) -> Vec<&'static str> {
        let mut slots: Vec<&'static str> = self
            .layers
            .iter()
            .flat_map(|layer| {
                let masked = layer.mask.map_or(&[][..], |mask| mask.inputs);
                [layer.channel].into_iter().chain(masked.iter().copied())
            })
            .collect();
        slots.sort_unstable();
        slots.dedup();
        slots
    }
}

/// The palette a new document starts with: the one that uses the most of
/// its inputs, and the first of those in [`PALETTES`].
pub fn default_palette(channels: &[String]) -> Option<&'static Palette> {
    let offered = palettes_for(channels);
    let most = offered.iter().map(|palette| palette.slots().len()).max()?;
    offered
        .into_iter()
        .find(|palette| palette.slots().len() == most)
}

/// An input's name as a person reads it: `Ha` for `ha`, `R` for `red`.
pub fn channel_label(channel: &str) -> String {
    // The stars a star removal took out of an input.
    if let Some(input) = channel.strip_suffix("@stars") {
        return format!("{} stars", channel_label(input));
    }
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

/// Whether a layer shows an input as prepared, not a state of one such
/// as its stars.
fn shows_an_input(layer: &LayerRecord) -> bool {
    matches!(&layer.source, LayerSource::Band { band } if !band.contains('@'))
}

impl Document {
    /// Replaces the top-level layers that show an input with the palette's,
    /// each slot showing the input of the same name. Every other layer
    /// keeps its place above them. See [`Self::apply_palette_in`].
    pub fn apply_palette(&mut self, palette: &Palette) {
        self.apply_palette_in(None, palette, &BTreeMap::new());
    }

    /// Gives one slot of the top-level palette another input. See
    /// [`Self::set_palette_input_in`].
    pub fn set_palette_input(&mut self, slot: &str, input: &str) -> bool {
        self.set_palette_input_in(None, slot, input)
    }

    /// Whether the top-level layers were made from this palette and are
    /// still as many as it makes.
    pub fn uses_palette(&self, palette: &Palette) -> bool {
        self.uses_palette_in(None, palette)
    }

    /// The palette the layers of `part` were made from, with the input each
    /// slot shows. `None` for `part` is the layers in no group.
    pub fn palette_in(&self, part: Option<uuid::Uuid>) -> Option<&PaletteUse> {
        match part {
            None => self.palette.as_ref(),
            Some(id) => self
                .layers()
                .iter()
                .find(|layer| layer.id == id)?
                .palette
                .as_ref(),
        }
    }

    /// Gives one slot of a part's palette another input, and remakes the
    /// layers and the masks that show or read that slot. Each layer keeps
    /// its color, opacity, own steps and place; each shared mask keeps its
    /// reach, smoothing and touch-ups.
    ///
    /// Returns whether the part has a palette with that slot.
    pub fn set_palette_input_in(
        &mut self,
        part: Option<uuid::Uuid>,
        slot: &str,
        input: &str,
    ) -> bool {
        let Some(used) = self.palette_in(part).cloned() else {
            return false;
        };
        let Some(palette) = PALETTES.iter().find(|palette| palette.name == used.name) else {
            return false;
        };
        if !palette.slots().contains(&slot) {
            return false;
        }
        let mut inputs = used.inputs;
        inputs.insert(slot.to_string(), input.to_string());
        self.apply_palette_in(part, palette, &inputs);
        true
    }

    /// Replaces the layers of one part that show an input with the
    /// palette's. `part` is a group, or `None` for the layers in no group.
    /// `inputs` says which input each slot shows; a slot it leaves out
    /// shows the input of its own name.
    ///
    /// Only that part's layers are touched, so a document may hold several
    /// parts with a palette each, and layers a person added by hand beside
    /// them. Within the part:
    ///
    /// - The same palette again, with other inputs, keeps each layer as it
    ///   was (color, opacity, own steps, painted mask), showing its slot's
    ///   input as it is now.
    /// - Another palette gives each new layer the painted mask and own
    ///   steps of the first replaced layer that showed the same input.
    /// - Layers no palette made, such as adjustments, stars and an input a
    ///   person added, stay above the new layers in their order.
    /// - A shared mask the palette made before is kept, with its reach,
    ///   smoothing and touch-ups, and reads its slots' inputs as they are
    ///   now. One no layer uses any more goes.
    pub fn apply_palette_in(
        &mut self,
        part: Option<uuid::Uuid>,
        palette: &Palette,
        inputs: &BTreeMap<String, String>,
    ) {
        let input_of = |slot: &str| {
            inputs
                .get(slot)
                .cloned()
                .unwrap_or_else(|| slot.to_string())
        };
        let remade = self
            .palette_in(part)
            .is_some_and(|used| used.name == palette.name);
        let mut replaced: Vec<LayerRecord> =
            self.palette_layers(part).into_iter().cloned().collect();
        // Where each replaced layer is in the stack, so a layer the palette
        // keeps goes back to the same place.
        let places: Vec<(uuid::Uuid, usize)> = replaced
            .iter()
            .filter_map(|layer| {
                let at = self
                    .layers()
                    .iter()
                    .position(|other| other.id == layer.id)?;
                Some((layer.id, at))
            })
            .collect();
        for layer in &replaced {
            self.remove_layer(layer.id);
        }
        // The same palette again keeps each layer it made: the layer that
        // fills the same slot, wherever a person has moved it in the part.
        // Two entries of one slot, as Foraxx has, pair in order.
        let kept: Option<Vec<LayerRecord>> = remade
            .then(|| {
                let mut left = replaced.clone();
                palette
                    .layers
                    .iter()
                    .map(|entry| {
                        let at = left
                            .iter()
                            .position(|layer| layer.slot.as_deref() == Some(entry.channel))?;
                        Some(left.remove(at))
                    })
                    .collect::<Option<Vec<_>>>()
                    .filter(|_| left.is_empty())
            })
            .flatten();
        let by_place = kept.is_some();

        // The palette's masks: one per set of slots, for this part.
        let mut masks: Vec<(&'static [&'static str], uuid::Uuid)> = Vec::new();
        for needed in palette.layers.iter().filter_map(|layer| layer.mask) {
            if masks.iter().any(|(slots, _)| *slots == needed.inputs) {
                continue;
            }
            let slots: Vec<String> = needed.inputs.iter().map(|slot| slot.to_string()).collect();
            let read: Vec<String> = needed.inputs.iter().map(|slot| input_of(slot)).collect();
            let known = self
                .masks()
                .iter()
                .find(|mask| mask.slots == slots && mask.part == part)
                .map(|mask| mask.id);
            let id = match known {
                Some(id) => {
                    let mask = self.mask_mut(id).expect("the mask was just found");
                    mask.name = SharedMask::name_for(&read);
                    mask.inputs = read;
                    id
                }
                None => self.add_mask(SharedMask {
                    slots,
                    part,
                    ..SharedMask::new(read)
                }),
            };
            masks.push((needed.inputs, id));
        }

        let mut layers: Vec<LayerRecord> = Vec::new();
        for (at, entry) in palette.layers.iter().enumerate() {
            let input = input_of(entry.channel);
            let mut layer = band_layer(&input, entry.hue, entry.saturation, entry.blend);
            if by_place {
                let was = kept.as_ref().expect("checked just above")[at].clone();
                let color = was.color.unwrap_or([1.0; 3]);
                layer = LayerRecord {
                    name: layer_name(&input, color, was.blend),
                    source: layer.source,
                    ..was
                };
            } else {
                let earlier = replaced.iter().position(
                    |old| matches!(&old.source, LayerSource::Band { band } if *band == input),
                );
                if let Some(earlier) = earlier {
                    let old = replaced.remove(earlier);
                    layer.steps = old.steps;
                    layer.clipped = old.clipped;
                    layer.mask = old.mask;
                }
            }
            // A painted mask, or a shared mask a person chose, is the
            // person's work and is kept. A mask this part's palette made
            // belongs to the palette.
            if let Some(MaskRecord::Shared { mask, .. }) = layer.mask {
                let made_here = self
                    .mask(mask)
                    .is_some_and(|mask| mask.part == part && !mask.slots.is_empty());
                if made_here || self.mask(mask).is_none() {
                    layer.mask = None;
                }
            }
            if let Some(needed) = entry.mask {
                let (_, mask) = masks
                    .iter()
                    .find(|(slots, _)| *slots == needed.inputs)
                    .expect("every palette mask was made above");
                layer.mask = Some(MaskRecord::Shared {
                    mask: *mask,
                    inverted: needed.inverted,
                });
            }
            layer.group = part;
            layer.slot = Some(entry.channel.to_string());
            layers.push(layer);
        }
        if !by_place {
            balance_colors(&mut layers);
        }
        if by_place {
            // Each kept layer goes back where it was. Put back from the
            // lowest place up, the places are those of the stack before.
            let mut back: Vec<(usize, LayerRecord)> = layers
                .into_iter()
                .map(|layer| {
                    let at = places
                        .iter()
                        .find(|(id, _)| *id == layer.id)
                        .map_or(usize::MAX, |(_, at)| *at);
                    (at, layer)
                })
                .collect();
            back.sort_by_key(|(at, _)| *at);
            for (at, layer) in back {
                self.insert_layer(at, layer)
                    .expect("a palette's layers are valid");
            }
            layers = Vec::new();
        }
        // The new layers go at the bottom of their part, in order, so what
        // else the part holds stays above them.
        for (at, layer) in layers.into_iter().enumerate() {
            let id = layer.id;
            self.insert_layer(usize::MAX, layer)
                .expect("a palette's layers are valid");
            let bottom = match part {
                None => 0,
                Some(group) => {
                    let stack = self.layers();
                    stack
                        .iter()
                        .position(|other| other.id != id && other.group == Some(group))
                        .or_else(|| stack.iter().position(|other| other.id == group))
                        .unwrap_or(0)
                }
            };
            // At the top level the palette's layers go beneath everything.
            let position = match part {
                None => at,
                Some(_) => bottom + at,
            };
            self.place_layer(id, position, part);
        }
        // A mask this part's palette made before, which no layer uses now,
        // goes.
        let unused: Vec<uuid::Uuid> = self
            .masks()
            .iter()
            .filter(|mask| !mask.slots.is_empty() && mask.part == part)
            .filter(|mask| !self.mask_in_use(mask.id))
            .map(|mask| mask.id)
            .collect();
        for id in unused {
            self.remove_mask(id);
        }
        let used = PaletteUse {
            name: palette.name.to_string(),
            inputs: palette
                .slots()
                .into_iter()
                .map(|slot| (slot.to_string(), input_of(slot)))
                .collect(),
        };
        match part {
            None => self.palette = Some(used),
            Some(id) => {
                if let Some(group) = self.layer_mut(id) {
                    group.palette = Some(used);
                }
            }
        }
    }

    /// The layers of a part that a palette replaces when it is applied
    /// there: those a palette made. In a part no palette has made layers
    /// for, they are the layers that show an input as prepared, so a first
    /// palette takes over the layers a document starts with.
    fn palette_layers(&self, part: Option<uuid::Uuid>) -> Vec<&LayerRecord> {
        let in_part = || {
            self.layers()
                .iter()
                .filter(move |layer| layer.group == part)
        };
        // A layer is the part's palette's when it fills one of that
        // palette's slots.
        let slots: Vec<&str> = self
            .palette_in(part)
            .map(|used| used.inputs.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let made = |layer: &&LayerRecord| {
            layer
                .slot
                .as_deref()
                .is_some_and(|slot| slots.contains(&slot))
        };
        if in_part().any(|layer| made(&layer)) {
            in_part().filter(made).collect()
        } else {
            in_part().filter(|layer| shows_an_input(layer)).collect()
        }
    }

    /// Whether a part's layers were made from this palette and are still as
    /// many as it makes.
    pub fn uses_palette_in(&self, part: Option<uuid::Uuid>, palette: &Palette) -> bool {
        let shown = self.palette_layers(part).len();
        self.palette_in(part)
            .is_some_and(|used| used.name == palette.name)
            && shown == palette.layers.len()
    }
}

/// Scales the colors of the layers that add together, so that their sum is
/// full scale in its brightest component.
///
/// A layer behind a shared mask is left out and keeps its color: such
/// layers share a color between them by their masks, so they do not add
/// together everywhere.
pub fn balance_colors(layers: &mut [LayerRecord]) {
    let adds = |layer: &LayerRecord| {
        layer.blend == BlendMode::Add
            && layer.color.is_some()
            && !matches!(layer.mask, Some(MaskRecord::Shared { .. }))
    };
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
