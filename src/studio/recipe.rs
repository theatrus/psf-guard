//! Parts and recipes: how a picture is put together from its inputs.
//!
//! A *part* is a group of layers that does one job in the picture: the
//! nebula from narrowband inputs, the color from red, green and blue, the
//! luminance, the stars, or emission lines added to a color picture. A part
//! is an ordinary group, and nothing about it is fixed once it is made: a
//! person may add a layer that shows any input to any part, move layers
//! between parts, or change a part's palette. A *recipe* is a set of parts
//! to start from.
//!
//! Setup, the page that shows what goes where, reads [`Document::routes`]
//! and so shows the layers as they are, however they were made.

use std::collections::BTreeMap;

use uuid::Uuid;

use super::document::{BlendMode, Document, LayerRecord, LayerSource};
use super::flow::stars_channel;
use super::palette::{band_layer, default_palette, Palette, PALETTES};

/// The narrowband inputs PSF Guard knows, with the hue each gets when it is
/// added to a color picture.
const LINES: [(&str, f32); 4] = [("ha", 0.0), ("nii", 12.0), ("sii", 350.0), ("oiii", 185.0)];

const COLOR: [&str; 3] = ["red", "green", "blue"];

const LUMINANCE: &str = "luminance";

/// A kind of part [`Document::add_part`] makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    /// The narrowband inputs through a palette.
    Nebula,
    /// Red, green and blue.
    Color,
    /// The luminance input, which replaces the lightness of what is
    /// beneath.
    Luminance,
    /// The stars the processing tools took out of one input, white,
    /// screened over the picture.
    Stars,
    /// The stars of red, green and blue, in color, screened over the
    /// picture.
    ColorStars,
    /// Narrowband inputs screened over a color picture, each in its hue.
    Lines,
    /// A group with nothing in it.
    Empty,
}

impl PartKind {
    pub const ALL: [PartKind; 7] = [
        PartKind::Nebula,
        PartKind::Color,
        PartKind::Luminance,
        PartKind::Stars,
        PartKind::ColorStars,
        PartKind::Lines,
        PartKind::Empty,
    ];

    /// The part's name, as a person reads it.
    pub fn label(self) -> &'static str {
        match self {
            PartKind::Nebula => "Nebula",
            PartKind::Color => "Color",
            PartKind::Luminance => "Luminance",
            PartKind::Stars => "Stars",
            PartKind::ColorStars => "RGB stars",
            PartKind::Lines => "Emission lines",
            PartKind::Empty => "Empty part",
        }
    }

    /// What the part does, in a sentence.
    pub fn help(self) -> &'static str {
        match self {
            PartKind::Nebula => "Narrowband inputs through a palette such as SHO or Foraxx.",
            PartKind::Color => "Red, green and blue as a color picture.",
            PartKind::Luminance => "The luminance input sets the lightness of the layers beneath.",
            PartKind::Stars => {
                "The stars the processing tools removed from one input, added back over the picture."
            }
            PartKind::ColorStars => {
                "The stars of red, green and blue, in their colors, over the picture."
            }
            PartKind::Lines => "Narrowband inputs added over a color picture, each in its own hue.",
            PartKind::Empty => "A group to fill by hand.",
        }
    }

    /// Whether the document's inputs can fill the part.
    pub fn fits(self, inputs: &[String]) -> bool {
        let has = |name: &str| inputs.iter().any(|input| input == name);
        match self {
            PartKind::Nebula => default_palette(&narrowband(inputs)).is_some(),
            PartKind::Color | PartKind::ColorStars => COLOR.iter().all(|name| has(name)),
            PartKind::Luminance => has(LUMINANCE),
            PartKind::Stars => !inputs.is_empty(),
            PartKind::Lines => !narrowband(inputs).is_empty(),
            PartKind::Empty => true,
        }
    }
}

/// The narrowband inputs among `inputs`.
fn narrowband(inputs: &[String]) -> Vec<String> {
    inputs
        .iter()
        .filter(|input| LINES.iter().any(|(line, _)| line == input))
        .cloned()
        .collect()
}

/// A set of parts to start a picture from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Recipe {
    pub name: &'static str,
    /// What the recipe makes, in a sentence.
    pub help: &'static str,
    /// The parts, bottom first.
    pub parts: &'static [PartKind],
}

pub const RECIPES: [Recipe; 7] = [
    Recipe {
        name: "Narrowband",
        help: "The narrowband inputs through a palette.",
        parts: &[PartKind::Nebula],
    },
    Recipe {
        name: "Narrowband with RGB stars",
        help: "A narrowband nebula with the stars of red, green and blue over it.",
        parts: &[PartKind::Nebula, PartKind::ColorStars],
    },
    Recipe {
        name: "Narrowband with its own stars",
        help: "A narrowband nebula with the stars the tools removed added back.",
        parts: &[PartKind::Nebula, PartKind::Stars],
    },
    Recipe {
        name: "RGB",
        help: "Red, green and blue.",
        parts: &[PartKind::Color],
    },
    Recipe {
        name: "LRGB",
        help: "Red, green and blue, with lightness from the luminance input.",
        parts: &[PartKind::Color, PartKind::Luminance],
    },
    Recipe {
        name: "RGB with emission lines",
        help: "Red, green and blue, with narrowband inputs added in their hues.",
        parts: &[PartKind::Color, PartKind::Lines],
    },
    Recipe {
        name: "LRGB with emission lines",
        help: "LRGB with narrowband inputs added in their hues.",
        parts: &[PartKind::Color, PartKind::Lines, PartKind::Luminance],
    },
];

impl Recipe {
    /// Whether the document's inputs can fill every part.
    pub fn fits(&self, inputs: &[String]) -> bool {
        self.parts.iter().all(|part| part.fits(inputs))
    }
}

/// The recipes the document's inputs can fill, the one that uses the most
/// inputs first.
pub fn recipes_for(inputs: &[String]) -> Vec<&'static Recipe> {
    let mut offered: Vec<&Recipe> = RECIPES
        .iter()
        .filter(|recipe| recipe.fits(inputs))
        .collect();
    offered.sort_by_key(|recipe| std::cmp::Reverse(recipe.parts.len()));
    offered
}

/// One layer that shows an input: where the input goes in the picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// The input, such as `ha`.
    pub input: String,
    /// What the layer shows: the input as prepared, or a state of it such
    /// as `ha@stars`.
    pub shows: String,
    pub layer: Uuid,
    /// The group the layer is in, if any.
    pub part: Option<Uuid>,
    /// The palette slot the layer fills, when a palette made it.
    pub slot: Option<String>,
}

impl Document {
    /// The groups, bottom first. Each is a part of the picture.
    pub fn parts(&self) -> Vec<&LayerRecord> {
        self.layers()
            .iter()
            .filter(|layer| layer.source == LayerSource::Group)
            .collect()
    }

    /// Where each input goes: one route for every layer that shows an
    /// input, bottom layer first. An input with no route is not in the
    /// picture.
    pub fn routes(&self) -> Vec<Route> {
        self.layers()
            .iter()
            .filter_map(|layer| {
                let LayerSource::Band { band } = &layer.source else {
                    return None;
                };
                let input = band.split('@').next().unwrap_or(band).to_string();
                Some(Route {
                    input,
                    shows: band.clone(),
                    layer: layer.id,
                    part: layer.group,
                    slot: layer.slot.clone(),
                })
            })
            .collect()
    }

    /// Adds an empty group on top of the layers and returns its id.
    pub fn add_group(&mut self, name: &str, blend: BlendMode) -> Uuid {
        let mut group = LayerRecord::new(name, LayerSource::Group);
        group.blend = blend;
        let id = group.id;
        self.insert_layer(usize::MAX, group)
            .expect("an empty group is a valid layer");
        id
    }

    /// Adds a layer that shows `shows` (an input, or a state of one such as
    /// `ha@stars`) on top of a part's layers, or on top of every layer when
    /// `part` is `None`. Nothing else in the document changes, so this is
    /// how an input joins a picture that is already set up.
    pub fn add_layer_showing(
        &mut self,
        shows: &str,
        part: Option<Uuid>,
        hue: f32,
        saturation: f32,
        blend: BlendMode,
    ) -> Uuid {
        let layer = band_layer(shows, hue, saturation, blend);
        let id = layer.id;
        self.insert_layer(usize::MAX, layer)
            .expect("a layer that shows an input is valid");
        if let Some(group) = part {
            // A group's members are directly beneath it, so the group's own
            // place is the top of its members.
            if let Some(at) = self.layers().iter().position(|layer| layer.id == group) {
                self.place_layer(id, at, Some(group));
            }
        }
        id
    }

    /// The hue and blend a new layer for `shows` gets by default: a
    /// narrowband input in its line's hue, screened; anything else white.
    pub fn default_tone(shows: &str) -> (f32, f32, BlendMode) {
        let input = shows.split('@').next().unwrap_or(shows);
        match LINES.iter().find(|(line, _)| *line == input) {
            Some((_, hue)) if !shows.contains('@') => (*hue, 1.0, BlendMode::Screen),
            _ => match COLOR.iter().position(|name| *name == input) {
                Some(at) => (at as f32 * 120.0, 1.0, BlendMode::Screen),
                None => (0.0, 0.0, BlendMode::Screen),
            },
        }
    }

    /// Adds a part of one kind on top of the layers, filled from `inputs`,
    /// and returns its group. `None` when the inputs cannot fill it.
    ///
    /// The first part of a document replaces what is beneath it, as the
    /// bottom of a stack does. A later Nebula or Color part does too, so a
    /// person sets its opacity or blend to mix it with what is beneath;
    /// Stars and Emission lines are screened, and Luminance sets lightness.
    pub fn add_part(&mut self, kind: PartKind, inputs: &[String]) -> Option<Uuid> {
        if !kind.fits(inputs) {
            return None;
        }
        let group = match kind {
            PartKind::Nebula => {
                let palette = default_palette(&narrowband(inputs))?;
                let group = self.add_group(kind.label(), BlendMode::Normal);
                self.apply_palette_in(Some(group), palette, &BTreeMap::new());
                group
            }
            PartKind::Color => {
                let group = self.add_group(kind.label(), BlendMode::Normal);
                self.apply_palette_in(Some(group), rgb(), &BTreeMap::new());
                group
            }
            PartKind::Luminance => {
                let group = self.add_group(kind.label(), BlendMode::Luminance);
                self.add_layer_showing(LUMINANCE, Some(group), 0.0, 0.0, BlendMode::Normal);
                group
            }
            PartKind::Stars => {
                let group = self.add_group(kind.label(), BlendMode::Screen);
                // The stars of the first narrowband input, or of the first
                // input when none is narrowband. A person may change which.
                let first = narrowband(inputs)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| inputs[0].clone());
                self.add_layer_showing(
                    &stars_channel(&first),
                    Some(group),
                    0.0,
                    0.0,
                    BlendMode::Normal,
                );
                group
            }
            PartKind::ColorStars => {
                let group = self.add_group(kind.label(), BlendMode::Screen);
                let stars: BTreeMap<String, String> = COLOR
                    .iter()
                    .map(|name| (name.to_string(), stars_channel(name)))
                    .collect();
                self.apply_palette_in(Some(group), rgb(), &stars);
                group
            }
            PartKind::Lines => {
                let group = self.add_group(kind.label(), BlendMode::Screen);
                for input in narrowband(inputs) {
                    let (hue, saturation, blend) = Self::default_tone(&input);
                    self.add_layer_showing(&input, Some(group), hue, saturation, blend);
                }
                group
            }
            PartKind::Empty => self.add_group("Part", BlendMode::Normal),
        };
        Some(group)
    }

    /// Replaces the layers that show inputs, and every group, with a
    /// recipe's parts. Adjustment layers stay, in no group, above the new
    /// parts. Returns whether the inputs can fill the recipe; when they
    /// cannot, nothing changes.
    pub fn use_recipe(&mut self, recipe: &Recipe, inputs: &[String]) -> bool {
        if !recipe.fits(inputs) {
            return false;
        }
        let gone: Vec<Uuid> = self
            .layers()
            .iter()
            .filter(|layer| matches!(layer.source, LayerSource::Band { .. } | LayerSource::Group))
            .map(|layer| layer.id)
            .collect();
        for id in gone {
            self.remove_layer(id);
        }
        let palette_masks: Vec<Uuid> = self
            .masks()
            .iter()
            .filter(|mask| !mask.slots.is_empty())
            .map(|mask| mask.id)
            .collect();
        for id in palette_masks {
            self.remove_mask(id);
        }
        self.palette = None;
        let kept = self.layers().len();
        let mut made = 0;
        for kind in recipe.parts {
            let Some(group) = self.add_part(*kind, inputs) else {
                continue;
            };
            // The part goes beneath the adjustment layers that stayed.
            let members = self.group_members(group).count();
            self.place_layer(group, made, None);
            made += members + 1;
        }
        debug_assert_eq!(self.layers().len(), kept + made);
        true
    }
}

fn rgb() -> &'static Palette {
    PALETTES
        .iter()
        .find(|palette| palette.name == "RGB")
        .expect("the RGB palette is in the table")
}
