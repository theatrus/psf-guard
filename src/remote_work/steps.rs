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

/// Whether a provider is one of RC-Astro's tools.
pub fn is_rc_astro(key: &str) -> bool {
    provider(key).is_some_and(|found| found.engine == Engine::RcAstro)
}

thread_local! {
    /// Whether this thread is encoding a value for an identity.
    static FOR_IDENTITY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a step being serialized on this thread is part of an identity.
pub fn encoding_identity() -> bool {
    FOR_IDENTITY.with(std::cell::Cell::get)
}

/// `value` as JSON for a cache or job identity. A step in it is encoded as
/// it was before providers, `{"tool", "parameters"}`, so every identity
/// made then is the one made now: a color job, a stretch, an emission
/// input. Use this, not `serde_json::to_vec`, wherever the bytes are hashed.
pub fn identity_json<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<Vec<u8>> {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            FOR_IDENTITY.with(|flag| flag.set(self.0));
        }
    }
    let _reset = Reset(FOR_IDENTITY.with(|flag| flag.replace(true)));
    serde_json::to_vec(value)
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

thread_local! {
    /// The threads the job on this thread leased, while it waits on a step.
    static HELD_THREADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Says, for as long as it lives, that the job on this thread holds
/// `threads` leased workers.
pub struct HeldThreads(usize);

/// Record the workers a job leased, so a step it waits on runs on them
/// rather than leasing again from a budget the job has already taken.
pub fn hold_threads(threads: usize) -> HeldThreads {
    HeldThreads(HELD_THREADS.with(|held| held.replace(threads)))
}

impl Drop for HeldThreads {
    fn drop(&mut self) {
        HELD_THREADS.with(|held| held.set(self.0));
    }
}

/// The threads the job waiting on this thread holds: its pool's size when
/// the step is applied inside the job's `ComputePool`, else what
/// [`hold_threads`] recorded, else 0. A job waits while its step runs, so
/// the step may use them.
pub fn held_threads() -> usize {
    if rayon::current_thread_index().is_some() {
        rayon::current_num_threads()
    } else {
        HELD_THREADS.with(std::cell::Cell::get)
    }
}

/// What fixes a provider no machine offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Remedy {
    /// Install RC-Astro's `rc-astro` CLI.
    InstallRcAstro,
    /// License the tool with RC-Astro on a machine that has it.
    License,
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
    /// with `schema`. `threads` is how many the waiting job holds, or 0
    /// when it holds none: see [`held_threads`].
    fn payload(
        &self,
        schema: &seiza_stacking::ExternalToolSchema,
        request: &seiza_stacking::ExternalToolRequest,
        headers: &[(String, seiza_fits::HeaderValue)],
        person: bool,
        threads: usize,
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
