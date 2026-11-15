//! Seiza's learned noise model over one image, as a worker task.
//!
//! The model is a step of the processing chain beside the RC-Astro tools:
//! it runs after sharpening and before star removal, and needs no RC-Astro
//! license. The server writes the image to `input.fits` and sends which
//! model the step runs and how much of the noise to remove. The worker runs
//! the model and writes `processed.fits`.
//!
//! `seiza-model` links nothing native. It runs the model through ONNX
//! Runtime, which is opened at run time from a library on disk. PSF Guard
//! never fetches the library or the model by itself: a person downloads them
//! (see [`install`]), or gives the path of an ONNX Runtime they installed.
//! Until both are present the step is not offered.

pub mod install;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use seiza_stacking::{
    ExternalParameterKind, ExternalParameterValue, ExternalToolParameter, ExternalToolRequest,
    ExternalToolSchema, LinearImage,
};
use serde::{Deserialize, Serialize};

use super::rc_astro::StepResult;

/// The step's key in a processing chain, and its `--work` choice.
pub const TOOL: &str = "noise-model";
/// The step's name, as the processing controls show it.
pub const NAME: &str = "Seiza noise model";
pub const INPUT: &str = super::rc_astro::INPUT;
pub const PROCESSED: &str = super::rc_astro::PROCESSED;

/// The share of the pixel-scale grain the step removes.
pub const GRAIN: &str = "grain";
/// The share of the wider mottle the step removes.
pub const MOTTLE: &str = "mottle";
/// Seiza's defaults for `seiza model denoise`.
const DEFAULT_GRAIN: f64 = 0.7;
const DEFAULT_MOTTLE: f64 = 0.9;
/// The Gaussian scale, in pixels, that divides the grain from the mottle.
/// Seiza's default; part of [`Identity::version`], so a change makes new
/// cache identities.
const BAND_SCALE_PX: f32 = 5.0;

/// Marks the model's card hash in [`Identity::version`].
const CARD_MARK: &str = ", card ";

/// The file name of ONNX Runtime's library on this platform.
#[cfg(target_os = "macos")]
pub const LIBRARY_FILE: &str = "libonnxruntime.dylib";
#[cfg(target_os = "windows")]
pub const LIBRARY_FILE: &str = "onnxruntime.dll";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const LIBRARY_FILE: &str = "libonnxruntime.so";

/// Gives the folder for the downloads, in place of PSF Guard's data folder.
pub const FOLDER_VARIABLE: &str = "PSF_GUARD_NOISE_MODEL_DIR";

/// Everything that decides a run's pixels besides the image and the
/// amounts: the model, by its card's hash, Seiza's output version, the
/// backend and the graph's precision. A worker offers it, a task demands
/// it, and a result's cache identity includes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The model's name, such as `noise-v1-d33`.
    pub model: String,
    /// The SHA-256 of the model's card, which covers its graphs.
    pub card_sha256: String,
    /// `seiza_model::OUTPUT_VERSION` of the build that runs it.
    pub output_version: String,
    /// The backend, as Seiza spells it: `cpu`, `coreml-gpu`, and so on.
    pub backend: String,
    /// The graph's precision: `f32` or `f16`.
    pub precision: String,
}

impl Identity {
    /// The identity as one line. It is the step's schema version, so the
    /// chain's cache identity and the task's demand both include every part.
    pub fn version(&self) -> String {
        format!(
            "{} {} {}, seiza-model {}, band scale {BAND_SCALE_PX}{CARD_MARK}{}",
            self.model, self.backend, self.precision, self.output_version, self.card_sha256
        )
    }

    /// The step's schema, in the shape the RC-Astro tools' schemas have, so
    /// one chain validates, hashes and runs both.
    pub fn schema(&self) -> ExternalToolSchema {
        let amount =
            |name: &str, label: &str, description: &str, default: f64| ExternalToolParameter {
                name: name.into(),
                flag: Some(format!("--{name}")),
                label: label.into(),
                description: description.into(),
                kind: ExternalParameterKind::Float {
                    default,
                    min: 0.0,
                    max: 1.0,
                },
            };
        ExternalToolSchema {
            schema_version: 1,
            cli_version: self.version(),
            key: TOOL.into(),
            name: NAME.into(),
            ml_version: None,
            licensed: true,
            license_message: None,
            parameters: vec![
                amount(
                    GRAIN,
                    "Grain",
                    "The share of the pixel-scale grain removed, from 0 to 1",
                    DEFAULT_GRAIN,
                ),
                amount(
                    MOTTLE,
                    "Mottle",
                    "The share of the wider mottle removed, from 0 to 1",
                    DEFAULT_MOTTLE,
                ),
            ],
        }
    }
}

/// A schema version without the card's hash, for a person to read:
/// `noise-v1-d33 cpu f32, seiza-model 0.1.0, band scale 5`.
pub fn display_version(version: &str) -> &str {
    version.split(CARD_MARK).next().unwrap_or(version)
}

/// Whether a chain step is the noise model.
pub fn is_step(tool: &str) -> bool {
    tool == TOOL
}

/// How much of the noise the model finds is removed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Amounts {
    pub grain: f32,
    pub mottle: f32,
}

impl Amounts {
    /// The amounts a chain step asks for; a parameter it leaves out keeps
    /// the schema's default.
    pub fn from_request(request: &ExternalToolRequest) -> Result<Self, String> {
        let amount = |name: &str, default: f64| -> Result<f32, String> {
            let value = match request.parameters.iter().find(|(key, _)| key == name) {
                None => default,
                Some((_, ExternalParameterValue::Float(value))) => *value,
                Some((_, ExternalParameterValue::Int(value))) => *value as f64,
                Some((_, ExternalParameterValue::Bool(_))) => {
                    return Err(format!("{NAME}: {name} must be a number"));
                }
            };
            if !(0.0..=1.0).contains(&value) {
                return Err(format!("{NAME}: {name} = {value} is outside [0, 1]"));
            }
            Ok(value as f32)
        };
        Ok(Self {
            grain: amount(GRAIN, DEFAULT_GRAIN)?,
            mottle: amount(MOTTLE, DEFAULT_MOTTLE)?,
        })
    }

    fn bands(self) -> seiza_model::NoiseBands {
        seiza_model::NoiseBands {
            high: self.grain,
            low: self.mottle,
            scale_px: BAND_SCALE_PX,
        }
    }
}

/// What the server sends with the image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepSpec {
    /// The model the step runs. A worker whose own differs fails the task
    /// instead of writing other pixels under this identity.
    pub identity: Identity,
    pub amounts: Amounts,
    /// A person waits for the result.
    #[serde(default)]
    pub person: bool,
}

/// Runs the model on this machine. The server and the worker command use
/// [`SeizaEngine`]; tests use a stand-in, since they have no ONNX Runtime.
pub trait Engine: Send + Sync {
    /// The model this machine would run, or what is missing. Opens ONNX
    /// Runtime when its library is on disk.
    fn identity(&self) -> Result<Identity, String>;

    /// Run the model `wanted` over `image` and remove `amounts` of the noise
    /// it finds. `stop` is read between tiles; `progress` hears how much is
    /// done, from 0 to 1.
    fn denoise(
        &self,
        wanted: &Identity,
        image: &LinearImage,
        amounts: Amounts,
        stop: &(dyn Fn() -> bool + Sync),
        progress: &(dyn Fn(f32) + Sync),
    ) -> Result<LinearImage, String>;
}

/// Run the step: read `input`, run the model on `threads` threads, and write
/// the result into `output_dir`.
pub fn run(
    engine: &dyn Engine,
    spec: &StepSpec,
    input: &Path,
    output_dir: &Path,
    threads: usize,
    stop: &(dyn Fn() -> bool + Sync),
    progress: &(dyn Fn(f32) + Sync),
) -> Result<StepResult, String> {
    let image = super::rc_astro::read_image(input)?;
    // Seiza does its own parallel work in the calling thread's pool.
    let pool = crate::concurrency::ComputePool::take("noise-model", threads)
        .map_err(|error| format!("The noise model could not start its threads: {error}"))?;
    let processed =
        pool.install(|| engine.denoise(&spec.identity, &image, spec.amounts, stop, progress))?;
    super::rc_astro::write_image(&output_dir.join(PROCESSED), &processed)?;
    Ok(StepResult {
        device: Some(spec.identity.backend.clone()),
        warnings: Vec::new(),
    })
}

/// Where this machine's ONNX Runtime and model are, and what it may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// An ONNX Runtime a person installed: its library, or the folder that
    /// contains it. It wins over a downloaded one.
    pub onnxruntime: Option<PathBuf>,
    /// The folder the downloads go to.
    pub folder: PathBuf,
    /// Whether the model may run on a GPU. Off keeps it on the CPU.
    pub use_gpu: bool,
}

impl Config {
    /// The downloads in PSF Guard's data folder, with `onnxruntime` given
    /// when a person set its path.
    pub fn new(onnxruntime: Option<PathBuf>, use_gpu: bool) -> Self {
        Self {
            onnxruntime: onnxruntime.filter(|path| !path.as_os_str().is_empty()),
            folder: default_folder(),
            use_gpu,
        }
    }

    /// The folder the model is installed in.
    pub fn models(&self) -> PathBuf {
        self.folder.join("models")
    }

    /// The folder a downloaded ONNX Runtime is unpacked in.
    pub fn runtime_folder(&self) -> PathBuf {
        self.folder.join("onnxruntime")
    }

    /// ONNX Runtime's library, or what a person does to get one. A path that
    /// was set and leads nowhere is an error; it never falls through to
    /// another library.
    pub fn library(&self) -> Result<PathBuf, String> {
        if let Some(path) = &self.onnxruntime {
            return library_at(path).ok_or_else(|| {
                format!(
                    "The ONNX Runtime path {} is not the library {LIBRARY_FILE} or a folder that contains it",
                    path.display()
                )
            });
        }
        if let Some(path) = std::env::var_os("SEIZA_ONNXRUNTIME").filter(|path| !path.is_empty()) {
            let path = PathBuf::from(path);
            return library_at(&path).ok_or_else(|| {
                format!(
                    "SEIZA_ONNXRUNTIME gives {}, which is not the library {LIBRARY_FILE}",
                    path.display()
                )
            });
        }
        let mut places = vec![self.runtime_folder()];
        // Where a package puts a private copy: beside the executable, or in
        // `../lib/seiza` from it.
        if let Some(directory) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            if let Some(prefix) = directory.parent() {
                places.push(prefix.join("lib").join("seiza"));
            }
            places.push(directory);
        }
        places
            .iter()
            .map(|place| place.join(LIBRARY_FILE))
            .find(|path| path.is_file())
            .ok_or_else(|| "ONNX Runtime is not installed".to_string())
    }

    /// The installed noise model, or why there is none to run.
    pub fn bundle(&self) -> Result<seiza_model::ModelBundle, String> {
        seiza_model::ModelStore::new(self.models())
            .resolve(None)
            .map_err(|error| match error {
                seiza_model::ModelError::NoModelInstalled { .. } => {
                    "The noise model is not installed".to_string()
                }
                error => error.to_string(),
            })
    }
}

/// The library `path` gives: the file itself, or the one in that folder or
/// in its `lib` folder, as ONNX Runtime's release archive has it.
fn library_at(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    [path.join(LIBRARY_FILE), path.join("lib").join(LIBRARY_FILE)]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Where the downloads go when nothing says otherwise: `noise-model` in
/// PSF Guard's data folder on this computer.
pub fn default_folder() -> PathBuf {
    if let Some(folder) = std::env::var_os(FOLDER_VARIABLE).filter(|folder| !folder.is_empty()) {
        return PathBuf::from(folder);
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("psf-guard")
        .join("noise-model")
}

/// What this machine has, for Settings and the worker's `status` command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// ONNX Runtime's library, when one is on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_path: Option<String>,
    /// The version the library reports, once it is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    /// Why there is no usable ONNX Runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_problem: Option<String>,
    /// The installed model's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Why there is no usable model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_problem: Option<String>,
    /// What the next run uses, when both are usable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// Each backend Seiza knows, and why it cannot run here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backends: Vec<BackendLine>,
    /// The folder the downloads go to.
    pub folder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendLine {
    pub backend: String,
    /// Why it cannot run here; absent when it can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// Whether PSF Guard picks it. Seiza picks only backends it has
    /// measured on this kind of hardware.
    pub automatic: bool,
}

/// The model through `seiza-model` and ONNX Runtime.
pub struct SeizaEngine {
    config: Config,
}

impl SeizaEngine {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Self { config })
    }

    /// The Seiza options for `library`. Seiza would otherwise search for
    /// the library itself, and a failed search stays with the process until
    /// it restarts, so PSF Guard gives the path only once the file exists.
    fn options(library: PathBuf) -> seiza_model::BackendOptions {
        seiza_model::BackendOptions {
            library: Some(library),
            ..Default::default()
        }
    }

    /// Open ONNX Runtime at `library`. A process opens the library once: a
    /// library that failed, or another path set later, needs a restart.
    fn open_runtime(library: &Path) -> Result<seiza_model::RuntimeInfo, String> {
        let info =
            seiza_model::runtime_info(&Self::options(library.to_path_buf())).map_err(|error| {
                format!(
                    "{error}. Restart PSF Guard after you replace the library at {}",
                    library.display()
                )
            })?;
        if !same_file(&info.path, library) {
            return Err(format!(
                "PSF Guard already opened ONNX Runtime at {}. Restart PSF Guard to use {}",
                info.path.display(),
                library.display()
            ));
        }
        Ok(info)
    }

    /// The backend the model runs on here: the best Seiza picks for this
    /// machine, or the CPU when the GPU is turned off.
    fn backend(
        &self,
        bundle: &seiza_model::ModelBundle,
        options: &seiza_model::BackendOptions,
    ) -> Result<seiza_model::Backend, String> {
        if self.config.use_gpu {
            seiza_model::Backend::detect(bundle, options).map_err(|error| error.to_string())
        } else {
            Ok(seiza_model::Backend::Cpu)
        }
    }

    fn identity_of(bundle: &seiza_model::ModelBundle, backend: seiza_model::Backend) -> Identity {
        Identity {
            model: bundle.card().name.clone(),
            card_sha256: bundle.card_sha256().to_string(),
            output_version: seiza_model::OUTPUT_VERSION.into(),
            backend: backend.to_string(),
            precision: backend.default_precision().to_string(),
        }
    }

    /// What this machine has. Opens ONNX Runtime when its library is on
    /// disk; touches nothing else.
    pub fn installed(&self) -> Installed {
        let mut installed = Installed {
            folder: self.config.folder.display().to_string(),
            ..Default::default()
        };
        let bundle = match self.config.bundle() {
            Ok(bundle) => {
                installed.model = Some(bundle.card().name.clone());
                Some(bundle)
            }
            Err(problem) => {
                installed.model_problem = Some(problem);
                None
            }
        };
        let library = match self.config.library() {
            Ok(library) => library,
            Err(problem) => {
                installed.runtime_problem = Some(problem);
                return installed;
            }
        };
        installed.runtime_path = Some(library.display().to_string());
        match Self::open_runtime(&library) {
            Ok(info) => installed.runtime_version = Some(info.version),
            Err(problem) => {
                installed.runtime_problem = Some(problem);
                return installed;
            }
        }
        let options = Self::options(library);
        installed.backends = seiza_model::Backend::available(&options)
            .into_iter()
            .map(|status| BackendLine {
                backend: status.backend.to_string(),
                unavailable: status.unavailable,
                automatic: status.automatic,
            })
            .collect();
        if let Some(bundle) = bundle {
            match self.backend(&bundle, &options) {
                Ok(backend) => installed.identity = Some(Self::identity_of(&bundle, backend)),
                Err(problem) => installed.model_problem = Some(problem),
            }
        }
        installed
    }
}

/// Whether two paths are one file, where the system can tell.
fn same_file(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

impl Engine for SeizaEngine {
    fn identity(&self) -> Result<Identity, String> {
        let bundle = self.config.bundle()?;
        let library = self.config.library()?;
        Self::open_runtime(&library)?;
        let backend = self.backend(&bundle, &Self::options(library))?;
        Ok(Self::identity_of(&bundle, backend))
    }

    fn denoise(
        &self,
        wanted: &Identity,
        image: &LinearImage,
        amounts: Amounts,
        stop: &(dyn Fn() -> bool + Sync),
        progress: &(dyn Fn(f32) + Sync),
    ) -> Result<LinearImage, String> {
        let bundle = self.config.bundle()?;
        let library = self.config.library()?;
        Self::open_runtime(&library)?;
        let options = Self::options(library);
        let backend = self.backend(&bundle, &options)?;
        let here = Self::identity_of(&bundle, backend);
        if &here != wanted {
            return Err(format!(
                "This machine runs {}, not the {} the step was planned with. Apply the processing again",
                display_version(&here.version()),
                display_version(&wanted.version()),
            ));
        }
        let mut model = seiza_model::NoiseModel::open(&bundle, backend, &options)
            .map_err(|error| format!("The noise model could not open on {backend}: {error}"))?;
        let view =
            seiza_model::ImageView::new(&image.data, image.width, image.height, image.channels)
                .map_err(|error| error.to_string())?;
        let report = |step: seiza_model::Progress| {
            let done = step.channel * step.tiles + step.tile;
            let all = (step.channels * step.tiles).max(1);
            progress(done as f32 / all as f32);
        };
        let found = model
            .denoise(
                view,
                &seiza_model::DenoiseOptions {
                    progress: Some(&report),
                    cancel: Some(stop),
                },
            )
            .map_err(|error| match error {
                seiza_model::ModelError::Cancelled => "The noise model was stopped".to_string(),
                error => format!("The noise model failed: {error}"),
            })?;
        let data = found
            .blend(&amounts.bands())
            .map_err(|error| error.to_string())?;
        LinearImage::new(image.width, image.height, image.channels, data)
            .map_err(|error| error.to_string())
    }
}

/// A stand-in for tests, which have no ONNX Runtime: it "removes noise" by
/// moving each pixel toward zero by the grain amount.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) struct FixedEngine {
        pub identity: Result<Identity, String>,
        pub runs: AtomicUsize,
    }

    impl FixedEngine {
        pub(crate) fn new(model: &str) -> Arc<Self> {
            Arc::new(Self {
                identity: Ok(identity(model)),
                runs: AtomicUsize::new(0),
            })
        }

        pub(crate) fn missing(problem: &str) -> Arc<Self> {
            Arc::new(Self {
                identity: Err(problem.into()),
                runs: AtomicUsize::new(0),
            })
        }
    }

    pub(crate) fn identity(model: &str) -> Identity {
        Identity {
            model: model.into(),
            card_sha256: "c".repeat(64),
            output_version: "0.1.0".into(),
            backend: "cpu".into(),
            precision: "f32".into(),
        }
    }

    impl Engine for FixedEngine {
        fn identity(&self) -> Result<Identity, String> {
            self.identity.clone()
        }

        fn denoise(
            &self,
            wanted: &Identity,
            image: &LinearImage,
            amounts: Amounts,
            stop: &(dyn Fn() -> bool + Sync),
            progress: &(dyn Fn(f32) + Sync),
        ) -> Result<LinearImage, String> {
            if self.identity.as_ref() != Ok(wanted) {
                return Err("This machine runs another model".into());
            }
            if stop() {
                return Err("The noise model was stopped".into());
            }
            self.runs.fetch_add(1, Ordering::SeqCst);
            progress(1.0);
            let mut output = image.clone();
            for sample in &mut output.data {
                *sample *= 1.0 - amounts.grain;
            }
            Ok(output)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(parameters: &[(&str, ExternalParameterValue)]) -> ExternalToolRequest {
        ExternalToolRequest {
            tool: TOOL.into(),
            parameters: parameters
                .iter()
                .map(|(name, value)| (name.to_string(), *value))
                .collect(),
            device: None,
        }
    }

    #[test]
    fn a_step_keeps_the_schema_s_defaults_for_what_it_leaves_out() {
        assert_eq!(
            Amounts::from_request(&request(&[])).unwrap(),
            Amounts {
                grain: 0.7,
                mottle: 0.9
            }
        );
        assert_eq!(
            Amounts::from_request(&request(&[
                (GRAIN, ExternalParameterValue::Float(0.25)),
                (MOTTLE, ExternalParameterValue::Int(1)),
            ]))
            .unwrap(),
            Amounts {
                grain: 0.25,
                mottle: 1.0
            }
        );
        let refused =
            Amounts::from_request(&request(&[(GRAIN, ExternalParameterValue::Float(1.5))]))
                .unwrap_err();
        assert!(refused.contains("outside"), "{refused}");
    }

    #[test]
    fn the_schema_version_carries_every_part_of_the_identity() {
        let identity = testing::identity("noise-v1-d33");
        let version = identity.version();
        for (part, changed) in [
            (
                "model",
                Identity {
                    model: "noise-v2".into(),
                    ..identity.clone()
                },
            ),
            (
                "card",
                Identity {
                    card_sha256: "d".repeat(64),
                    ..identity.clone()
                },
            ),
            (
                "output version",
                Identity {
                    output_version: "0.2.0".into(),
                    ..identity.clone()
                },
            ),
            (
                "backend",
                Identity {
                    backend: "coreml-gpu".into(),
                    ..identity.clone()
                },
            ),
            (
                "precision",
                Identity {
                    precision: "f16".into(),
                    ..identity.clone()
                },
            ),
        ] {
            assert_ne!(changed.version(), version, "{part}");
        }
        assert_eq!(identity.schema().cli_version, version);
        assert_eq!(
            display_version(&version),
            "noise-v1-d33 cpu f32, seiza-model 0.1.0, band scale 5"
        );
        let schema = identity.schema();
        assert_eq!(schema.key, TOOL);
        assert!(schema.licensed);
        assert_eq!(
            schema
                .parameters
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>(),
            [GRAIN, MOTTLE]
        );
    }

    #[test]
    fn a_set_path_is_the_library_or_its_folder_and_never_falls_through() {
        let folder = tempfile::tempdir().unwrap();
        let config = |onnxruntime: Option<PathBuf>| Config {
            onnxruntime,
            folder: folder.path().join("downloads"),
            use_gpu: true,
        };
        // Nothing set and nothing downloaded.
        let missing = config(None).library();
        if std::env::var_os("SEIZA_ONNXRUNTIME").is_none() {
            assert_eq!(missing.unwrap_err(), "ONNX Runtime is not installed");
        }
        // A release archive unpacks to `lib/<library>`.
        let lib = folder.path().join("onnxruntime-1.0").join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join(LIBRARY_FILE), b"library").unwrap();
        for given in [
            lib.join(LIBRARY_FILE),
            lib.clone(),
            lib.parent().unwrap().to_path_buf(),
        ] {
            assert_eq!(
                config(Some(given.clone())).library().unwrap(),
                lib.join(LIBRARY_FILE),
                "{}",
                given.display()
            );
        }
        // A downloaded copy does not stand in for a set path that is wrong.
        let downloaded = config(None).runtime_folder();
        std::fs::create_dir_all(&downloaded).unwrap();
        std::fs::write(downloaded.join(LIBRARY_FILE), b"library").unwrap();
        let wrong = config(Some(folder.path().join("nowhere")))
            .library()
            .unwrap_err();
        assert!(wrong.contains("nowhere"), "{wrong}");
        if std::env::var_os("SEIZA_ONNXRUNTIME").is_none() {
            assert_eq!(
                config(None).library().unwrap(),
                downloaded.join(LIBRARY_FILE)
            );
        }
    }

    #[test]
    fn a_machine_with_no_model_reports_it_without_opening_onnx_runtime() {
        let folder = tempfile::tempdir().unwrap();
        let engine = SeizaEngine::new(Config {
            onnxruntime: None,
            folder: folder.path().to_path_buf(),
            use_gpu: true,
        });
        assert_eq!(
            engine.identity().unwrap_err(),
            "The noise model is not installed"
        );
        let installed = engine.installed();
        assert_eq!(installed.model, None);
        assert_eq!(
            installed.model_problem.as_deref(),
            Some("The noise model is not installed")
        );
        assert_eq!(installed.identity, None);
        assert_eq!(installed.runtime_version, None);
    }

    #[test]
    fn the_task_writes_what_the_engine_returns() {
        let folder = tempfile::tempdir().unwrap();
        let input = folder.path().join(INPUT);
        let image = LinearImage::new(2, 2, 1, vec![1.0, 2.0, f32::NAN, 4.0]).unwrap();
        crate::remote_work::rc_astro::write_image(&input, &image).unwrap();
        let engine = testing::FixedEngine::new("noise-v1");
        let spec = StepSpec {
            identity: testing::identity("noise-v1"),
            amounts: Amounts {
                grain: 0.5,
                mottle: 0.5,
            },
            person: true,
        };
        let result = run(
            engine.as_ref(),
            &spec,
            &input,
            folder.path(),
            2,
            &|| false,
            &|_| {},
        )
        .unwrap();
        assert_eq!(result.device.as_deref(), Some("cpu"));
        let written =
            crate::remote_work::rc_astro::read_image(&folder.path().join(PROCESSED)).unwrap();
        assert_eq!(written.data[0], 0.5);
        assert_eq!(written.data[3], 2.0);
        assert!(written.data[2].is_nan());

        // Another machine's model never runs under this identity.
        let other = StepSpec {
            identity: testing::identity("noise-v2"),
            ..spec
        };
        assert!(run(
            engine.as_ref(),
            &other,
            &input,
            folder.path(),
            1,
            &|| false,
            &|_| {}
        )
        .is_err());
    }

    /// A real run through the worker task, for a computer that has the
    /// downloads: set `PSF_GUARD_NOISE_MODEL_DIR` to their folder and
    /// `PSF_GUARD_NOISE_MODEL_REAL_INPUT` to a linear stack, then run
    /// `cargo test the_real_model -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs ONNX Runtime, the model and a real stack"]
    fn the_real_model_denoises_a_real_stack() {
        let input = PathBuf::from(
            std::env::var_os("PSF_GUARD_NOISE_MODEL_REAL_INPUT")
                .expect("set PSF_GUARD_NOISE_MODEL_REAL_INPUT to a linear stack"),
        );
        let engine = SeizaEngine::new(Config::new(None, true));
        let identity = engine.identity().unwrap();
        println!("runs {}", display_version(&identity.version()));
        let folder = tempfile::tempdir().unwrap();
        let before = crate::remote_work::rc_astro::read_image(&input).unwrap();
        let task_input = folder.path().join(INPUT);
        crate::remote_work::rc_astro::write_image(&task_input, &before).unwrap();
        let spec = StepSpec {
            identity,
            amounts: Amounts {
                grain: 0.7,
                mottle: 0.9,
            },
            person: true,
        };
        struct Quiet;
        impl crate::remote_work::TaskReport for Quiet {
            fn fraction(&mut self, _: f32) {}
            fn event(&mut self, _: serde_json::Value) {}
        }
        let started = std::time::Instant::now();
        crate::remote_work::execute(
            crate::remote_work::protocol::TaskKind::NoiseModelStep,
            serde_json::to_value(&spec).unwrap(),
            &std::collections::BTreeMap::from([(INPUT.to_string(), task_input)]),
            folder.path(),
            folder.path(),
            &crate::remote_work::LocalTools {
                noise_model: Some(engine),
                slots: 1,
                ..Default::default()
            },
            &crate::concurrency::Stop::new(),
            &mut Quiet,
        )
        .unwrap();
        println!(
            "{}×{}×{} in {:.1} s",
            before.width,
            before.height,
            before.channels,
            started.elapsed().as_secs_f64()
        );
        let after =
            crate::remote_work::rc_astro::read_image(&folder.path().join(PROCESSED)).unwrap();
        // The noise as the spread of each pixel's difference from its
        // right-hand neighbour.
        let spread = |image: &LinearImage| {
            let mut differences: Vec<f32> = image
                .data
                .chunks_exact(image.channels)
                .zip(image.data.chunks_exact(image.channels).skip(1))
                .step_by(97)
                .map(|(left, right)| (left[0] - right[0]).abs())
                .filter(|difference| difference.is_finite())
                .collect();
            differences.sort_by(f32::total_cmp);
            differences[differences.len() / 2]
        };
        let (noise_before, noise_after) = (spread(&before), spread(&after));
        println!("median neighbour difference {noise_before} before, {noise_after} after");
        assert_eq!(
            (after.width, after.height, after.channels),
            (before.width, before.height, before.channels)
        );
        assert!(noise_after < noise_before);
        if let Some(keep) = std::env::var_os("PSF_GUARD_NOISE_MODEL_REAL_OUTPUT") {
            std::fs::copy(folder.path().join(PROCESSED), keep).unwrap();
        }
    }
}
