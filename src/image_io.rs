//! One door for reading an image frame, whichever container holds it.
//!
//! PSF Guard reads FITS and XISF. Seiza decodes both into the same
//! [`seiza_fits::FitsImage`] with the same FITS-style header cards, so nothing
//! downstream — statistics, star detection, stretching, solving, stacking —
//! needs to know which one it opened. Route every read and every "is this a
//! frame?" test through here so a new container only has to be added once.

#[cfg(test)]
use seiza_fits::WriteHeaderCard;
use seiza_fits::{FitsImage, HeaderValue};
use std::path::Path;

/// File extensions PSF Guard treats as image frames, matched case-insensitively.
///
/// `fts` is the old 8.3-era spelling of `fits`; N.I.N.A. and PixInsight both
/// still read it, so a catalog that contains one should not silently lose it.
pub const IMAGE_EXTENSIONS: &[&str] = &["fits", "fit", "fts", "xisf"];

/// Failure to read an image, whatever container it came from.
#[derive(Debug)]
pub enum ImageError {
    Fits(seiza_fits::FitsError),
    Xisf(seiza_xisf::XisfError),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fits(error) => write!(f, "{error}"),
            Self::Xisf(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ImageError {}

impl From<seiza_fits::FitsError> for ImageError {
    fn from(error: seiza_fits::FitsError) -> Self {
        Self::Fits(error)
    }
}

impl From<seiza_xisf::XisfError> for ImageError {
    fn from(error: seiza_xisf::XisfError) -> Self {
        Self::Xisf(error)
    }
}

/// Whether this extension names a frame container, with or without its
/// leading dot (`xisf` and `.xisf` both answer yes).
///
/// Config lists extensions dotted, so take both spellings rather than make
/// every caller remember which one it holds.
pub fn is_image_extension(extension: &str) -> bool {
    let extension = extension.strip_prefix('.').unwrap_or(extension);
    IMAGE_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
}

/// Whether this filename carries an image extension.
pub fn has_image_extension(filename: &str) -> bool {
    is_image_path(Path::new(filename))
}

/// Whether this path carries an image extension.
///
/// Extension only: a scan reads thousands of names and cannot afford to open
/// each one to sniff its signature.
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(is_image_extension)
}

/// The filename without its image extension, for naming derived output.
///
/// A name that is not an image comes back whole.
pub fn strip_image_extension(filename: &str) -> &str {
    if !has_image_extension(filename) {
        return filename;
    }
    filename
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(filename)
}

/// Read the header cards without decoding pixels.
pub fn read_header(path: &Path) -> Result<Vec<(String, HeaderValue)>, ImageError> {
    read_header_named(path, path)
}

/// [`read_header`] for a file whose own path does not name its container.
///
/// A remote upload streams into a temporary file inside a scanned image root.
/// That file must not carry a frame extension — a scan would pick it up
/// mid-write — so the decoder is chosen from the name the client declared
/// while the bytes are read from `path`.
pub fn read_header_named(
    path: &Path,
    declared: impl AsRef<Path>,
) -> Result<Vec<(String, HeaderValue)>, ImageError> {
    if seiza_xisf::is_xisf_path(declared.as_ref()) {
        Ok(seiza_xisf::read_header(path)?)
    } else {
        Ok(seiza_fits::read_header(path)?)
    }
}

/// A frame's header cards with the `HISTORY` and `COMMENT` text beside
/// them.
///
/// Processing software records its steps there (Siril in `HISTORY`, ASTAP in
/// `COMMENT`), which the plain card list leaves out. XISF frames carry
/// PixInsight's history as properties instead, which [`classify_frame`]
/// reads itself, so their commentary is empty.
#[derive(Debug, Clone, Default)]
pub struct FrameHeader {
    pub cards: Vec<(String, HeaderValue)>,
    pub history: Vec<String>,
    pub comments: Vec<String>,
}

/// [`read_header_named`] with the commentary kept.
pub fn read_frame_header(
    path: &Path,
    declared: impl AsRef<Path>,
) -> Result<FrameHeader, ImageError> {
    if seiza_xisf::is_xisf_path(declared.as_ref()) {
        Ok(FrameHeader {
            cards: seiza_xisf::read_header(path)?,
            ..FrameHeader::default()
        })
    } else {
        let header = seiza_fits::read_header_with_commentary(path)?;
        Ok(FrameHeader {
            cards: header.cards,
            history: header.history,
            comments: header.comments,
        })
    }
}

/// What processing a frame has been through, as far as its file can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    /// An acquisition as the camera wrote it.
    Raw,
    /// Bias, dark and/or flat applied; geometry unchanged.
    Calibrated,
    /// Resampled onto another frame's grid.
    Registered,
    /// A master or a stack: never a light.
    Integration,
}

impl FrameKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Calibrated => "calibrated",
            Self::Registered => "registered",
            Self::Integration => "integration",
        }
    }

    /// A calibrated or registered copy of one light.
    pub fn is_derivative(self) -> bool {
        matches!(self, Self::Calibrated | Self::Registered)
    }
}

/// What decided a [`FrameKind`], so a report can say why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KindEvidence {
    /// Nothing marked the frame; it is taken as raw.
    None,
    /// A mark the processing tool wrote into the header.
    Header,
    /// The file name, backed by floating-point samples.
    Name,
}

/// The program that wrote a derivative, when the file says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Producer {
    Pixinsight,
    Siril,
    /// Astro Pixel Processor.
    App,
    /// DeepSkyStacker.
    Dss,
    Astap,
    Maxim,
    Unknown,
}

impl Producer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pixinsight => "pixinsight",
            Self::Siril => "siril",
            Self::App => "app",
            Self::Dss => "dss",
            Self::Astap => "astap",
            Self::Maxim => "maxim",
            Self::Unknown => "unknown",
        }
    }
}

/// A frame's [`FrameKind`] with the evidence behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FrameClass {
    pub kind: FrameKind,
    pub evidence: KindEvidence,
    pub producer: Producer,
    /// A registered frame that was calibrated first. WBPP can leave both a
    /// `_r` and a `_c_r` copy of one light; the calibrated one is preferred.
    pub includes_calibration: bool,
}

impl Default for FrameClass {
    fn default() -> Self {
        Self::RAW
    }
}

impl FrameClass {
    pub const RAW: Self = Self {
        kind: FrameKind::Raw,
        evidence: KindEvidence::None,
        producer: Producer::Unknown,
        includes_calibration: false,
    };

    fn header(kind: FrameKind, producer: Producer, includes_calibration: bool) -> Self {
        Self {
            kind,
            evidence: KindEvidence::Header,
            producer,
            includes_calibration: includes_calibration || kind == FrameKind::Calibrated,
        }
    }
}

/// Classify a frame as raw, calibrated, registered or an integration.
///
/// Calibration tools copy the acquisition keywords forward, so `IMAGETYP`
/// and `DATE-OBS` say nothing about processing. The marks the tools add
/// decide, strongest first: integration keywords, then the processing
/// history (PixInsight's XISF properties; FITS `HISTORY`, ASTAP's
/// `COMMENT`, `CALSTAT`, `PEDESTAL`, read with [`read_frame_header`]), then
/// the file name (WBPP and ASTAP suffixes, Siril prefixes, DeepSkyStacker's
/// `.cal` and `.reg`). A name counts only for a frame with floating-point
/// samples: calibration output is float, camera data is integer, and a raw
/// `M31_r.fits` shot through an r filter must stay raw.
pub fn classify_frame(path: &Path, declared: impl AsRef<Path>, header: &FrameHeader) -> FrameClass {
    let declared = declared.as_ref();
    let headers = header.cards.as_slice();
    if has_integration_card(headers) || has_stack_count(headers) {
        let producer = fits_producer(headers, "");
        return FrameClass::header(FrameKind::Integration, producer, false);
    }
    let name = declared
        .file_name()
        .and_then(|name| name.to_str())
        .map(strip_image_extension)
        .unwrap_or_default();
    let float_samples = if seiza_xisf::is_xisf_path(declared) {
        let Ok(info) = seiza_xisf::inspect(path) else {
            return FrameClass::RAW;
        };
        let Some(image) = info.images.first() else {
            return FrameClass::RAW;
        };
        if let Some(class) = classify_xisf_properties(&image.properties, name) {
            return class;
        }
        matches!(
            image.sample_format,
            seiza_xisf::SampleFormat::Float32 | seiza_xisf::SampleFormat::Float64
        )
    } else {
        if let Some(class) = classify_fits_cards(headers, &header.history, &header.comments) {
            return class;
        }
        header_number(headers, "BITPIX").is_some_and(|bitpix| bitpix < 0.0)
    };
    if float_samples && let Some(class) = classify_name(name) {
        return class;
    }
    FrameClass::RAW
}

fn header_number(headers: &[(String, HeaderValue)], wanted: &str) -> Option<f64> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
        .and_then(|(_, value)| match value {
            HeaderValue::Integer(_) | HeaderValue::Float(_) => value.as_f64(),
            _ => None,
        })
}

fn header_text<'a>(headers: &'a [(String, HeaderValue)], wanted: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
        .and_then(|(_, value)| match value {
            HeaderValue::String(text) | HeaderValue::Raw(text) => Some(text.as_str()),
            _ => None,
        })
}

fn has_integration_card(headers: &[(String, HeaderValue)]) -> bool {
    let has_card = |wanted: &str| {
        headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(wanted))
    };
    has_card("NCOMBINE")
        || has_card("SEIZAMST")
        || header_text(headers, "IMAGETYP")
            .is_some_and(|text| text.to_ascii_uppercase().contains("MASTER"))
}

/// Frame counts the stacking tools write: Siril `STACKCNT`, APP `NUMFRAME`
/// (beside `INTEGRAT`), ASTAP's per-filter counts, MaxIm's `SNAPSHOT`. A
/// count of one is a single frame passed through.
const STACK_COUNT_CARDS: &[&str] = &[
    "STACKCNT", "NUMFRAME", "LIGH_CNT", "LUM_CNT", "RED_CNT", "GREN_CNT", "BLUE_CNT", "SNAPSHOT",
];

fn has_stack_count(headers: &[(String, HeaderValue)]) -> bool {
    STACK_COUNT_CARDS
        .iter()
        .any(|card| header_number(headers, card).is_some_and(|count| count > 1.0))
        || header_text(headers, "INTEGRAT").is_some()
        // ASTAP's `S`: stacked.
        || header_text(headers, "CALSTAT").is_some_and(|text| text.contains('S'))
}

/// PixInsight process classes that produce each kind, as they appear in an
/// XISF processing history.
const PIXINSIGHT_INTEGRATION: &[&str] =
    &["ImageIntegration", "DrizzleIntegration", "FastIntegration"];
const PIXINSIGHT_REGISTRATION: &[&str] = &["StarAlignment"];
const PIXINSIGHT_CALIBRATION: &[&str] = &["ImageCalibration", "CosmeticCorrection", "Debayer"];

fn history_names(history: &str, classes: &[&str]) -> bool {
    classes.iter().any(|class| {
        history.contains(&format!("class=\"{class}\""))
            || history.contains(&format!("class=&quot;{class}&quot;"))
    })
}

/// Classify from an XISF image's properties, or `None` when PixInsight left
/// no mark on it.
fn classify_xisf_properties(
    properties: &[seiza_xisf::XisfProperty],
    name: &str,
) -> Option<FrameClass> {
    let history = properties
        .iter()
        .filter(|property| property.id.starts_with("PixInsight:ProcessingHistory"))
        .filter_map(|property| property.value.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    let signed = |what: &str| {
        properties
            .iter()
            .any(|property| property.id == format!("PCL:Signature:{what}"))
    };
    let marked = !history.is_empty()
        || properties
            .iter()
            .any(|property| property.id.starts_with("PCL:Signature:"));
    if !marked {
        return None;
    }
    let calibrated = history_names(&history, PIXINSIGHT_CALIBRATION) || signed("Calibration");
    if history_names(&history, PIXINSIGHT_INTEGRATION) || signed("Integration") {
        return Some(FrameClass::header(
            FrameKind::Integration,
            Producer::Pixinsight,
            false,
        ));
    }
    if history_names(&history, PIXINSIGHT_REGISTRATION) || signed("Registration") {
        return Some(FrameClass::header(
            FrameKind::Registered,
            Producer::Pixinsight,
            calibrated,
        ));
    }
    if calibrated {
        return Some(FrameClass::header(
            FrameKind::Calibrated,
            Producer::Pixinsight,
            true,
        ));
    }
    // PixInsight processed it, but with nothing this list names (a noise
    // evaluation, a format conversion). The name may still say what it is;
    // otherwise it is called calibrated, the gentlest kind that keeps it out
    // of the catalog as a second light, which is what this mark has always
    // meant to import.
    Some(match classify_name(name) {
        Some(class) => FrameClass {
            evidence: KindEvidence::Header,
            producer: Producer::Pixinsight,
            ..class
        },
        None => FrameClass::header(FrameKind::Calibrated, Producer::Pixinsight, true),
    })
}

/// Words in a FITS `HISTORY` card that name each kind of processing. Siril
/// writes "Calibrated with a master dark" and "Cosmetic correction of ..."
/// and stacking parameters; PixInsight's FITS output names its processes.
/// Capture software leaves `HISTORY` empty. Siril's registration leaves no
/// card at all (it keeps the transforms in its `.seq` file), so an `r_` frame
/// is known by its name.
const HISTORY_INTEGRATION: &[&str] = &[
    "imageintegration",
    "drizzleintegration",
    "stacking",
    "stacked",
];
/// Registration, and the other steps that change a frame's geometry (Siril
/// writes `Crop (x=25, ...)` and `Rotation (-90 deg)`): none of them leaves a
/// copy that can stand in for the light's pixels.
const HISTORY_REGISTRATION: &[&str] = &[
    "registrat",
    "staralignment",
    "star alignment",
    "aligned",
    "crop (",
    "rotation (",
    "resampl",
    "mirror",
];
const HISTORY_CALIBRATION: &[&str] = &[
    "calibrat",
    "bias subtract",
    "dark subtract",
    "flat field",
    "flat-field",
    "flatfield",
    "master-bias",
    "master-dark",
    "master-flat",
    "cosmetic correction",
    "imagecalibration",
    "cosmeticcorrection",
    "debayer",
    "demosaic",
];

/// ASTAP's own `COMMENT` lines. Other comments are left alone: drivers and
/// capture programs write free text there.
const ASTAP_ALIGNED_COMMENT: &str = "calibrated & aligned by astap";
const ASTAP_CALIBRATED_COMMENT: &str = "calibrated by astap";

/// Calibration counts ASTAP writes beside `CALSTAT`.
const CALIBRATION_COUNT_CARDS: &[&str] = &["DARK_CNT", "FLAT_CNT", "BIAS_CNT"];

/// Classify a FITS frame from its cards and commentary, or `None` when
/// neither records any processing.
fn classify_fits_cards(
    headers: &[(String, HeaderValue)],
    history: &[String],
    comments: &[String],
) -> Option<FrameClass> {
    let history = history.join("\n").to_ascii_lowercase();
    let comments = comments.join("\n").to_ascii_lowercase();
    let says = |words: &[&str]| words.iter().any(|word| history.contains(word));
    let producer = fits_producer(headers, &format!("{history}\n{comments}"));
    if says(HISTORY_INTEGRATION) {
        return Some(FrameClass::header(FrameKind::Integration, producer, false));
    }
    // CALSTAT letters: B bias, D dark, F flat (MaxIm, ASTAP).
    let calstat = header_text(headers, "CALSTAT")
        .is_some_and(|text| text.chars().any(|letter| "BDF".contains(letter)));
    let calibrated = says(HISTORY_CALIBRATION)
        || calstat
        || comments.contains(ASTAP_CALIBRATED_COMMENT)
        || comments.contains(ASTAP_ALIGNED_COMMENT)
        || header_number(headers, "PEDESTAL").is_some()
        || CALIBRATION_COUNT_CARDS
            .iter()
            .any(|card| header_number(headers, card).is_some_and(|count| count > 0.0));
    if says(HISTORY_REGISTRATION) || comments.contains(ASTAP_ALIGNED_COMMENT) {
        return Some(FrameClass::header(
            FrameKind::Registered,
            producer,
            calibrated,
        ));
    }
    calibrated.then(|| FrameClass::header(FrameKind::Calibrated, producer, true))
}

/// The processing program, from the cards that name software and the
/// commentary. `SWCREATE` comes last: APP copies N.I.N.A.'s forward, while
/// MaxIm names itself there.
fn fits_producer(headers: &[(String, HeaderValue)], commentary: &str) -> Producer {
    let mut text = String::new();
    for card in [
        "PROGRAM", "SOFTWARE", "CREATOR", "ORIGIN", "SWMODIFY", "SWCREATE",
    ] {
        if let Some(value) = header_text(headers, card) {
            text.push_str(&value.to_ascii_lowercase());
            text.push('\n');
        }
    }
    text.push_str(commentary);
    let names: &[(&str, Producer)] = &[
        ("siril", Producer::Siril),
        ("astro pixel processor", Producer::App),
        ("deepskystacker", Producer::Dss),
        ("astap", Producer::Astap),
        ("maxim", Producer::Maxim),
        ("pixinsight", Producer::Pixinsight),
    ];
    names
        .iter()
        .find(|(name, _)| text.contains(name))
        .map(|(_, producer)| *producer)
        .unwrap_or(Producer::Unknown)
}

/// Suffix tokens at the end of a stem: WBPP's `_c` calibrated, `_cc`
/// cosmetic correction, `_d` debayered, `_r` registered; ASTAP's `_cal` and
/// `_aligned`.
const NAME_SUFFIXES: &[(&str, FrameKind, Producer)] = &[
    ("c", FrameKind::Calibrated, Producer::Pixinsight),
    ("cc", FrameKind::Calibrated, Producer::Pixinsight),
    ("d", FrameKind::Calibrated, Producer::Pixinsight),
    ("r", FrameKind::Registered, Producer::Pixinsight),
    ("cal", FrameKind::Calibrated, Producer::Astap),
    ("aligned", FrameKind::Registered, Producer::Astap),
];

/// Siril's sequence prefixes, which stack: `r_bkg_pp_light_00001`.
/// `bkg_` (background extraction) keeps the geometry, so it reads as
/// calibrated; `cropped_` changes it, so it reads as registered.
const SIRIL_PREFIXES: &[(&str, FrameKind)] = &[
    ("r_", FrameKind::Registered),
    ("cropped_", FrameKind::Registered),
    ("pp_", FrameKind::Calibrated),
    ("bkg_", FrameKind::Calibrated),
];

/// DeepSkyStacker's `<base>.cal.fits` and `<base>.reg.fits`.
const DSS_SUFFIXES: &[(&str, FrameKind)] = &[
    (".cal", FrameKind::Calibrated),
    (".reg", FrameKind::Registered),
];

fn suffix_kind(token: &str) -> Option<(FrameKind, Producer)> {
    NAME_SUFFIXES
        .iter()
        .find(|(suffix, _, _)| *suffix == token)
        .map(|(_, kind, producer)| (*kind, *producer))
}

fn name_class(kind: FrameKind, producer: Producer, includes_calibration: bool) -> FrameClass {
    FrameClass {
        kind,
        evidence: KindEvidence::Name,
        producer,
        includes_calibration: includes_calibration || kind == FrameKind::Calibrated,
    }
}

/// Classify from the file name alone.
fn classify_name(stem: &str) -> Option<FrameClass> {
    // Stacks: Siril's `<sequence>_stacked`, ASTAP's `..._stacked`.
    if stem.ends_with("stacked") {
        return Some(name_class(FrameKind::Integration, Producer::Unknown, false));
    }
    for (suffix, kind) in DSS_SUFFIXES {
        if stem.len() > suffix.len() && stem.ends_with(suffix) {
            return Some(name_class(*kind, Producer::Dss, false));
        }
    }
    let suffixes: Vec<(FrameKind, Producer)> = stem.rsplit('_').map_while(suffix_kind).collect();
    // A stem made only of suffix tokens ("r", "c_r") has no frame name left.
    if !suffixes.is_empty() && suffixes.len() < stem.split('_').count() {
        let registered = suffixes
            .iter()
            .any(|(kind, _)| *kind == FrameKind::Registered);
        let calibrated = suffixes
            .iter()
            .any(|(kind, _)| *kind == FrameKind::Calibrated);
        let kind = if registered {
            FrameKind::Registered
        } else {
            FrameKind::Calibrated
        };
        return Some(name_class(kind, suffixes[0].1, calibrated));
    }
    let mut rest = stem;
    let mut kinds = Vec::new();
    while let Some((prefix, kind)) = SIRIL_PREFIXES
        .iter()
        .find(|(prefix, _)| rest.len() > prefix.len() && rest.starts_with(prefix))
    {
        rest = &rest[prefix.len()..];
        kinds.push(*kind);
    }
    let first = *kinds.first()?;
    let calibrated = kinds.contains(&FrameKind::Calibrated);
    Some(name_class(first, Producer::Siril, calibrated))
}

/// Whether a file name alone looks like a calibrated or registered copy
/// (WBPP, ASTAP, DeepSkyStacker or Siril naming). A cheap filter for scans
/// that cannot read every header; [`classify_frame`] decides.
pub fn name_suggests_derivative(file_name: &str) -> bool {
    classify_name(strip_image_extension(file_name)).is_some_and(|class| class.kind.is_derivative())
}

/// The name of the light a derivative was made from: the stem with the
/// tools' prefixes and suffixes taken off. A raw frame's stem comes back
/// unchanged, so pairing can compare the two.
pub fn derivative_base_stem(stem: &str) -> &str {
    let mut base = stem;
    while let Some((prefix, _)) = SIRIL_PREFIXES
        .iter()
        .find(|(prefix, _)| base.len() > prefix.len() && base.starts_with(prefix))
    {
        base = &base[prefix.len()..];
    }
    for (suffix, _) in DSS_SUFFIXES {
        if let Some(head) = base.strip_suffix(suffix).filter(|head| !head.is_empty()) {
            return head;
        }
    }
    while let Some((head, tail)) = base.rsplit_once('_') {
        if head.is_empty() || suffix_kind(tail).is_none() {
            break;
        }
        base = head;
    }
    base
}

/// The scale a normalized frame is placed on: full-well for 16-bit data.
///
/// PSF Guard compares background and flux across frames in physical ADU, and
/// almost every frame it meets is 16-bit camera data. A PixInsight float
/// frame that declares itself normalized has no ADU of its own, so the
/// nearest honest thing is to put it on the same scale as its neighbours.
pub const NORMALIZED_FULL_SCALE: f32 = 65535.0;

/// Decode a frame's pixels and headers.
///
/// A float XISF frame that declares `bounds="0:1"` is placed on a 16-bit
/// scale on the way through. PixInsight normalizes float images, so such a
/// frame's samples run 0..1 where a camera frame's run in the thousands, and
/// leaving them alone would make every cross-frame background and flux
/// comparison meaningless — quality screening would read the normalized frame
/// as a near-black outlier and its neighbours as blown. Only an exact `0:1`
/// is converted; seiza declines any other declared range, because writers
/// disagree about what it means.
pub fn open(path: &Path) -> Result<FitsImage, ImageError> {
    if seiza_xisf::is_xisf_path(path) {
        let mut read = seiza_xisf::read_image(path)?;
        read.rescale_normalized_to(NORMALIZED_FULL_SCALE);
        Ok(read.image)
    } else {
        Ok(FitsImage::open(path)?)
    }
}

/// Open a frame as linear samples for calibration and stacking, with the same
/// normalization [`open`] applies.
///
/// Stacking compares frames against a reference, so one normalized frame
/// among camera frames skews normalization and rejection for the whole group.
/// A no-op for FITS and for any XISF that does not declare itself normalized,
/// which is why every stacking read goes through here rather than picking and
/// choosing.
pub fn open_linear_frame(
    path: impl AsRef<Path>,
) -> Result<seiza_stacking::FitsFrame, seiza_stacking::Error> {
    let mut frame = seiza_stacking::FitsFrame::open(path)?;
    frame.rescale_declared_unit_bounds(NORMALIZED_FULL_SCALE);
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn recognizes_every_image_extension_in_any_case() {
        for name in [
            "frame.fits",
            "frame.FITS",
            "frame.fit",
            "frame.FIT",
            "frame.fts",
            "frame.xisf",
            "frame.XISF",
            "frame.Xisf",
        ] {
            assert!(has_image_extension(name), "{name} should be an image");
        }
    }

    #[test]
    fn rejects_sidecars_and_extensionless_names() {
        for name in ["frame.json", "frame.txt", "frame.png", "frame", "frame."] {
            assert!(!has_image_extension(name), "{name} should not be an image");
        }
    }

    #[test]
    fn strips_only_image_extensions() {
        assert_eq!(strip_image_extension("frame.fits"), "frame");
        assert_eq!(strip_image_extension("frame.XISF"), "frame");
        assert_eq!(
            strip_image_extension("m31.2026-01-01.fit"),
            "m31.2026-01-01"
        );
        assert_eq!(strip_image_extension("notes.json"), "notes.json");
    }

    /// A sample 4x3 mono XISF light frame, written by the same XISF writer a
    /// reader in the wild would meet. Generating it beats checking in a blob:
    /// the fixture cannot drift away from the format the crate speaks.
    ///
    /// The samples span 0..1, so the writer declares `bounds="0:1"` and the
    /// frame reads back as a normalized one.
    fn write_sample_xisf(directory: &Path, name: &str) -> PathBuf {
        write_xisf_spanning(directory, name, 0.0, 1.0)
    }

    /// The same frame with its samples spread evenly over `low..=high`.
    fn write_xisf_spanning(directory: &Path, name: &str, low: f32, high: f32) -> PathBuf {
        let pixels: Vec<f32> = (0..12)
            .map(|index| low + (high - low) * index as f32 / 11.0)
            .collect();
        let path = directory.join(name);
        seiza_xisf::write_f32_image(
            &path,
            4,
            3,
            seiza_fits::F32ImageData::Mono(&pixels),
            &[
                WriteHeaderCard::new("IMAGETYP", HeaderValue::String("LIGHT".into())),
                WriteHeaderCard::new("OBJECT", HeaderValue::String("M31".into())),
                WriteHeaderCard::new("FILTER", HeaderValue::String("Ha".into())),
                WriteHeaderCard::new("EXPTIME", HeaderValue::Float(300.0)),
            ],
        )
        .expect("sample XISF should write");
        path
    }

    fn write_temp(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        (directory, path)
    }

    #[test]
    fn opens_an_xisf_frame_through_the_shared_reader() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_sample_xisf(directory.path(), "light.xisf");
        let image = open(&path).expect("XISF should decode");
        assert_eq!((image.width, image.height, image.planes), (4, 3, 1));
        assert!(
            matches!(image.pixels, seiza_fits::Pixels::F32(ref samples) if samples.len() == 12)
        );
    }

    #[test]
    fn reads_xisf_headers_without_decoding_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_sample_xisf(directory.path(), "light.xisf");
        let headers = read_header(&path).expect("XISF headers should parse");
        let text = |keyword: &str| {
            headers
                .iter()
                .find(|(name, _)| name == keyword)
                .and_then(|(_, value)| value.as_str())
                .map(str::to_string)
        };
        assert_eq!(text("OBJECT").as_deref(), Some("M31"));
        assert_eq!(text("FILTER").as_deref(), Some("Ha"));
        assert_eq!(text("IMAGETYP").as_deref(), Some("LIGHT"));
        assert_eq!(
            headers
                .iter()
                .find(|(name, _)| name == "EXPTIME")
                .and_then(|(_, value)| value.as_f64()),
            Some(300.0)
        );
    }

    #[test]
    fn xisf_frames_reach_the_shared_astrometry_header_reader() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_sample_xisf(directory.path(), "light.xisf");
        let headers = crate::astrometry_headers::FitsAstrometryHeaders::from_path(&path)
            .expect("XISF headers should normalize");
        assert_eq!(
            headers.object_name.map(|value| value.value),
            Some("M31".into())
        );
        assert_eq!(headers.width.map(|value| value.value), Some(4));
        assert_eq!(headers.height.map(|value| value.value), Some(3));
    }

    /// A remote upload streams into an extensionless temporary inside a
    /// scanned image root. The decoder has to come from the declared name, or
    /// the temporary would need a frame extension and a concurrent scan would
    /// pick it up mid-write.
    #[test]
    fn a_declared_name_picks_the_decoder_for_an_extensionless_file() {
        let directory = tempfile::tempdir().unwrap();
        let written = write_sample_xisf(directory.path(), "light.xisf");
        let bare = directory.path().join(".tmpAbC123");
        std::fs::rename(&written, &bare).unwrap();

        assert!(
            !is_image_path(&bare),
            "the temporary must stay invisible to scans"
        );
        assert!(
            read_header(&bare).is_err(),
            "without the declared name this is read as FITS"
        );

        let headers = read_header_named(&bare, "light.xisf").expect("declared name should decode");
        assert_eq!(
            headers
                .iter()
                .find(|(name, _)| name == "OBJECT")
                .and_then(|(_, value)| value.as_str()),
            Some("M31")
        );
    }

    /// The finding this exists for: PixInsight normalizes float images, so a
    /// frame declaring `0:1` has samples four orders of magnitude below a
    /// camera frame's, and every cross-frame ADU comparison built on it is
    /// meaningless.
    #[test]
    fn a_normalized_xisf_frame_lands_on_a_sixteen_bit_scale() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_sample_xisf(directory.path(), "light.xisf");

        let seiza_fits::Pixels::F32(samples) = open(&path).unwrap().pixels else {
            panic!("expected float samples");
        };
        assert_eq!(samples.first().copied(), Some(0.0));
        assert_eq!(samples.last().copied(), Some(65535.0));
    }

    /// Only an exact `0:1` is treated as normalized. Any other declared range
    /// is ambiguous — this crate's own stack output declares the observed
    /// minimum and maximum — so the samples must survive untouched.
    #[test]
    fn a_physical_xisf_frame_passes_through_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_xisf_spanning(directory.path(), "light.xisf", 100.0, 30000.0);

        let seiza_fits::Pixels::F32(samples) = open(&path).unwrap().pixels else {
            panic!("expected float samples");
        };
        assert_eq!(samples.first().copied(), Some(100.0));
        assert_eq!(samples.last().copied(), Some(30000.0));
    }

    /// The measured value the grader actually compares across frames. Before
    /// the conversion this read about 0.01 against a camera frame's thousands.
    #[test]
    fn a_normalized_frame_reports_adu_a_camera_frame_can_be_compared_with() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_sample_xisf(directory.path(), "light.xisf");

        let frame = crate::image_analysis::FitsImage::from_file(&path).unwrap();
        let statistics = frame.calculate_basic_statistics();
        let median_adu = frame.stored_to_adu(statistics.median);
        assert!(
            (1000.0..65536.0).contains(&median_adu),
            "median should read as 16-bit ADU, got {median_adu}"
        );
    }

    /// Stacking normalizes every frame against a reference, so one normalized
    /// frame among camera frames would skew the whole group.
    #[test]
    fn the_stacking_reader_applies_the_same_conversion() {
        let directory = tempfile::tempdir().unwrap();
        let normalized = write_sample_xisf(directory.path(), "normalized.xisf");
        let frame = open_linear_frame(&normalized).unwrap();
        assert_eq!(frame.image.data.first().copied(), Some(0.0));
        assert_eq!(frame.image.data.last().copied(), Some(65535.0));

        let physical = write_xisf_spanning(directory.path(), "physical.xisf", 100.0, 30000.0);
        let frame = open_linear_frame(&physical).unwrap();
        assert_eq!(frame.image.data.first().copied(), Some(100.0));
        assert_eq!(frame.image.data.last().copied(), Some(30000.0));
    }

    #[test]
    fn reports_which_container_failed() {
        let (_directory, path) = write_temp("broken.xisf", b"not an xisf file at all");
        let error = open(&path).expect_err("a non-XISF body must fail");
        assert!(matches!(error, ImageError::Xisf(_)), "{error}");

        let (_directory, path) = write_temp("broken.fits", b"not a fits file at all");
        let error = open(&path).expect_err("a non-FITS body must fail");
        assert!(matches!(error, ImageError::Fits(_)), "{error}");
    }

    fn property(id: &str, value: Option<&str>) -> seiza_xisf::XisfProperty {
        seiza_xisf::XisfProperty {
            id: id.into(),
            type_name: "String".into(),
            value: value.map(str::to_string),
            comment: None,
            format: None,
            location: None,
        }
    }

    fn history(classes: &[&str]) -> String {
        let instances: String = classes
            .iter()
            .map(|class| format!("<instance class=\"{class}\" version=\"256\"/>"))
            .collect();
        format!("<?xml version=\"1.0\"?><ProcessingHistory>{instances}</ProcessingHistory>")
    }

    #[test]
    fn pixinsight_history_and_signatures_decide_the_kind() {
        let calibrated = [
            property(
                "PixInsight:ProcessingHistory",
                Some(&history(&["ImageCalibration"])),
            ),
            property("PCL:Signature:Calibration", None),
        ];
        let class = classify_xisf_properties(&calibrated, "frame_0115_c").unwrap();
        assert_eq!(class.kind, FrameKind::Calibrated);
        assert_eq!(class.evidence, KindEvidence::Header);
        assert_eq!(class.producer, Producer::Pixinsight);

        let registered = [property(
            "PixInsight:ProcessingHistory",
            Some(&history(&["ImageCalibration", "StarAlignment"])),
        )];
        let class = classify_xisf_properties(&registered, "x").unwrap();
        assert_eq!(class.kind, FrameKind::Registered);
        assert!(class.includes_calibration);

        // A registered raw: WBPP's `_r` beside `_c_r`.
        let signed_only = [property("PCL:Signature:Registration", None)];
        let class = classify_xisf_properties(&signed_only, "x").unwrap();
        assert_eq!(class.kind, FrameKind::Registered);
        assert!(!class.includes_calibration);

        let integrated = [property(
            "PixInsight:ProcessingHistory",
            Some(&history(&[
                "ImageCalibration",
                "StarAlignment",
                "ImageIntegration",
            ])),
        )];
        assert_eq!(
            classify_xisf_properties(&integrated, "x").unwrap().kind,
            FrameKind::Integration
        );

        // Processed by something the list does not name: the name may say
        // more, and otherwise it is still kept out of the lights.
        let unnamed = [property("PCL:Signature:NoiseEvaluation", None)];
        assert_eq!(
            classify_xisf_properties(&unnamed, "frame_r").unwrap().kind,
            FrameKind::Registered
        );
        assert_eq!(
            classify_xisf_properties(&unnamed, "frame").unwrap().kind,
            FrameKind::Calibrated
        );

        let camera_only = [property(
            "Instrument:Camera:Name",
            Some("ZWO ASI2600MM Pro"),
        )];
        assert_eq!(classify_xisf_properties(&camera_only, "frame_c"), None);
    }

    #[test]
    fn fits_history_and_calibration_cards_decide_the_kind() {
        let text = |name: &str, value: &str| (name.to_string(), HeaderValue::String(value.into()));
        let lines = |texts: &[&str]| texts.iter().map(|t| t.to_string()).collect::<Vec<_>>();

        let siril = [text("PROGRAM", "Siril 1.2.6")];
        let class =
            classify_fits_cards(&siril, &lines(&["Calibrated with a master dark"]), &[]).unwrap();
        assert_eq!(class.kind, FrameKind::Calibrated);
        assert_eq!(class.producer, Producer::Siril);

        let class = classify_fits_cards(&[], &lines(&["Registration with shift"]), &[]).unwrap();
        assert_eq!(class.kind, FrameKind::Registered);
        assert!(!class.includes_calibration);

        let maxim = [text("CALSTAT", "BDF")];
        assert_eq!(
            classify_fits_cards(&maxim, &[], &[]).unwrap().kind,
            FrameKind::Calibrated
        );
        let pedestal = [("PEDESTAL".to_string(), HeaderValue::Integer(100))];
        assert_eq!(
            classify_fits_cards(&pedestal, &[], &[]).unwrap().kind,
            FrameKind::Calibrated
        );

        // ASTAP: CALSTAT letters, its own comments, its calibration counts.
        let astap = [
            text("CALSTAT", "DF"),
            ("DARK_CNT".to_string(), HeaderValue::Integer(20)),
        ];
        let class = classify_fits_cards(
            &astap,
            &[],
            &lines(&["1  Calibrated & aligned by ASTAP. www.hnsky.org"]),
        )
        .unwrap();
        assert_eq!(
            (class.kind, class.producer, class.includes_calibration),
            (FrameKind::Registered, Producer::Astap, true)
        );

        // Siril's crop changes the geometry.
        let class = classify_fits_cards(&[], &lines(&["Crop (x=25, y=31, w=5263, h=4138)"]), &[]);
        assert_eq!(class.unwrap().kind, FrameKind::Registered);

        // A capture program's free-text comment is not processing.
        let class = classify_fits_cards(&[], &[], &lines(&["Calibration target: none"]));
        assert_eq!(class, None);

        // Capture software: no history, no marks.
        let nina = [text("SWCREATE", "N.I.N.A. 3.1"), text("IMAGETYP", "LIGHT")];
        assert_eq!(classify_fits_cards(&nina, &[], &[]), None);
    }

    #[test]
    fn names_follow_wbpp_suffixes_and_siril_prefixes() {
        let kind = |stem: &str| classify_name(stem).map(|class| class.kind);
        assert_eq!(kind("frame_0042_c"), Some(FrameKind::Calibrated));
        assert_eq!(kind("frame_0042_c_cc"), Some(FrameKind::Calibrated));
        assert_eq!(kind("frame_0042_c_d"), Some(FrameKind::Calibrated));
        assert_eq!(kind("frame_0042_c_r"), Some(FrameKind::Registered));
        assert_eq!(kind("frame_0042_r"), Some(FrameKind::Registered));
        assert_eq!(kind("pp_light_00001"), Some(FrameKind::Calibrated));
        assert_eq!(kind("r_pp_light_00001"), Some(FrameKind::Registered));
        assert_eq!(kind("bkg_pp_light_00001"), Some(FrameKind::Calibrated));
        assert_eq!(
            kind("cropped_r_alpha-o3_00001"),
            Some(FrameKind::Registered)
        );
        assert_eq!(kind("r_pp_light_stacked"), Some(FrameKind::Integration));
        assert_eq!(kind("M31_0001_cal"), Some(FrameKind::Calibrated));
        assert_eq!(kind("M31_0001_aligned"), Some(FrameKind::Registered));
        assert_eq!(kind("M31_0001.cal"), Some(FrameKind::Calibrated));
        assert_eq!(kind("M31_0001.reg"), Some(FrameKind::Registered));
        assert_eq!(
            classify_name("M31_0001.reg").unwrap().producer,
            Producer::Dss
        );
        assert_eq!(
            classify_name("M31_0001_cal").unwrap().producer,
            Producer::Astap
        );
        assert_eq!(kind("frame_0042"), None);
        assert_eq!(kind("pp_"), None);
        assert_eq!(kind("c_r"), None, "no frame name left");
        assert!(
            classify_name("frame_0042_c_r")
                .unwrap()
                .includes_calibration
        );
        assert!(!classify_name("frame_0042_r").unwrap().includes_calibration);

        assert_eq!(derivative_base_stem("frame_0042_c_cc_r"), "frame_0042");
        assert_eq!(derivative_base_stem("r_pp_light_00001"), "light_00001");
        assert_eq!(derivative_base_stem("frame_0042"), "frame_0042");
        assert_eq!(derivative_base_stem("c"), "c");
        assert_eq!(derivative_base_stem("M31_0001.cal"), "M31_0001");
        assert_eq!(derivative_base_stem("M31_0001_cal"), "M31_0001");
        assert_eq!(
            derivative_base_stem("cropped_r_alpha-o3_00001"),
            "alpha-o3_00001"
        );
    }

    /// A minimal FITS file: the given cards, then one block of zero pixels.
    fn write_fits(directory: &Path, name: &str, bitpix: i32, cards: &[&str]) -> PathBuf {
        let mut header: Vec<String> = vec![
            "SIMPLE  =                    T".into(),
            format!("BITPIX  = {bitpix:>20}"),
            "NAXIS   =                    2".into(),
            "NAXIS1  =                    4".into(),
            "NAXIS2  =                    3".into(),
        ];
        header.extend(cards.iter().map(|card| card.to_string()));
        header.push("END".into());
        let mut bytes: Vec<u8> = header
            .iter()
            .flat_map(|card| format!("{card:<80}").into_bytes())
            .collect();
        bytes.resize(2880, b' ');
        bytes.resize(2880 * 2, 0);
        let path = directory.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn classify_path(path: &Path) -> FrameClass {
        let header = read_frame_header(path, path).expect("header should read");
        classify_frame(path, path, &header)
    }

    #[test]
    fn a_frame_is_classified_from_its_file() {
        let directory = tempfile::tempdir().unwrap();
        let siril = write_fits(
            directory.path(),
            "pp_light_00001.fit",
            -32,
            &[
                "PROGRAM = 'Siril 1.2.6'",
                "HISTORY Calibration: master-dark.fit",
            ],
        );
        let class = classify_path(&siril);
        assert_eq!(
            (class.kind, class.evidence, class.producer),
            (FrameKind::Calibrated, KindEvidence::Header, Producer::Siril)
        );

        // No history, but a WBPP name on float samples.
        let named = write_fits(directory.path(), "frame_0042_c_r.fits", -32, &[]);
        let class = classify_path(&named);
        assert_eq!(
            (class.kind, class.evidence),
            (FrameKind::Registered, KindEvidence::Name)
        );

        // The same name on camera integers is a raw frame shot through an r
        // filter, not a WBPP output.
        let raw = write_fits(directory.path(), "M31_0042_r.fits", 16, &[]);
        assert_eq!(classify_path(&raw), FrameClass::RAW);

        for (name, card) in [
            ("dss.fits", "NCOMBINE=                   40"),
            ("siril.fit", "STACKCNT=                   40"),
            ("app.fits", "INTEGRAT= 'Integration'"),
            ("astap.fit", "CALSTAT = 'DFBS'"),
        ] {
            let stack = write_fits(directory.path(), name, -32, &[card]);
            assert_eq!(classify_path(&stack).kind, FrameKind::Integration, "{name}");
        }
        // One frame "stacked" is a frame passed through.
        let single = write_fits(
            directory.path(),
            "single.fits",
            16,
            &["STACKCNT=                    1"],
        );
        assert_eq!(classify_path(&single), FrameClass::RAW);

        let astap = write_fits(
            directory.path(),
            "M31_0001.fit",
            -32,
            &[
                "CALSTAT = 'DF'",
                "COMMENT 1  Calibrated by ASTAP. www.hnsky.org",
            ],
        );
        let class = classify_path(&astap);
        assert_eq!(
            (class.kind, class.producer),
            (FrameKind::Calibrated, Producer::Astap)
        );

        // An XISF frame without PixInsight's marks falls back to its name.
        let plain = write_sample_xisf(directory.path(), "frame_0042_c.xisf");
        assert_eq!(classify_path(&plain).kind, FrameKind::Calibrated);
        let plain_raw = write_sample_xisf(directory.path(), "frame_0042.xisf");
        assert_eq!(classify_path(&plain_raw), FrameClass::RAW);
    }

    /// The real WBPP output of the C925 NGC 6543 run, on the maintainer's
    /// read-only share. Skipped where the share is not mounted.
    #[test]
    fn real_wbpp_output_is_classified_from_its_headers() {
        let root = Path::new("/mnt/barium/astrobin/_ByTelescope/C925");
        let process = root.join("_Process/2026/NGC 6543");
        let raw = root.join(
            "_Source/2026/NGC 6543/2026-06-06/LIGHT/2026-06-07_03-20-00_B_-10.00_75.00s_0115.fits",
        );
        if !raw.is_file() {
            return;
        }
        let folder = "Light_BIN-1_6248x4176_EXPOSURE-75.00s_FILTER-B_mono";
        let stem = "2026-06-07_03-20-00_B_-10.00_75.00s_0115";
        let cases = [
            (raw.clone(), FrameKind::Raw, false),
            (
                process.join(format!("calibrated/{folder}/{stem}_c.xisf")),
                FrameKind::Calibrated,
                true,
            ),
            (
                process.join(format!("registered/{folder}/{stem}_c_r.xisf")),
                FrameKind::Registered,
                true,
            ),
            (
                process.join(format!("registered/{folder}/{stem}_r.xisf")),
                FrameKind::Registered,
                false,
            ),
        ];
        for (path, kind, includes_calibration) in cases {
            let class = classify_path(&path);
            assert_eq!(class.kind, kind, "{}", path.display());
            if kind != FrameKind::Raw {
                assert_eq!(class.evidence, KindEvidence::Header, "{}", path.display());
                assert_eq!(class.includes_calibration, includes_calibration);
            }
        }
    }

    /// Real Siril and Astro Pixel Processor output on the same share: an APP
    /// stack, Siril's registration of it (no mark but its name), Siril's crop
    /// of that, and a Siril-processed stack. Skipped where the share is not
    /// mounted.
    #[test]
    fn real_siril_and_app_output_is_classified() {
        let folder = Path::new(
            "/mnt/barium/astrobin/_ByTelescope/Radian61/_Process/2025/NGC7000/2025-07-18/Siril",
        );
        if !folder.is_dir() {
            return;
        }
        let cases = [
            (
                "Target-Oxygen_III-session_1.fits",
                FrameKind::Integration,
                KindEvidence::Header,
                Producer::App,
            ),
            (
                "r_alpha-o3_00001.fit",
                FrameKind::Registered,
                KindEvidence::Name,
                Producer::Siril,
            ),
            (
                "cropped_r_alpha-o3_00001.fit",
                FrameKind::Registered,
                KindEvidence::Header,
                Producer::Siril,
            ),
            (
                "Pixel Math result.fit",
                FrameKind::Integration,
                KindEvidence::Header,
                Producer::Siril,
            ),
        ];
        for (name, kind, evidence, producer) in cases {
            let class = classify_path(&folder.join(name));
            assert_eq!(
                (class.kind, class.evidence, class.producer),
                (kind, evidence, producer),
                "{name}"
            );
        }
    }
}
