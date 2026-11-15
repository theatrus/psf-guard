//! The processing steps a stack's linear image can go through before its
//! stretch, by category and provider.
//!
//! A category is what a step does: sharpen, reduce noise, remove stars. A
//! provider is what does it: an RC-Astro tool, or one of Seiza's models. A
//! chain runs the categories in a fixed order with at most one provider in
//! each, so a new model is one more provider here and no new flow: the
//! chain's cache, the worker task, the capabilities answer and the controls
//! all read this list.

use serde::{Deserialize, Serialize};

/// What a step does. The order here is the order a chain runs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Sharpen,
    Denoise,
    StarRemoval,
}

impl Category {
    pub const ALL: [Category; 3] = [Self::Sharpen, Self::Denoise, Self::StarRemoval];

    /// The category as a person reads it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Sharpen => "Sharpening",
            Self::Denoise => "Noise reduction",
            Self::StarRemoval => "Star removal",
        }
    }
}

/// What runs a provider's steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// RC-Astro's standalone `rc-astro` CLI; the provider's key is the
    /// subcommand.
    RcAstro,
    /// Seiza's noise model, through ONNX Runtime.
    SeizaNoiseModel,
}

/// One provider of one category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    /// The provider's key in a stored step, an offer and a task. The
    /// RC-Astro keys are the tool names stored steps have always used.
    pub key: &'static str,
    pub category: Category,
    /// The provider's name, for a message before any machine has reported
    /// its own.
    pub name: &'static str,
    pub engine: Engine,
}

/// Every provider, in the order a chain runs their categories and a picker
/// lists them.
pub const PROVIDERS: [Provider; 4] = [
    Provider {
        key: "bxt",
        category: Category::Sharpen,
        name: "RC-Astro BlurXTerminator",
        engine: Engine::RcAstro,
    },
    Provider {
        key: "nxt",
        category: Category::Denoise,
        name: "RC-Astro NoiseXTerminator",
        engine: Engine::RcAstro,
    },
    Provider {
        key: super::noise_model::TOOL,
        category: Category::Denoise,
        name: super::noise_model::NAME,
        engine: Engine::SeizaNoiseModel,
    },
    Provider {
        key: "sxt",
        category: Category::StarRemoval,
        name: "RC-Astro StarXTerminator",
        engine: Engine::RcAstro,
    },
];

/// The provider with this key.
pub fn provider(key: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|provider| provider.key == key)
}

/// The category a provider key belongs to.
pub fn category_of(key: &str) -> Option<Category> {
    provider(key).map(|provider| provider.category)
}

/// One provider a worker can run: its schema gives the name, the version
/// that identifies its output, whether it is licensed, and its parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepOffer {
    pub category: Category,
    pub provider: String,
    pub schema: seiza_stacking::ExternalToolSchema,
}

impl StepOffer {
    /// The offer for a schema, when its key is a provider this build knows.
    pub fn of(schema: seiza_stacking::ExternalToolSchema) -> Option<Self> {
        Some(Self {
            category: category_of(&schema.key)?,
            provider: schema.key.clone(),
            schema,
        })
    }
}

/// The offers a registration makes: its `steps`, or, from a worker older
/// than providers, its RC-Astro tools read as offers.
pub fn offers_of(offers: &super::protocol::Offers) -> Vec<StepOffer> {
    if !offers.steps.is_empty() {
        return offers.steps.clone();
    }
    if !offers
        .work
        .contains(&super::protocol::TaskKind::RcAstroStep)
    {
        return Vec::new();
    }
    offers
        .rc_astro
        .iter()
        .cloned()
        .filter_map(StepOffer::of)
        .collect()
}

/// What fixes a provider no machine offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Remedy {
    /// Install RC-Astro's `rc-astro` CLI and license the tool.
    InstallRcAstro,
    /// Download ONNX Runtime and the noise model in Settings.
    DownloadNoiseModel,
}

/// What a machine needs to run a step, handed to its runner.
pub struct RunContext<'a> {
    pub inputs: &'a std::collections::BTreeMap<String, std::path::PathBuf>,
    pub output_dir: &'a std::path::Path,
    pub tools: &'a super::LocalTools,
    pub stop: &'a crate::concurrency::Stop,
    pub report: &'a mut dyn super::TaskReport,
}

/// What every engine does, so a chain, a task and a worker treat every
/// provider alike. An engine with a new model implements this and lists its
/// providers in [`PROVIDERS`].
pub trait StepRunner: Sync {
    /// The schemas of this engine's providers this machine can run now, and
    /// the last problem reading one. A schema is the provider's contract:
    /// its name, the version that identifies its output, whether it is
    /// licensed, and the parameters a step may set, with their ranges.
    fn offers(
        &self,
        tools: &super::LocalTools,
    ) -> (Vec<seiza_stacking::ExternalToolSchema>, Option<String>);

    /// What the server sends a worker with the image, for a step planned
    /// with `schema`.
    fn payload(
        &self,
        schema: &seiza_stacking::ExternalToolSchema,
        request: &seiza_stacking::ExternalToolRequest,
        headers: &[(String, seiza_fits::HeaderValue)],
        person: bool,
    ) -> Result<serde_json::Value, String>;

    /// The files a run writes, and whether the task fails without each.
    fn outputs(&self) -> Vec<(String, bool)>;

    /// Whether the engine reads an image with its uncovered pixels left as
    /// they are. Otherwise the chain fills them before the run.
    fn reads_holes(&self) -> bool;

    /// Run one step on this machine: read `input.fits`, write the outputs.
    fn run(
        &self,
        payload: serde_json::Value,
        context: RunContext<'_>,
    ) -> Result<serde_json::Value, String>;

    /// Why no machine offers a provider of this engine, with what a person
    /// does about it, and the kind of action for the controls.
    fn missing(&self) -> (&'static str, Remedy);
}

/// The runner of an engine's providers.
pub fn runner(engine: Engine) -> &'static dyn StepRunner {
    match engine {
        Engine::RcAstro => &super::rc_astro::RcAstroEngine,
        Engine::SeizaNoiseModel => &super::noise_model::SeizaNoiseEngine,
    }
}

/// A processing step task's spec: which provider runs, and what its engine
/// needs, side by side. An RC-Astro provider's fields are the ones an
/// RC-Astro task has always had, so a worker older than providers reads the
/// same spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepSpec {
    pub category: Category,
    pub provider: String,
    #[serde(flatten)]
    pub payload: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_list_one_category_after_another_in_run_order() {
        let categories: Vec<Category> = PROVIDERS.iter().map(|found| found.category).collect();
        let mut sorted = categories.clone();
        sorted.sort();
        assert_eq!(categories, sorted);
        assert_eq!(category_of("nxt"), Some(Category::Denoise));
        assert_eq!(category_of("noise-model"), Some(Category::Denoise));
        assert_eq!(category_of("gxt"), None);
        // The stored spelling of a category.
        assert_eq!(
            serde_json::to_string(&Category::StarRemoval).unwrap(),
            "\"star_removal\""
        );
    }
}
