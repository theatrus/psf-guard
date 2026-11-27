use super::{Junction, PaintedMask, Stroke, StrokeMode, StudioError};
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
    /// No pixels of its own: a change to the layers beneath it, in its group
    /// or in the whole stack. Its step holds what it does and its settings:
    /// exposure, curves or saturation, an output stretch, Tone and color, a
    /// heal, or a processing tool that runs on the picture beneath it. The
    /// layer's visibility, opacity and mask say where and how strongly it
    /// applies, so a step's own are not used. What the Finish stage held is
    /// such layers at the top of the stack.
    Adjust { step: StepRecord },
    /// How documents written before [`Self::Adjust`] kept an adjustment.
    /// Opening one makes it an [`Self::Adjust`] layer.
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

    /// Whether the layer adjusts what is beneath it.
    pub fn is_adjustment(&self) -> bool {
        matches!(
            self,
            LayerSource::Adjust { .. } | LayerSource::Adjustment { .. }
        )
    }

    /// An adjustment layer's step.
    pub fn adjustment_step(&self) -> Option<&StepRecord> {
        match self {
            LayerSource::Adjust { step } => Some(step),
            _ => None,
        }
    }

    fn adjustment_step_mut(&mut self) -> Option<&mut StepRecord> {
        match self {
            LayerSource::Adjust { step } => Some(step),
            _ => None,
        }
    }
}

/// An id made from `from`, the same each time a document is opened: for a
/// layer made from a step, or a step made from a layer, as documents are
/// brought up to date.
fn derived_id(from: Uuid, salt: u8) -> Uuid {
    let mut bytes = *from.as_bytes();
    bytes[13] ^= salt;
    bytes[14] ^= 0x5a;
    Uuid::from_bytes(bytes)
}

/// How deep groups nest: a layer inside this many groups at most, as deep
/// as Seiza's compositor draws them.
pub const MAX_GROUP_DEPTH: usize = 4;

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
    /// A painted mask that limits a layer behind a shared mask as well:
    /// where the shared mask follows the inputs, this is painted by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub painted_too: Option<super::PaintedMask>,
    /// The group the layer is a member of. A group's members, groups among
    /// them, are directly beneath it in the stack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<Uuid>,
    /// For a group: whether its layers pass through, each blending straight
    /// onto the layers beneath rather than together first. The group's
    /// opacity and masks still limit each of them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pass_through: bool,
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
    /// How much the stars a layer shows are made smaller, from 0 to 1: each
    /// star blended toward a one-pixel erosion of itself. On a group, the
    /// stars of its layers are reduced together, so their colors keep. A
    /// layer's own amount, when above 0, is used in place of its group's.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub star_reduction: f32,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
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
            painted_too: None,
            group: None,
            pass_through: false,
            clipped: false,
            slot: None,
            palette: None,
            star_reduction: 0.0,
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
    /// The recipe the layers were made from last, by its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    /// What part of the image the picture keeps.
    #[serde(default, skip_serializing_if = "CropRecord::is_automatic")]
    pub crop: CropRecord,
    /// How far above SDR white the picture reaches, in stops, when it is
    /// HDR: every stage of it, from the stretches on, may be brighter than
    /// white up to there. `None` is an SDR picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headroom: Option<f32>,
    /// In an HDR picture, where each stretch's continuation into the
    /// headroom starts, as the stretch's output, from 0.3 to 0.95. A lower
    /// knee gives the bright core more of the headroom, and more contrast
    /// there. `None` is three quarters. An HDR layered stretch keeps its
    /// own knee.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr_knee: Option<f32>,
}

/// What part of the image a picture keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CropRecord {
    /// The largest rectangle every input covers, which the store records
    /// when it is prepared. The empty edges registration leaves are cut.
    #[default]
    Automatic,
    /// The whole image, empty edges included.
    Whole,
    /// A rectangle a person drew, in document pixels.
    Region {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
}

impl CropRecord {
    pub fn is_automatic(&self) -> bool {
        *self == CropRecord::Automatic
    }

    /// The rectangle a person drew from one corner to another, inside an
    /// image of `size`. `None` when it is less than 8 pixels across, too
    /// small to be meant.
    pub fn from_corners(a: [f64; 2], b: [f64; 2], size: (u32, u32)) -> Option<CropRecord> {
        let clamp = |value: f64, limit: u32| value.round().clamp(0.0, f64::from(limit)) as u32;
        let (x0, x1) = (clamp(a[0].min(b[0]), size.0), clamp(a[0].max(b[0]), size.0));
        let (y0, y1) = (clamp(a[1].min(b[1]), size.1), clamp(a[1].max(b[1]), size.1));
        (x1 - x0 >= 8 && y1 - y0 >= 8).then_some(CropRecord::Region {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        })
    }
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
            recipe: None,
            crop: CropRecord::Automatic,
            headroom: None,
            hdr_knee: None,
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

    /// Moves a step to `index` in its own stage, counted among the stage's
    /// other steps; past the end puts it last. Returns whether the step was
    /// found in a stage. A layer's own steps are not moved.
    pub fn move_step(&mut self, id: Uuid, index: usize) -> bool {
        for stage in &mut self.stages {
            if let Some(at) = stage.steps.iter().position(|step| step.id == id) {
                let step = stage.steps.remove(at);
                let index = index.min(stage.steps.len());
                stage.steps.insert(index, step);
                return true;
            }
        }
        false
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
    /// members, groups among them, are directly beneath it, in their order;
    /// a layer whose group is gone, or that would hold itself, is a member
    /// of none.
    fn keep_groups_together(&mut self) {
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        let groups: std::collections::HashSet<Uuid> = layers
            .iter()
            .filter(|layer| layer.source == LayerSource::Group)
            .map(|layer| layer.id)
            .collect();
        for layer in layers.iter_mut() {
            if layer.group.is_some_and(|group| !groups.contains(&group)) {
                layer.group = None;
            }
        }
        // A group inside itself, however deep, leaves its group.
        let parent: std::collections::HashMap<Uuid, Option<Uuid>> =
            layers.iter().map(|layer| (layer.id, layer.group)).collect();
        for layer in layers.iter_mut() {
            let mut at = layer.group;
            for _ in 0..parent.len() {
                match at {
                    Some(group) if group == layer.id => {
                        layer.group = None;
                        break;
                    }
                    Some(group) => at = parent.get(&group).copied().flatten(),
                    None => break,
                }
            }
        }
        // Each group's members, then the group, from the top level down,
        // siblings in the order they were in.
        let mut children: std::collections::HashMap<Option<Uuid>, Vec<usize>> =
            std::collections::HashMap::new();
        for (at, layer) in layers.iter().enumerate() {
            children.entry(layer.group).or_default().push(at);
        }
        fn emit(
            at: usize,
            layers: &[LayerRecord],
            children: &std::collections::HashMap<Option<Uuid>, Vec<usize>>,
            order: &mut Vec<usize>,
        ) {
            if layers[at].source == LayerSource::Group {
                for child in children.get(&Some(layers[at].id)).into_iter().flatten() {
                    emit(*child, layers, children, order);
                }
            }
            order.push(at);
        }
        let mut order = Vec::with_capacity(layers.len());
        for at in children.get(&None).into_iter().flatten() {
            emit(*at, layers, &children, &mut order);
        }
        let mut taken: Vec<Option<LayerRecord>> =
            std::mem::take(layers).into_iter().map(Some).collect();
        *layers = order
            .into_iter()
            .filter_map(|at| taken[at].take())
            .collect();
    }

    /// The groups a layer is in, outermost first.
    pub fn ancestors(&self, id: Uuid) -> Vec<Uuid> {
        let layers = self.layers();
        let mut chain = Vec::new();
        let mut at = layers
            .iter()
            .find(|layer| layer.id == id)
            .and_then(|layer| layer.group);
        while let Some(group) = at {
            if chain.contains(&group) || chain.len() > layers.len() {
                break;
            }
            chain.push(group);
            at = layers
                .iter()
                .find(|layer| layer.id == group)
                .and_then(|layer| layer.group);
        }
        chain.reverse();
        chain
    }

    /// How deep the deepest member of the stack sits: 0 when no layer is in
    /// a group.
    pub fn group_depth(&self) -> usize {
        self.layers()
            .iter()
            .map(|layer| self.ancestors(layer.id).len())
            .max()
            .unwrap_or(0)
    }

    /// The places a layer takes in the stack with what moves with it,
    /// bottom first: a group with every layer inside it, however deep, and
    /// any other layer alone. `None` when the document has no such layer.
    pub fn block_of(&self, id: Uuid) -> Option<std::ops::RangeInclusive<usize>> {
        let layers = self.layers();
        let at = layers.iter().position(|layer| layer.id == id)?;
        if layers[at].source != LayerSource::Group {
            return Some(at..=at);
        }
        let low = layers[..at]
            .iter()
            .rposition(|layer| !self.ancestors(layer.id).contains(&id))
            .map_or(0, |other| other + 1);
        Some(low..=at)
    }

    /// The members of a group, bottom first.
    pub fn group_members(&self, group: Uuid) -> impl Iterator<Item = &LayerRecord> {
        self.layers()
            .iter()
            .filter(move |layer| layer.group == Some(group))
    }

    /// Puts a layer, or a group with its members, into a new group of its
    /// own, right where it is, in the group it was in, and returns the new
    /// group's id. Other layers join by being moved into it. The group takes
    /// the layer's blend mode.
    ///
    /// Refuses a layer that would sit deeper than [`MAX_GROUP_DEPTH`].
    pub fn group_layer(&mut self, id: Uuid) -> Result<Uuid, StudioError> {
        let refuse = |why: &str| StudioError::Document(why.to_string());
        let before = self.stages[StageKind::Combine as usize].layers.clone();
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        let at = layers
            .iter()
            .position(|layer| layer.id == id)
            .ok_or_else(|| refuse("the layer is not in the document"))?;
        // The group blends as the layer did, so grouping one layer does not
        // change the picture.
        let mut group = LayerRecord::new("Group", LayerSource::Group);
        group.blend = layers[at].blend;
        group.group = layers[at].group;
        let group_id = group.id;
        layers[at].group = Some(group_id);
        // The layer leaves the part whose palette made it.
        layers[at].slot = None;
        layers.insert(at + 1, group);
        if self.group_depth() > MAX_GROUP_DEPTH {
            self.stages[StageKind::Combine as usize].layers = before;
            return Err(refuse(&format!("groups go {MAX_GROUP_DEPTH} deep at most")));
        }
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
        // group's palette made are theirs alone. They join the group it was
        // in.
        let outer = self
            .layers()
            .iter()
            .find(|layer| layer.id == group)
            .and_then(|layer| layer.group);
        for layer in &mut self.stages[StageKind::Combine as usize].layers {
            if layer.group == Some(group) {
                layer.slot = None;
                layer.group = outer;
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
    /// Lists the layers a palette made in the palette's order from the top,
    /// as palettes now stack them. A document saved before stacks them in
    /// the palette's order from the bottom, so R, G and B read B, G, R in
    /// the layer list. Only such a run is turned over: a part's palette
    /// layers still in that order, next to each other, all added and none
    /// clipped, with no clipped layer above them. Adding is the same in any
    /// order, so the picture does not change. Returns whether a run moved.
    pub fn list_palette_layers_top_down(&mut self) -> bool {
        let parts: Vec<(Option<Uuid>, String)> = std::iter::once(None)
            .chain(
                self.layers()
                    .iter()
                    .filter(|layer| layer.source == LayerSource::Group)
                    .map(|layer| Some(layer.id)),
            )
            .filter_map(|part| Some((part, self.palette_in(part)?.name.clone())))
            .collect();
        let mut moved = false;
        for (part, name) in parts {
            let Some(palette) = super::palette::PALETTES
                .iter()
                .find(|palette| palette.name == name)
            else {
                continue;
            };
            let layers = &mut self.stages[StageKind::Combine as usize].layers;
            let members: Vec<usize> = layers
                .iter()
                .enumerate()
                .filter(|(_, layer)| layer.group == part && layer.slot.is_some())
                .map(|(at, _)| at)
                .collect();
            let (Some(&first), Some(&last)) = (members.first(), members.last()) else {
                continue;
            };
            let slots: Vec<&str> = members
                .iter()
                .filter_map(|at| layers[*at].slot.as_deref())
                .collect();
            let old_order: Vec<&str> = palette.layers.iter().map(|layer| layer.channel).collect();
            let turnable = members.len() > 1
                && last - first + 1 == members.len()
                && slots == old_order
                && members
                    .iter()
                    .all(|at| layers[*at].blend == BlendMode::Add && !layers[*at].clipped)
                && layers
                    .get(last + 1)
                    .is_none_or(|above| !above.clipped || above.group != part);
            if turnable {
                layers[first..=last].reverse();
                moved = true;
            }
        }
        moved
    }

    pub fn place_layer(&mut self, id: Uuid, position: usize, group: Option<Uuid>) -> bool {
        let Some(block) = self.block_of(id) else {
            return false;
        };
        // A group joins a group only outside itself, and no deeper than
        // groups go.
        let joins = group.filter(|group| {
            *group != id
                && !self.ancestors(*group).contains(&id)
                && self
                    .layers()
                    .iter()
                    .any(|layer| layer.id == *group && layer.source == LayerSource::Group)
        });
        let before = self.stages[StageKind::Combine as usize].layers.clone();
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        let (low, at) = (*block.start(), *block.end());
        {
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
        if self.group_depth() > MAX_GROUP_DEPTH {
            self.stages[StageKind::Combine as usize].layers = before;
            return false;
        }
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

    /// Moves a layer one place toward the top (`up`) or the bottom, past
    /// its neighbour: a group's member past the next member of its group,
    /// and a layer or a group at the top level past the next layer or
    /// whole group there. A group moves with its members. Returns whether it
    /// moved: an end of its group or of the stack stops it.
    pub fn step_layer(&mut self, id: Uuid, up: bool) -> bool {
        let Some(own) = self.block_of(id) else {
            return false;
        };
        let layers = &self.stages[StageKind::Combine as usize].layers;
        let at = *own.end();
        let container = layers[at].group;
        // The block a layer moves as, bottom first: a group with every layer
        // inside it, any other layer alone.
        let block = |head: usize| self.block_of(layers[head].id).unwrap_or(head..=head);
        let neighbour = if up {
            // Above, the next sibling: the layer there, or the group whose
            // bottom member it is, however deep.
            let next = own.end() + 1;
            layers.get(next).and_then(|first| {
                let mut chain = self.ancestors(first.id);
                chain.push(first.id);
                let depth = container.map_or(0, |group| self.ancestors(group).len() + 1);
                // The member must be inside this layer's group.
                if container.is_some_and(|group| !chain.contains(&group)) {
                    return None;
                }
                let sibling = *chain.get(depth)?;
                let head = layers.iter().position(|layer| layer.id == sibling)?;
                Some(block(head))
            })
        } else {
            own.start()
                .checked_sub(1)
                .filter(|before| layers[*before].group == container)
                .map(block)
        };
        let Some(neighbour) = neighbour else {
            return false;
        };
        let (low, high) = if up {
            (*own.start(), *neighbour.end())
        } else {
            (*neighbour.start(), *own.end())
        };
        let moved = if up {
            own.end() - own.start() + 1
        } else {
            neighbour.end() - neighbour.start() + 1
        };
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        layers[low..=high].rotate_left(moved);
        self.keep_groups_together();
        true
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
                .chain(
                    stage
                        .layers
                        .iter()
                        .filter_map(|layer| layer.source.adjustment_step()),
                )
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
                .chain(stage.layers.iter_mut().flat_map(|layer| {
                    let LayerRecord { steps, source, .. } = layer;
                    steps.iter_mut().chain(source.adjustment_step_mut())
                }))
                .find(|step| step.id == id)?;
            Some((kind, step))
        })
    }

    /// The adjustment layer whose step `step` is.
    pub fn layer_of_adjustment(&self, step: Uuid) -> Option<Uuid> {
        self.layers()
            .iter()
            .find(|layer| {
                layer
                    .source
                    .adjustment_step()
                    .is_some_and(|own| own.id == step)
            })
            .map(|layer| layer.id)
    }

    /// Adds `step` on top of every layer, as an adjustment layer of its
    /// own whose mask is the step's. A star removal also gets a layer of
    /// the stars it took out, above it. Returns the adjustment layer's id.
    pub fn add_picture_step(&mut self, mut step: StepRecord) -> Uuid {
        let stars = (step.kind.name() == super::flow::flow_operations::STAR_REMOVAL)
            .then(|| super::flow::stars_layer(step.id));
        let mask = step.mask.take();
        let mut layer = super::flow::adjustment_layer(step);
        layer.mask = mask;
        let id = layer.id;
        let layers = &mut self.stages[StageKind::Combine as usize].layers;
        layers.push(layer);
        layers.extend(stars);
        id
    }

    /// The steps that change every layer beneath them, in the order they
    /// apply: each adjustment layer's, bottom layer first, then the Finish
    /// stage's.
    pub fn picture_steps(&self) -> impl Iterator<Item = &StepRecord> {
        self.layers()
            .iter()
            .filter_map(|layer| layer.source.adjustment_step())
            .chain(self.stage(StageKind::Finish).steps.iter())
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
        if target.source.is_adjustment() {
            return Err(refuse("an adjustment has no pixels of its own to adjust"));
        }
        // A layer's own edits are those that change one pixel at a time and
        // processing tools; a step that reads the whole picture beneath it,
        // such as an output stretch, stays a layer of the stack.
        let own_kind = |step: &StepRecord| {
            ["Exposure", "Curves", "Saturation", "Green reduction"].contains(&step.kind.name())
                || super::flow::is_tool_step(step)
        };
        // A layer kept the older way is brought up to date first.
        if let Some(layer) = self.layer_mut(adjustment)
            && let LayerSource::Adjustment {
                operation,
                settings,
            } = &layer.source
        {
            let step = StepRecord::new(StepKind::Operation {
                operation: operation.clone(),
                accepts: Accepts::Display,
                settings: settings.clone(),
            });
            layer.source = LayerSource::Adjust { step };
        }
        let fits = self.layers().iter().any(|layer| {
            layer.id == adjustment && layer.source.adjustment_step().is_some_and(own_kind)
        });
        if !fits {
            return Err(refuse(
                "only an exposure, a curve, a saturation or a tool can be given to one layer",
            ));
        }
        let layer = self.remove_layer(adjustment).expect("the layer was found");
        let LayerSource::Adjust { mut step } = layer.source else {
            unreachable!("checked above");
        };
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
        let mut own = layers[at].steps.remove(index);
        let (visible, mask, opacity) = (own.enabled, own.mask.take(), own.opacity.clamp(0.0, 1.0));
        own.enabled = true;
        own.opacity = 1.0;
        let name = own.kind.name().to_string();
        let mut layer = LayerRecord::new(&name, LayerSource::Adjust { step: own });
        layer.visible = visible;
        layer.mask = mask;
        layer.opacity = opacity;
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
        // An adjustment layer's step is on while its layer is shown.
        if let Some(layer) = self.layer_of_adjustment(id) {
            return self
                .layer_mut(layer)
                .map(|layer| layer.visible = enabled)
                .is_some();
        }
        self.step_mut(id)
            .map(|(_, step)| step.enabled = enabled)
            .is_some()
    }

    /// Removes the newest stroke from a painted mask: of a layer, of a step
    /// (an input's, a layer's own, or a Finish step's), or the newest
    /// touch-up of a shared mask. `id` names the layer, the step or the
    /// shared mask. Returns whether a stroke was removed.
    pub fn remove_last_stroke(&mut self, id: Uuid) -> bool {
        if let Some(mask) = self.masks.iter_mut().find(|mask| mask.id == id) {
            return mask.touch_ups.pop().is_some();
        }
        let pop = |record: &mut Option<MaskRecord>| match record {
            Some(MaskRecord::Painted(mask)) => {
                let removed = mask.strokes.pop().is_some();
                if mask.strokes.is_empty() {
                    *record = None;
                }
                removed
            }
            _ => false,
        };
        for stage in &mut self.stages {
            for step in &mut stage.steps {
                if step.id == id {
                    return pop(&mut step.mask);
                }
            }
            for layer in &mut stage.layers {
                if layer.id == id {
                    // The strokes painted behind a shared mask go first.
                    if let Some(too) = &mut layer.painted_too {
                        let removed = too.strokes.pop().is_some();
                        if too.strokes.is_empty() {
                            layer.painted_too = None;
                        }
                        return removed;
                    }
                    return pop(&mut layer.mask);
                }
                for step in &mut layer.steps {
                    if step.id == id {
                        return pop(&mut step.mask);
                    }
                }
            }
        }
        false
    }

    /// How many strokes the painted mask of the layer or the step `id` has,
    /// or the touch-ups of the shared mask `id`.
    pub fn stroke_count(&self, id: Uuid) -> usize {
        if let Some(mask) = self.masks.iter().find(|mask| mask.id == id) {
            return mask.touch_ups.len();
        }
        let count = |record: &Option<MaskRecord>| match record {
            Some(MaskRecord::Painted(mask)) => mask.strokes.len(),
            _ => 0,
        };
        for stage in &self.stages {
            for step in &stage.steps {
                if step.id == id {
                    return count(&step.mask);
                }
            }
            for layer in &stage.layers {
                if layer.id == id {
                    return count(&layer.mask)
                        + layer
                            .painted_too
                            .as_ref()
                            .map_or(0, |mask| mask.strokes.len());
                }
                for step in &layer.steps {
                    if step.id == id {
                        return count(&step.mask);
                    }
                }
            }
        }
        0
    }

    /// Sets how far a painted mask's strokes are softened: of the layer or
    /// the step `id`. Returns whether it has a painted mask.
    pub fn set_mask_blur(&mut self, id: Uuid, blur: f32) -> bool {
        let set = |record: &mut Option<MaskRecord>| match record {
            Some(MaskRecord::Painted(mask)) => {
                mask.blur = blur.max(0.0);
                true
            }
            _ => false,
        };
        for stage in &mut self.stages {
            for step in &mut stage.steps {
                if step.id == id {
                    return set(&mut step.mask);
                }
            }
            for layer in &mut stage.layers {
                if layer.id == id {
                    if let Some(too) = &mut layer.painted_too {
                        too.blur = blur.max(0.0);
                        return true;
                    }
                    return set(&mut layer.mask);
                }
                for step in &mut layer.steps {
                    if step.id == id {
                        return set(&mut step.mask);
                    }
                }
            }
        }
        false
    }

    /// Adds a stroke to a step's painted mask, starting the mask if the step
    /// has none. A mask of another kind is refused.
    ///
    /// Refuses a step in a stage that allows no manual edits.
    /// An erasing stroke on a step with no mask is refused: the step applies
    /// everywhere, and a new mask would start empty and hide it.
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
                if stroke.mode == StrokeMode::Subtract {
                    return Err(StudioError::Document(
                        "the step has no mask to erase from".to_string(),
                    ));
                }
                *mask = Some(MaskRecord::Painted(PaintedMask {
                    strokes: vec![stroke],
                    blur: 0.0,
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
        // An adjustment layer's step applies at its layer's opacity.
        if let Some(layer) = self.layer_of_adjustment(id) {
            return self
                .layer_mut(layer)
                .map(|layer| layer.opacity = opacity.clamp(0.0, 1.0))
                .is_some();
        }
        self.step_mut(id)
            .map(|(_, step)| step.opacity = opacity.clamp(0.0, 1.0))
            .is_some()
    }

    /// Removes a step's mask, so the step applies everywhere. Returns
    /// whether the document has the step.
    pub fn clear_step_mask(&mut self, id: Uuid) -> bool {
        // An adjustment layer's step applies within its layer's mask.
        if let Some(layer) = self.layer_of_adjustment(id) {
            return self
                .layer_mut(layer)
                .map(|layer| layer.mask = None)
                .is_some();
        }
        self.step_mut(id)
            .map(|(_, step)| step.mask = None)
            .is_some()
    }

    /// Adds a stroke to a layer's painted mask. A layer with no mask gets a
    /// painted one. A layer behind a shared or computed mask is refused:
    /// painting over it would discard it. A shared mask is touched up with
    /// [`Self::touch_up_mask`].
    /// Puts a layer behind a shared mask, or its inverse, or takes it from
    /// behind one with `None`. A painted mask stays: behind a shared mask it
    /// limits the layer as well. Returns whether the document has the layer.
    pub fn set_layer_shared_mask(&mut self, id: Uuid, shared: Option<(Uuid, bool)>) -> bool {
        let Some(layer) = self.layer_mut(id) else {
            return false;
        };
        let painted = match layer.mask.take() {
            Some(MaskRecord::Painted(painted)) => Some(painted),
            Some(MaskRecord::Shared { .. }) => layer.painted_too.take(),
            other => {
                layer.mask = other;
                None
            }
        };
        match shared {
            Some((mask, inverted)) => {
                layer.mask = Some(MaskRecord::Shared { mask, inverted });
                layer.painted_too = painted;
            }
            None => {
                if layer.mask.is_none() {
                    layer.mask = painted.map(MaskRecord::Painted);
                }
                layer.painted_too = None;
            }
        }
        true
    }

    pub fn paint_layer_mask(&mut self, id: Uuid, stroke: Stroke) -> Result<(), StudioError> {
        let layer = self
            .layer_mut(id)
            .ok_or_else(|| StudioError::Document("the layer is not in the document".to_string()))?;
        match &mut layer.mask {
            Some(MaskRecord::Painted(mask)) => mask.strokes.push(stroke),
            // Behind a shared mask, the strokes limit the layer as well.
            Some(MaskRecord::Shared { .. }) => match &mut layer.painted_too {
                Some(mask) => mask.strokes.push(stroke),
                None if stroke.mode == StrokeMode::Subtract => {
                    return Err(StudioError::Document(
                        "the layer has no painted mask to erase from".to_string(),
                    ));
                }
                too @ None => {
                    *too = Some(PaintedMask {
                        strokes: vec![stroke],
                        blur: 0.0,
                    });
                }
            },
            Some(_) => {
                return Err(StudioError::Document(
                    "the layer has a range mask; remove it before painting one".to_string(),
                ));
            }
            mask @ None => {
                if stroke.mode == StrokeMode::Subtract {
                    return Err(StudioError::Document(
                        "the layer has no mask to erase from".to_string(),
                    ));
                }
                *mask = Some(MaskRecord::Painted(PaintedMask {
                    strokes: vec![stroke],
                    blur: 0.0,
                }));
            }
        }
        Ok(())
    }

    /// Takes a step out of the document and returns it. An adjustment
    /// layer's step goes with its layer.
    pub fn remove_step(&mut self, id: Uuid) -> Option<StepRecord> {
        if let Some(layer) = self.layer_of_adjustment(id) {
            return match self.remove_layer(layer)?.source {
                LayerSource::Adjust { step } => Some(step),
                _ => None,
            };
        }
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

    /// Brings a document written before adjustment layers held steps up to
    /// date: each older adjustment layer holds its operation as a step, and
    /// each Finish step becomes an adjustment layer at the top of the stack,
    /// in order, with the step's mask and opacity, shown when it was on. A
    /// star removal there gets a layer of the stars it takes out right above
    /// it, screened, shown when its stars were. The ids made are the same
    /// each time the document opens.
    pub fn adopt_adjustment_layers(&mut self) {
        for layer in &mut self.stages[StageKind::Combine as usize].layers {
            if let LayerSource::Adjustment {
                operation,
                settings,
            } = &layer.source
            {
                let mut step = StepRecord::new(StepKind::Operation {
                    operation: operation.clone(),
                    accepts: Accepts::Display,
                    settings: settings.clone(),
                });
                step.id = derived_id(layer.id, 1);
                layer.source = LayerSource::Adjust { step };
            }
        }
        let finish = std::mem::take(&mut self.stages[StageKind::Finish as usize].steps);
        for step in finish {
            let (visible, opacity, mask) = (
                step.enabled,
                step.opacity.clamp(0.0, 1.0),
                step.mask.clone(),
            );
            let stars =
                (step.kind.name() == super::flow::flow_operations::STAR_REMOVAL).then(|| {
                    let shown = super::flow::tool_of(&step)
                        .and_then(|tool| tool[super::flow::STARS_SHOWN_KEY].as_bool())
                        .unwrap_or(true);
                    let mut stars = super::flow::stars_layer(step.id);
                    stars.id = derived_id(step.id, 3);
                    stars.visible = shown;
                    stars
                });
            let name = step.kind.name().to_string();
            let mut own = step;
            own.enabled = true;
            own.opacity = 1.0;
            own.mask = None;
            let mut layer = LayerRecord::new(&name, LayerSource::Adjust { step: own });
            layer.id = derived_id(layer.source.adjustment_step().expect("an adjustment").id, 2);
            layer.visible = visible;
            layer.opacity = opacity;
            layer.mask = mask;
            let layers = &mut self.stages[StageKind::Combine as usize].layers;
            layers.push(layer);
            layers.extend(stars);
        }
    }

    /// The document as JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a document is always valid JSON")
    }

    /// Reads a document from JSON. Refuses a document from a newer build,
    /// and one whose stages are missing or out of order.
    pub fn from_json(text: &str) -> Result<Self, StudioError> {
        let mut document: Document =
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
        // A document from before each tool was a step of its own.
        super::flow::split_tool_chains(&mut document);
        // And from before the Finish stage was adjustment layers.
        document.adopt_adjustment_layers();
        Ok(document)
    }
}
