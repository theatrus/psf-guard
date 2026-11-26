use super::{Junction, PaintedMask, Stroke, StudioError};
use crate::image_steps::{Accepts, Domain};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// The version of the document format this build writes.
pub const DOCUMENT_VERSION: u32 = 1;

/// One stage of the workflow. A document has each stage once, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StageKind {
    /// Choosing stacks or masters, naming bands, registering and cropping.
    Sources,
    /// Work on linear samples: background, deconvolution, noise, stars,
    /// color calibration.
    Linear,
    /// From linear samples to display-referred ones.
    Stretch,
    /// Layers: each has its own color, mask, adjustments and blend. Editing
    /// that is specific to one band, to the stars, or to one region of one
    /// layer happens here.
    Combine,
    /// Adjustments to the combined image as a whole: curves, saturation,
    /// local contrast and retouching. A step here may still be masked, and
    /// it acts on every layer at once.
    Finish,
    /// Color space, renditions and the file written.
    Export,
}

impl StageKind {
    /// Every stage, in the order a document has them.
    pub const ALL: [StageKind; 6] = [
        StageKind::Sources,
        StageKind::Linear,
        StageKind::Stretch,
        StageKind::Combine,
        StageKind::Finish,
        StageKind::Export,
    ];

    /// The samples the stage's steps are given.
    pub fn works_on(self) -> Domain {
        match self {
            StageKind::Sources | StageKind::Linear | StageKind::Stretch => Domain::Linear,
            StageKind::Combine | StageKind::Finish | StageKind::Export => Domain::Display,
        }
    }

    /// Whether a person may paint masks and heal in this stage.
    pub fn allows_manual_edits(self) -> bool {
        !matches!(self, StageKind::Sources | StageKind::Export)
    }

    fn word(self) -> &'static str {
        match self {
            StageKind::Sources => "sources",
            StageKind::Linear => "linear",
            StageKind::Stretch => "stretch",
            StageKind::Combine => "combine",
            StageKind::Finish => "finish",
            StageKind::Export => "export",
        }
    }
}

impl fmt::Display for StageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

impl FromStr for StageKind {
    type Err = StudioError;

    fn from_str(word: &str) -> Result<Self, Self::Err> {
        StageKind::ALL
            .into_iter()
            .find(|stage| stage.word() == word)
            .ok_or_else(|| StudioError::Document(format!("{word:?} is not a stage")))
    }
}

/// What a step does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum StepKind {
    /// A named operation with its settings, such as a background model or a
    /// curve.
    Operation {
        /// The operation's name.
        operation: String,
        /// The samples the operation works on.
        accepts: Accepts,
        /// The operation's settings, in its own format.
        settings: serde_json::Value,
    },
    /// One stroke of the heal brush.
    Heal {
        /// The area healed.
        stroke: Stroke,
        /// Where the replacement comes from, relative to each healed pixel,
        /// in document pixels.
        offset: [i64; 2],
        /// The brightness added to each channel of the copied samples,
        /// measured when the stroke was made.
        shift: Vec<f32>,
    },
}

impl StepKind {
    /// The step's name, as a person reads it.
    pub fn name(&self) -> &str {
        match self {
            StepKind::Operation { operation, .. } => operation,
            StepKind::Heal { .. } => "Heal",
        }
    }

    fn accepts(&self) -> Accepts {
        match self {
            StepKind::Operation { accepts, .. } => *accepts,
            StepKind::Heal { .. } => Accepts::Either,
        }
    }

    fn is_manual_edit(&self) -> bool {
        matches!(self, StepKind::Heal { .. })
    }
}

/// Where a step takes effect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MaskRecord {
    /// A mask painted by hand.
    Painted(PaintedMask),
    /// A mask that selects pixels by value.
    Range {
        /// The lowest fully selected value.
        low: f32,
        /// The highest fully selected value.
        high: f32,
        /// How far outside `low` and `high` the mask reaches 0.
        feather: f32,
    },
    /// One of the document's shared masks, or its inverse. A shared mask is
    /// computed from inputs at each pixel, so it follows their signal; two
    /// layers that use a mask and its inverse share one color between them.
    Shared {
        /// The shared mask.
        mask: Uuid,
        /// Whether the layer shows where the mask is weak.
        inverted: bool,
    },
}

/// A mask computed from inputs, kept by the document so that several layers
/// can use it, or its inverse, and stay consistent when it is changed.
///
/// At each pixel the mask is the product of the inputs' stretched samples,
/// shaped and raised to `reach`. The result is smoothed, so the noise of
/// the inputs does not become speckle in the layers behind the mask, and
/// then touched up by hand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedMask {
    /// The mask's identity.
    pub id: Uuid,
    /// The mask's name, as a person reads it.
    pub name: String,
    /// The inputs whose stretched samples are multiplied, by PSF Guard's
    /// role names. One or two.
    pub inputs: Vec<String>,
    /// How the product becomes the mask.
    pub shape: MaskShape,
    /// The power the shaped mask is raised to. 1 leaves it as it is; below
    /// 1 the mask reaches further into faint signal, above 1 less far.
    pub reach: f32,
    /// How far the mask is smoothed: the standard deviation of a Gaussian
    /// blur, in document pixels. 0 leaves the mask as computed, noise and
    /// all.
    #[serde(default = "SharedMask::default_smoothing")]
    pub smoothing: f32,
    /// Strokes painted over the computed mask: an adding stroke sets the
    /// mask to 1 under it, a subtracting stroke to 0.
    #[serde(default)]
    pub touch_ups: Vec<Stroke>,
    /// The palette slots the mask was made for, such as `oiii`. A palette
    /// remakes such a mask when its slots are given other inputs. Empty for
    /// a mask a person made.
    #[serde(default)]
    pub slots: Vec<String>,
    /// The group whose palette made the mask, when it was a group's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<Uuid>,
}

impl SharedMask {
    /// A new mask of these inputs, with no touch-ups.
    pub fn new(inputs: Vec<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: Self::name_for(&inputs),
            inputs,
            shape: MaskShape::PowerOfInverse,
            reach: 1.0,
            smoothing: Self::default_smoothing(),
            touch_ups: Vec::new(),
            slots: Vec::new(),
            part: None,
        }
    }

    /// The smoothing a new mask has: enough to take the pixel noise of a
    /// stack out of the mask without softening the structures it follows.
    pub fn default_smoothing() -> f32 {
        4.0
    }

    /// The name a mask gets from its inputs: "OIII signal", "Ha × OIII
    /// signal".
    pub fn name_for(inputs: &[String]) -> String {
        let labels: Vec<String> = inputs
            .iter()
            .map(|input| super::channel_label(input))
            .collect();
        if labels.is_empty() {
            "Mask with no input".to_string()
        } else {
            format!("{} signal", labels.join(" × "))
        }
    }
}

/// The palette a document's layers were made from, and the input each of
/// its slots shows. A slot is one of the palette's own names, such as `ha`;
/// giving a slot another input remakes the layers and the masks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteUse {
    /// The palette's name.
    pub name: String,
    /// The input each slot shows, by slot.
    pub inputs: std::collections::BTreeMap<String, String>,
}

/// How the product of a computed mask's inputs becomes the mask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MaskShape {
    /// The product itself.
    Value,
    /// The product raised to the power of 1 minus itself, which lifts
    /// faint signal: the "power of inverted pixels" of dynamic narrowband
    /// palettes.
    #[default]
    PowerOfInverse,
}

impl MaskShape {
    /// The mask for a product of stretched samples, from 0 to 1.
    pub fn apply(self, product: f32) -> f32 {
        let product = product.clamp(0.0, 1.0);
        match self {
            MaskShape::Value => product,
            MaskShape::PowerOfInverse => product.powf(1.0 - product),
        }
    }
}

/// One step of a stage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    /// The step's identity, which stays the same as its settings change.
    pub id: Uuid,
    /// Whether the step is applied. A step that is off stays in the
    /// document.
    pub enabled: bool,
    /// What the step does.
    #[serde(flatten)]
    pub kind: StepKind,
    /// Where the step takes effect. `None` is everywhere.
    pub mask: Option<MaskRecord>,
    /// The band the step applies to. `None` is every band.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band: Option<String>,
    /// How strongly the step applies, from 0 to 1.
    #[serde(default = "full", skip_serializing_if = "is_full")]
    pub opacity: f32,
}

fn full() -> f32 {
    1.0
}

fn is_full(opacity: &f32) -> bool {
    *opacity == 1.0
}

impl StepRecord {
    /// A new step that is on and applies everywhere.
    pub fn new(kind: StepKind) -> Self {
        Self {
            id: Uuid::new_v4(),
            enabled: true,
            kind,
            mask: None,
            band: None,
            opacity: 1.0,
        }
    }

    /// The step, applied to one band only.
    pub fn for_band(mut self, band: &str) -> Self {
        self.band = Some(band.to_string());
        self
    }
}

/// How a layer combines with the layers beneath it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlendMode {
    /// The layer covers what is beneath it, by its opacity.
    #[default]
    Normal,
    /// Brightens, without clipping: how stars go back onto a starless image.
    Screen,
    /// Adds the layer's samples.
    Add,
    /// Multiplies by the layer's samples.
    Multiply,
    /// Keeps the brighter of the two at each sample.
    Lighten,
    /// Keeps the darker of the two at each sample.
    Darken,
    /// Takes the layer's brightness and keeps the color beneath it: how a
    /// luminance stack goes onto a color image.
    Luminance,
}

impl BlendMode {
    /// Every blend mode, in the order a list shows them.
    pub const ALL: [BlendMode; 7] = [
        BlendMode::Normal,
        BlendMode::Screen,
        BlendMode::Add,
        BlendMode::Multiply,
        BlendMode::Lighten,
        BlendMode::Darken,
        BlendMode::Luminance,
    ];

    fn word(self) -> &'static str {
        match self {
            BlendMode::Normal => "normal",
            BlendMode::Screen => "screen",
            BlendMode::Add => "add",
            BlendMode::Multiply => "multiply",
            BlendMode::Lighten => "lighten",
            BlendMode::Darken => "darken",
            BlendMode::Luminance => "luminance",
        }
    }
}

impl fmt::Display for BlendMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

impl FromStr for BlendMode {
    type Err = StudioError;

    fn from_str(word: &str) -> Result<Self, Self::Err> {
        BlendMode::ALL
            .into_iter()
            .find(|mode| mode.word() == word)
            .ok_or_else(|| StudioError::Document(format!("{word:?} is not a blend mode")))
    }
}

/// Where a layer's pixels come from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "kebab-case")]
pub enum LayerSource {
    /// A band's stretched result.
    Band {
        /// The band's name, such as "Ha" or "OIII".
        band: String,
    },
    /// The result of an earlier step: the starless image before noise
    /// reduction, the stars from star separation, a band before sharpening.
    Step {
        /// The step whose result the layer shows.
        step: Uuid,
        /// Whether the layer keeps the pixels as they were when it was made.
        /// A layer that is not frozen follows later changes to that step.
        frozen: bool,
    },
    /// No pixels of its own: an adjustment of the layers beneath it, such
    /// as exposure or curves. The layer's mask and opacity limit where and
    /// how strongly it applies.
    Adjustment {
        /// The adjustment's name, as a person reads it.
        operation: String,
        /// The adjustment's settings.
        settings: serde_json::Value,
    },
    /// No pixels of its own: the layers beneath it that name it as their
    /// group, blended together on their own first. The group's opacity,
    /// blend, mask and own steps then apply to that result as they would to
    /// one layer.
    Group,
}

impl LayerSource {
    /// Whether a layer with this source shows pixels of its own: a band or
    /// a step's result, and not an adjustment or a group.
    pub fn shows_pixels(&self) -> bool {
        matches!(self, LayerSource::Band { .. } | LayerSource::Step { .. })
    }
}

/// One layer of the Combine stage: a source with its own color, mask and
/// adjustments, combined with the layers beneath it.
///
/// A layer's steps and mask apply to that layer alone. This is where very
/// specific editing happens: one band's highlights, the stars only, one
/// region of one layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerRecord {
    /// The layer's identity.
    pub id: Uuid,
    /// The layer's name, as a person reads it.
    pub name: String,
    /// Where the layer's pixels come from.
    pub source: LayerSource,
    /// Whether the layer is shown.
    pub visible: bool,
    /// How strongly the layer applies, from 0 to 1.
    pub opacity: f32,
    /// How the layer combines with the layers beneath it.
    pub blend: BlendMode,
    /// The color a mono layer is given. `None` keeps the source's own color.
    pub color: Option<[f32; 3]>,
    /// Adjustments to this layer alone, in order.
    pub steps: Vec<StepRecord>,
    /// Where this layer shows. `None` is everywhere.
    pub mask: Option<MaskRecord>,
    /// The group the layer is a member of. A group's members are directly
    /// beneath it in the stack. A group is a member of no group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<Uuid>,
    /// Whether the layer is clipped to the layer beneath it: it shows only
    /// inside that layer's mask, and not at all while that layer is hidden.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clipped: bool,
    /// The palette slot the layer fills, when a palette made it. A palette
    /// replaces only such layers, so one a person added beside them stays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    /// For a group: the palette its layers were made from, if any, and the
    /// input each of its slots shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<PaletteUse>,
}

impl LayerRecord {
    /// A new visible layer at full opacity with the normal blend.
    pub fn new(name: &str, source: LayerSource) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.to_string(),
            source,
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            color: None,
            steps: Vec::new(),
            mask: None,
            group: None,
            clipped: false,
            slot: None,
            palette: None,
        }
    }
}

/// One stage: its steps in order, and the junction at its end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    /// Which stage this is.
    pub kind: StageKind,
    /// The stage's steps, in the order they are applied.
    pub steps: Vec<StepRecord>,
    /// The Combine stage's layers, bottom first. Empty in every other stage.
    #[serde(default)]
    pub layers: Vec<LayerRecord>,
    /// The end of the stage, where its result may be replaced by an edited
    /// file.
    #[serde(default)]
    pub junction: Junction,
}

/// A processing attempt: six stages in a fixed order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// The format version the document was written with.
    pub version: u32,
    /// The document's name.
    pub name: String,
    stages: Vec<Stage>,
    /// The masks layers share, in the order they were made.
    #[serde(default)]
    masks: Vec<SharedMask>,
    /// The palette the layers were made from, if any.
    #[serde(default)]
    pub palette: Option<PaletteUse>,
}

impl Document {
    /// An empty document with every stage.
    pub fn new(name: &str) -> Self {
        Self {
            version: DOCUMENT_VERSION,
            name: name.to_string(),
            stages: StageKind::ALL
                .into_iter()
                .map(|kind| Stage {
                    kind,
                    steps: Vec::new(),
                    layers: Vec::new(),
                    junction: Junction::default(),
                })
                .collect(),
            masks: Vec::new(),
            palette: None,
        }
    }

    /// The masks layers share, in the order they were made.
    pub fn masks(&self) -> &[SharedMask] {
        &self.masks
    }

    /// One shared mask.
    pub fn mask(&self, id: Uuid) -> Option<&SharedMask> {
        self.masks.iter().find(|mask| mask.id == id)
    }

    /// One shared mask, to change it.
    pub fn mask_mut(&mut self, id: Uuid) -> Option<&mut SharedMask> {
        self.masks.iter_mut().find(|mask| mask.id == id)
    }

    /// Adds a shared mask and returns its id.
    pub fn add_mask(&mut self, mask: SharedMask) -> Uuid {
        let id = mask.id;
        self.masks.push(mask);
        id
    }

    /// The layers that use a shared mask, or its inverse.
    pub fn layers_using_mask(&self, id: Uuid) -> impl Iterator<Item = &LayerRecord> {
        let uses = move |record: &Option<MaskRecord>| matches!(record, Some(MaskRecord::Shared { mask, .. }) if *mask == id);
        // A layer uses the mask itself, or through one of its own steps.
        self.layers().iter().filter(move |layer| {
            uses(&layer.mask) || layer.steps.iter().any(|step| uses(&step.mask))
        })
    }

    /// Whether anything uses a shared mask: a layer, a layer's own step, or
    /// a step of a stage.
    pub fn mask_in_use(&self, id: Uuid) -> bool {
        let uses = |record: &Option<MaskRecord>| matches!(record, Some(MaskRecord::Shared { mask, .. }) if *mask == id);
        self.layers_using_mask(id).next().is_some()
            || self
                .stages
                .iter()
                .flat_map(|stage| &stage.steps)
                .any(|step| uses(&step.mask))
    }

    /// Takes a shared mask out of the document. The layers that used it
    /// then show everywhere.
    pub fn remove_mask(&mut self, id: Uuid) -> Option<SharedMask> {
        let at = self.masks.iter().position(|mask| mask.id == id)?;
        let clear = |record: &mut Option<MaskRecord>| {
            if matches!(record, Some(MaskRecord::Shared { mask, .. }) if *mask == id) {
                *record = None;
            }
        };
        for stage in &mut self.stages {
            for step in &mut stage.steps {
                clear(&mut step.mask);
            }
            for layer in &mut stage.layers {
                clear(&mut layer.mask);
                for step in &mut layer.steps {
                    clear(&mut step.mask);
                }
            }
        }
        Some(self.masks.remove(at))
    }

    /// Adds a stroke to a shared mask's touch-ups. Returns whether the
    /// document has the mask.
    pub fn touch_up_mask(&mut self, id: Uuid, stroke: Stroke) -> bool {
        self.mask_mut(id)
            .map(|mask| mask.touch_ups.push(stroke))
            .is_some()
    }

    /// The stages, in order.
    pub fn stages(&self) -> &[Stage] {
        &self.stages
    }

    /// One stage.
    pub fn stage(&self, kind: StageKind) -> &Stage {
        &self.stages[kind as usize]
    }

    /// One stage's junction, to record an export or an edit.
    pub fn junction_mut(&mut self, kind: StageKind) -> &mut Junction {
        &mut self.stages[kind as usize].junction
    }

    /// Puts a step into a stage at `index`, or at the end when `index` is
    /// past it.
    ///
    /// Refuses a step that does not work on the stage's samples, a manual
    /// edit in a stage that does not take them, and a mask on a step in such
    /// a stage.
    pub fn insert_step(
        &mut self,
        stage: StageKind,
        index: usize,
        step: StepRecord,
    ) -> Result<(), StudioError> {
        step.kind
            .accepts()
            .check(step.kind.name(), stage.works_on())?;
        let manual =
            step.kind.is_manual_edit() || matches!(step.mask, Some(MaskRecord::Painted(_)));
        if manual && !stage.allows_manual_edits() {
            return Err(StudioError::Document(format!(
                "manual edits are not allowed in the {stage} stage"
            )));
        }
        let steps = &mut self.stages[stage as usize].steps;
        steps.insert(index.min(steps.len()), step);
        Ok(())
    }

    /// The Combine stage's layers, bottom first.
    pub fn layers(&self) -> &[LayerRecord] {
        &self.stages[StageKind::Combine as usize].layers
    }

    /// One layer, to change its settings. Use [`Self::insert_layer_step`]
    /// to add a step to it.
    pub fn layer_mut(&mut self, id: Uuid) -> Option<&mut LayerRecord> {
        self.stages[StageKind::Combine as usize]
            .layers
            .iter_mut()
            .find(|layer| layer.id == id)
    }

    /// Puts a layer into the Combine stage at `index`, counted from the
    /// bottom, or on top when `index` is past the last layer.
    ///
    /// Refuses an opacity outside 0 to 1, and a layer step that does not
    /// work on display samples.
    pub fn insert_layer(&mut self, index: usize, layer: LayerRecord) -> Result<(), StudioError> {
        if !(0.0..=1.0).contains(&layer.opacity) {
            return Err(StudioError::Document(format!(
                "a layer's opacity is from 0 to 1, not {}",
                layer.opacity
            )));
        }
        for step in &layer.steps {
            step.kind
                .accepts()
                .check(step.kind.name(), StageKind::Combine.works_on())?;
        }
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        layers.insert(index.min(layers.len()), layer);
        self.keep_groups_together();
        Ok(())
    }

    /// Restores the rules of groups after the stack changed: a group's
    /// members are directly beneath it, in their order; a layer whose group
    /// is gone is a member of none; a group is a member of none.
    fn keep_groups_together(&mut self) {
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        let groups: Vec<Uuid> = layers
            .iter()
            .filter(|layer| layer.source == LayerSource::Group)
            .map(|layer| layer.id)
            .collect();
        for layer in layers.iter_mut() {
            let is_group = layer.source == LayerSource::Group;
            if is_group || layer.group.is_some_and(|group| !groups.contains(&group)) {
                layer.group = None;
            }
        }
        for group in groups {
            let (members, mut rest): (Vec<_>, Vec<_>) = std::mem::take(layers)
                .into_iter()
                .partition(|layer| layer.group == Some(group));
            let at = rest
                .iter()
                .position(|layer| layer.id == group)
                .expect("the group was just listed");
            rest.splice(at..at, members);
            *layers = rest;
        }
    }

    /// The members of a group, bottom first.
    pub fn group_members(&self, group: Uuid) -> impl Iterator<Item = &LayerRecord> {
        self.layers()
            .iter()
            .filter(move |layer| layer.group == Some(group))
    }

    /// Puts a layer into a new group of its own, right where the layer is,
    /// and returns the group's id. Other layers join by being moved into it.
    /// The group takes the layer's blend mode.
    ///
    /// Refuses a layer that is a group, and a layer already in a group:
    /// groups do not hold groups.
    pub fn group_layer(&mut self, id: Uuid) -> Result<Uuid, StudioError> {
        let refuse = |why: &str| StudioError::Document(why.to_string());
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        let at = layers
            .iter()
            .position(|layer| layer.id == id)
            .ok_or_else(|| refuse("the layer is not in the document"))?;
        if layers[at].source == LayerSource::Group || layers[at].group.is_some() {
            return Err(refuse("a group does not hold another group"));
        }
        // The group blends as the layer did, so grouping one layer does not
        // change the picture.
        let mut group = LayerRecord::new("Group", LayerSource::Group);
        group.blend = layers[at].blend;
        let group_id = group.id;
        layers[at].group = Some(group_id);
        // The layer leaves the part whose palette made it.
        layers[at].slot = None;
        layers.insert(at + 1, group);
        Ok(group_id)
    }

    /// Dissolves a group: its members stay where they are, as members of no
    /// group. The group's own mask and steps go with it. Returns whether
    /// the document had the group.
    pub fn ungroup(&mut self, group: Uuid) -> bool {
        let is_group = self
            .layers()
            .iter()
            .any(|layer| layer.id == group && layer.source == LayerSource::Group);
        if !is_group {
            return false;
        }
        // The members are no palette's from here on, and the masks the
        // group's palette made are theirs alone.
        for layer in &mut self.stages[StageKind::Combine as usize].layers {
            if layer.group == Some(group) {
                layer.slot = None;
            }
        }
        for mask in &mut self.masks {
            if mask.part == Some(group) {
                mask.part = None;
                mask.slots.clear();
            }
        }
        self.remove_layer(group).is_some()
    }

    /// Moves a layer so that it lands under the layer that is at `position`
    /// now, counted from the bottom, as a member of `group` or of none. A
    /// group moves with its members and joins no group.
    ///
    /// Returns whether the document has the layer.
    pub fn place_layer(&mut self, id: Uuid, position: usize, group: Option<Uuid>) -> bool {
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        let Some(at) = layers.iter().position(|layer| layer.id == id) else {
            return false;
        };
        let is_group = layers[at].source == LayerSource::Group;
        // A group's members are directly beneath it.
        let low = if is_group {
            layers[..at]
                .iter()
                .rposition(|layer| layer.group != Some(id))
                .map_or(0, |other| other + 1)
        } else {
            at
        };
        let joins = group.filter(|group| {
            !is_group
                && layers
                    .iter()
                    .any(|layer| layer.id == *group && layer.source == LayerSource::Group)
        });
        if !is_group {
            // A layer that leaves its part is no longer one its palette
            // made: the palette of the part it joins must not replace it.
            if layers[at].group != joins {
                layers[at].slot = None;
            }
            layers[at].group = joins;
        }
        // A place inside the block that moves is where the block is.
        if !(low..=at + 1).contains(&position) {
            let block: Vec<LayerRecord> = layers.drain(low..=at).collect();
            let to = if position > at {
                position - block.len()
            } else {
                position
            };
            let to = to.min(layers.len());
            layers.splice(to..to, block);
        }
        self.keep_groups_together();
        true
    }

    pub fn insert_layer_step(&mut self, layer: Uuid, step: StepRecord) -> Result<(), StudioError> {
        step.kind
            .accepts()
            .check(step.kind.name(), StageKind::Combine.works_on())?;
        let layer = self
            .layer_mut(layer)
            .ok_or_else(|| StudioError::Document("the layer is not in the document".to_string()))?;
        layer.steps.push(step);
        Ok(())
    }

    /// Moves one layer to `index`, counted from the bottom. Use
    /// [`Self::place_layer`] to move a layer into or out of a group, or to
    /// move a group with its members.
    pub fn move_layer(&mut self, id: Uuid, index: usize) -> bool {
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        let Some(from) = layers.iter().position(|layer| layer.id == id) else {
            return false;
        };
        let layer = layers.remove(from);
        layers.insert(index.min(layers.len()), layer);
        self.keep_groups_together();
        true
    }

    /// Takes a layer out of the document and returns it.
    pub fn remove_layer(&mut self, id: Uuid) -> Option<LayerRecord> {
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        let index = layers.iter().position(|layer| layer.id == id)?;
        let removed = layers.remove(index);
        self.keep_groups_together();
        Some(removed)
    }

    /// A step of any stage, and the stage it is in. A step that belongs to
    /// one layer is in the Combine stage.
    pub fn step(&self, id: Uuid) -> Option<(StageKind, &StepRecord)> {
        self.stages.iter().find_map(|stage| {
            let step = stage
                .steps
                .iter()
                .chain(stage.layers.iter().flat_map(|layer| &layer.steps))
                .find(|step| step.id == id)?;
            Some((stage.kind, step))
        })
    }

    fn step_mut(&mut self, id: Uuid) -> Option<(StageKind, &mut StepRecord)> {
        self.stages.iter_mut().find_map(|stage| {
            let kind = stage.kind;
            let step = stage
                .steps
                .iter_mut()
                .chain(stage.layers.iter_mut().flat_map(|layer| &mut layer.steps))
                .find(|step| step.id == id)?;
            Some((kind, step))
        })
    }

    /// The layer a step belongs to, when it is one layer's own step.
    pub fn layer_of_step(&self, id: Uuid) -> Option<Uuid> {
        self.layers()
            .iter()
            .find(|layer| layer.steps.iter().any(|step| step.id == id))
            .map(|layer| layer.id)
    }

    /// Makes an adjustment layer one layer's own: the adjustment then
    /// changes that layer alone, and moves with it. Returns the id of the
    /// layer's new step.
    ///
    /// The adjustment keeps its settings, its mask and its opacity.
    ///
    /// Refuses a layer that is not an adjustment, and a target that is one.
    pub fn attach_adjustment(&mut self, adjustment: Uuid, to: Uuid) -> Result<Uuid, StudioError> {
        let refuse = |why: &str| StudioError::Document(why.to_string());
        let target = self
            .layers()
            .iter()
            .find(|layer| layer.id == to)
            .ok_or_else(|| refuse("the layer is not in the document"))?;
        if matches!(target.source, LayerSource::Adjustment { .. }) {
            return Err(refuse("an adjustment has no pixels of its own to adjust"));
        }
        let is_adjustment = self.layers().iter().any(|layer| {
            layer.id == adjustment && matches!(layer.source, LayerSource::Adjustment { .. })
        });
        if !is_adjustment {
            return Err(refuse("only an adjustment layer can be given to one layer"));
        }
        let layer = self.remove_layer(adjustment).expect("the layer was found");
        let LayerSource::Adjustment {
            operation,
            settings,
        } = layer.source
        else {
            unreachable!("checked above");
        };
        let mut step = StepRecord::new(StepKind::Operation {
            operation,
            accepts: Accepts::Display,
            settings,
        });
        step.enabled = layer.visible;
        step.mask = layer.mask;
        step.opacity = layer.opacity;
        let id = step.id;
        self.insert_layer_step(to, step)?;
        Ok(id)
    }

    /// Makes one layer's own adjustment a layer of the stack, placed right
    /// above the layer it belonged to, so it changes every layer beneath
    /// it. Returns the new layer's id.
    ///
    /// Refuses a step that is not an operation, such as a heal.
    pub fn detach_adjustment(&mut self, step: Uuid) -> Result<Uuid, StudioError> {
        let refuse = |why: &str| StudioError::Document(why.to_string());
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        let at = layers
            .iter()
            .position(|layer| layer.steps.iter().any(|own| own.id == step))
            .ok_or_else(|| refuse("the step does not belong to a layer"))?;
        let index = layers[at]
            .steps
            .iter()
            .position(|own| own.id == step)
            .expect("the step was found");
        if !matches!(layers[at].steps[index].kind, StepKind::Operation { .. }) {
            return Err(refuse("only an adjustment can become a layer"));
        }
        let own = layers[at].steps.remove(index);
        let StepKind::Operation {
            operation,
            settings,
            ..
        } = own.kind
        else {
            unreachable!("checked above");
        };
        let mut layer = LayerRecord::new(
            &operation.clone(),
            LayerSource::Adjustment {
                operation,
                settings,
            },
        );
        layer.visible = own.enabled;
        layer.mask = own.mask;
        layer.opacity = own.opacity.clamp(0.0, 1.0);
        // Above its owner, in the owner's group: a group's own step becomes
        // a layer above the group, a member's stays in the member's group.
        layer.group = if layers[at].source == LayerSource::Group {
            None
        } else {
            layers[at].group
        };
        let id = layer.id;
        layers.insert(at + 1, layer);
        self.keep_groups_together();
        Ok(id)
    }

    /// Replaces an operation's settings. Returns whether the document has
    /// such an operation.
    pub fn set_step_settings(&mut self, id: Uuid, new: serde_json::Value) -> bool {
        match self.step_mut(id) {
            Some((
                _,
                StepRecord {
                    kind: StepKind::Operation { settings, .. },
                    ..
                },
            )) => {
                *settings = new;
                true
            }
            _ => false,
        }
    }

    /// Turns a step on or off. Returns whether the document has the step.
    pub fn set_step_enabled(&mut self, id: Uuid, enabled: bool) -> bool {
        self.step_mut(id)
            .map(|(_, step)| step.enabled = enabled)
            .is_some()
    }

    /// Adds a stroke to a step's painted mask, starting the mask if the step
    /// has none. A mask of another kind is replaced by the painted one.
    ///
    /// Refuses a step in a stage that allows no manual edits.
    pub fn paint_step_mask(&mut self, id: Uuid, stroke: Stroke) -> Result<(), StudioError> {
        let (stage, step) = self
            .step_mut(id)
            .ok_or_else(|| StudioError::Document("the step is not in the document".to_string()))?;
        if !stage.allows_manual_edits() {
            return Err(StudioError::Document(format!(
                "manual edits are not allowed in the {stage} stage"
            )));
        }
        match &mut step.mask {
            Some(MaskRecord::Painted(mask)) => mask.strokes.push(stroke),
            mask @ None => {
                *mask = Some(MaskRecord::Painted(PaintedMask {
                    strokes: vec![stroke],
                }));
            }
            Some(_) => {
                return Err(StudioError::Document(
                    "the step has another kind of mask; remove it before painting one".to_string(),
                ))
            }
        }
        Ok(())
    }

    /// Sets how strongly a step applies, from 0 to 1. Returns whether the
    /// document has the step.
    pub fn set_step_opacity(&mut self, id: Uuid, opacity: f32) -> bool {
        self.step_mut(id)
            .map(|(_, step)| step.opacity = opacity.clamp(0.0, 1.0))
            .is_some()
    }

    /// Removes a step's mask, so the step applies everywhere. Returns
    /// whether the document has the step.
    pub fn clear_step_mask(&mut self, id: Uuid) -> bool {
        self.step_mut(id)
            .map(|(_, step)| step.mask = None)
            .is_some()
    }

    /// Adds a stroke to a layer's painted mask. A layer with no mask gets a
    /// painted one. A layer behind a shared or computed mask is refused:
    /// painting over it would discard it. A shared mask is touched up with
    /// [`Self::touch_up_mask`].
    pub fn paint_layer_mask(&mut self, id: Uuid, stroke: Stroke) -> Result<(), StudioError> {
        let layer = self
            .layer_mut(id)
            .ok_or_else(|| StudioError::Document("the layer is not in the document".to_string()))?;
        match &mut layer.mask {
            Some(MaskRecord::Painted(mask)) => mask.strokes.push(stroke),
            Some(_) => {
                return Err(StudioError::Document(
                    "the layer has a computed mask; remove it before painting one".to_string(),
                ));
            }
            mask @ None => {
                *mask = Some(MaskRecord::Painted(PaintedMask {
                    strokes: vec![stroke],
                }));
            }
        }
        Ok(())
    }

    /// Takes a step out of the document and returns it.
    pub fn remove_step(&mut self, id: Uuid) -> Option<StepRecord> {
        self.stages.iter_mut().find_map(|stage| {
            if let Some(index) = stage.steps.iter().position(|step| step.id == id) {
                return Some(stage.steps.remove(index));
            }
            stage.layers.iter_mut().find_map(|layer| {
                let index = layer.steps.iter().position(|step| step.id == id)?;
                Some(layer.steps.remove(index))
            })
        })
    }

    /// The document as JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a document is always valid JSON")
    }

    /// Reads a document from JSON. Refuses a document from a newer build,
    /// and one whose stages are missing or out of order.
    pub fn from_json(text: &str) -> Result<Self, StudioError> {
        let document: Document =
            serde_json::from_str(text).map_err(|error| StudioError::Document(error.to_string()))?;
        if document.version > DOCUMENT_VERSION {
            return Err(StudioError::Document(format!(
                "it was written by a newer version (format {}; this build reads up to {})",
                document.version, DOCUMENT_VERSION
            )));
        }
        let kinds: Vec<StageKind> = document.stages.iter().map(|stage| stage.kind).collect();
        if kinds != StageKind::ALL {
            return Err(StudioError::Document(
                "its stages are missing or out of order".to_string(),
            ));
        }
        Ok(document)
    }
}
