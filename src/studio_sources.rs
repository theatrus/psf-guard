//! A PSF-Studio document's sources: PSF Guard's channel stacks.
//!
//! PSF Guard stacks each channel. Studio starts from those stacks and does
//! the color processing itself. This module lists a target's channel stacks
//! on a server, downloads them, and registers them onto one grid with
//! Seiza, which is the first thing every later step needs.

use crate::color_steps::{run_tool_chain, ChannelDeconvolution, ToolChain, ToolEvent};
use crate::server::stack_preview::color::StackColorSource;
use crate::server::stack_preview::rc_astro::{ProcessingChain, ProcessingStep};
use crate::server::workers::rc_astro::RcAstroRunner;
use crate::server::workers::Workers;
use anyhow::{anyhow, bail, Context, Result};
use seiza_stacking::LinearImage;
use serde::de::DeserializeOwned;
use std::path::Path;

// Registering channel stacks and running a document's Linear steps on them
// is shared with PSF Studio.
pub use psf_studio_core::channels::{
    average, line_channel, seam_spacing, shift_mirrored, shifted_passes, unshift, ChainResult,
    ChainRun, ChannelContinuum, ChannelPlacement, ChannelRegistration, ChannelState, LinearProgram,
    LinearStep, LinearTools, Pass, RegisteredChannels, SHIFTED_PASS,
};

/// One database on a PSF Guard server.
pub struct Server {
    client: reqwest::Client,
    /// `http://host:port/api/db/<database>`.
    database_url: String,
    token: Option<String>,
}

impl Server {
    pub fn new(origin: &str, database: &str, token: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            database_url: format!("{}/api/db/{database}", origin.trim_end_matches('/')),
            token,
        }
    }

    fn get(&self, url: String) -> reqwest::RequestBuilder {
        let request = self.client.get(url);
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// The plate solve of one frame: the one the server has, or a new one
    /// when it has none. `None` when the frame cannot be solved; the reason
    /// is the server's.
    pub async fn frame_solve(
        &self,
        image_id: i32,
    ) -> Result<Option<crate::astrometry::AstrometrySolutionResponse>> {
        let url = format!("{}/images/{image_id}/astrometry", self.database_url);
        let known: serde_json::Value = self.data(url.clone()).await?;
        // Only a solve of the pixels shows where the stars are; a WCS from
        // the file's header may describe other pixels.
        let pixel_solve = !known["solution"].is_null() && known["mode"] != "embedded_wcs";
        let analysis = if !pixel_solve {
            let request = self.client.post(url);
            let request = match &self.token {
                Some(token) => request.bearer_auth(token),
                None => request,
            };
            let response = request.send().await?;
            let status = response.status();
            let body: serde_json::Value = response
                .json()
                .await
                .with_context(|| format!("reading the server's reply ({status})"))?;
            if !status.is_success() {
                let reason = body["error"].as_str().unwrap_or("no reason given");
                bail!("the server could not solve the frame ({status}): {reason}");
            }
            body["data"].clone()
        } else {
            known
        };
        if analysis["solution"].is_null() {
            return Ok(None);
        }
        serde_json::from_value(analysis["solution"].clone())
            .map(Some)
            .context("reading the frame's plate solve")
    }

    /// Reads the `data` of the server's reply to a GET.
    async fn data<T: DeserializeOwned>(&self, url: String) -> Result<T> {
        let response = self.get(url).send().await?;
        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .with_context(|| format!("reading the server's reply ({status})"))?;
        if !status.is_success() {
            let reason = body["error"].as_str().unwrap_or("no reason given");
            bail!("the server refused the request ({status}): {reason}");
        }
        serde_json::from_value(body["data"].clone()).context("reading the server's reply")
    }

    /// A target's name and its current channel stacks, one per channel, in
    /// the server's order. A channel with several current stacks is
    /// refused: the caller must say which stack it means.
    pub async fn channel_stacks(
        &self,
        project_id: i32,
        target_id: i32,
    ) -> Result<(String, Vec<StackColorSource>)> {
        let catalog: serde_json::Value = self
            .data(format!(
                "{}/projects/{project_id}/stack-previews/color",
                self.database_url
            ))
            .await?;
        let target = catalog["targets"]
            .as_array()
            .and_then(|targets| {
                targets
                    .iter()
                    .find(|target| target["target_id"] == target_id)
            })
            .ok_or_else(|| {
                anyhow!("project {project_id} has no channel stacks for target {target_id}")
            })?;
        let stacks: Vec<StackColorSource> =
            serde_json::from_value(target["source_candidates"].clone())
                .context("reading the target's channel stacks")?;
        let mut one_each: Vec<StackColorSource> = Vec::new();
        for stack in stacks {
            if one_each.iter().any(|kept| kept.role == stack.role) {
                bail!(
                    "target {target_id} has more than one current {} stack; choosing between them is not built yet",
                    stack.filter_name
                );
            }
            one_each.push(stack);
        }
        let name = target["target_name"]
            .as_str()
            .map_or_else(|| format!("Target {target_id}"), str::to_string);
        Ok((name, one_each))
    }

    /// Downloads a channel stack's FITS to `path`, a piece at a time.
    pub async fn download_stack(&self, stack: &StackColorSource, path: &Path) -> Result<u64> {
        self.download(stack, "fits", path)
            .await?
            .ok_or_else(|| anyhow!("the server has no {} stack", stack.filter_name))
    }

    /// Downloads the map of where a stack's frames clipped, which the
    /// stacker recorded. `None` when the stack has none.
    pub async fn download_clipped(
        &self,
        stack: &StackColorSource,
        path: &Path,
    ) -> Result<Option<u64>> {
        self.download(stack, "clipped", path).await
    }

    /// Downloads one file of a stack to `path` through a partial file.
    /// `None` when the server has no such file.
    async fn download(
        &self,
        stack: &StackColorSource,
        file: &str,
        path: &Path,
    ) -> Result<Option<u64>> {
        use std::io::Write;
        let url = format!(
            "{}/stack-previews/{}/{}/{file}?v={}",
            self.database_url, stack.job_id, stack.group_index, stack.artifact_revision
        );
        let response = self.get(url).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND && file != "fits" {
            let _ = std::fs::remove_file(path);
            return Ok(None);
        }
        let mut response = response
            .error_for_status()
            .with_context(|| format!("downloading the {} stack", stack.filter_name))?;
        let partial = path.with_extension("fits.part");
        let mut output = std::fs::File::create(&partial)
            .with_context(|| format!("creating {}", partial.display()))?;
        let mut bytes = 0;
        while let Some(chunk) = response.chunk().await? {
            output.write_all(&chunk)?;
            bytes += chunk.len() as u64;
        }
        output.sync_all()?;
        std::fs::rename(&partial, path).with_context(|| format!("writing {}", path.display()))?;
        Ok(Some(bytes))
    }
}

/// Runs a document's tools on PSF Guard's workers, and keeps each result
/// under `cache`.
pub struct WorkerTools<'a> {
    /// This machine's workers, and any connected to it.
    pub workers: &'a Workers,
    /// Where each step's result is kept, so a repeat run is fast.
    pub cache: &'a Path,
}

impl LinearTools for WorkerTools<'_> {
    type Tool = ProcessingStep;

    fn extend_chain(
        &self,
        chain: &[ProcessingStep],
        tool: &ProcessingStep,
    ) -> Option<Vec<ProcessingStep>> {
        extend_chain(chain, tool)
    }

    fn name(&self, tool: &ProcessingStep) -> String {
        tool.provider.clone()
    }

    fn run_chain(
        &self,
        run: &ChainRun<'_, ProcessingStep>,
        progress: &dyn Fn(&str, f32),
    ) -> Result<ChainResult> {
        let resolved = ToolChain::resolve(
            self.workers,
            &ProcessingChain {
                steps: run.chain.to_vec(),
            },
            "studio",
            run.source,
            run.revision,
            run.deconvolution,
        )
        .map_err(|error| anyhow!(error))?;
        let outcome = run_tool_chain(
            RcAstroRunner {
                workers: self.workers,
                person: true,
            },
            self.cache,
            &resolved,
            run.image,
            run.headers,
            None,
            |ToolEvent::Running { label, fraction }| progress(label, fraction),
            &mut || Ok(()),
        )
        .map_err(|error| anyhow!(error))?;
        Ok(ChainResult {
            image: outcome.image,
            stars: outcome.stars,
            ids: resolved.ids,
        })
    }

    fn deconvolve(
        &self,
        config: seiza_deconvolution::DeconvolutionConfig,
        source: &str,
        revision: &str,
        frame: &seiza_stacking::FitsFrame,
    ) -> Result<(LinearImage, String)> {
        let deconvolution = ChannelDeconvolution::resolve(config, "studio", source, revision)
            .map_err(|error| anyhow!(error))?;
        let (restored, _) = deconvolution
            .apply(self.cache, frame)
            .map_err(|error| anyhow!(error))?;
        Ok((restored, deconvolution.id))
    }
}

/// `chain` with `tool` after it, when one chain can run them all: each
/// tool does something later in a chain's order than the one before it,
/// as a chain runs its tools in that order.
fn extend_chain(chain: &[ProcessingStep], tool: &ProcessingStep) -> Option<Vec<ProcessingStep>> {
    let rank = |step: &ProcessingStep| {
        step.category().and_then(|category| {
            crate::remote_work::steps::Category::ALL
                .iter()
                .position(|other| *other == category)
        })
    };
    let fits = match chain.last() {
        None => true,
        Some(last) => matches!((rank(last), rank(tool)), (Some(a), Some(b)) if b > a),
    };
    fits.then(|| {
        let mut chain = chain.to_vec();
        chain.push(tool.clone());
        chain
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn tool(provider: &str) -> ProcessingStep {
        ProcessingStep {
            provider: provider.to_string(),
            parameters: BTreeMap::new(),
        }
    }

    #[test]
    fn tools_in_a_chains_order_run_as_one_chain() {
        let grow = |chain: &[ProcessingStep], next: &str| -> Option<Vec<String>> {
            extend_chain(chain, &tool(next))
                .map(|chain| chain.into_iter().map(|step| step.provider).collect())
        };
        let chain = vec![tool("bxt"), tool("nxt")];
        assert_eq!(grow(&[], "nxt").unwrap(), ["nxt"]);
        assert_eq!(grow(&chain, "sxt").unwrap(), ["bxt", "nxt", "sxt"]);
        // A chain runs its tools in its own order, and one tool of a kind,
        // so noise reduction then sharpening, or a second noise reduction,
        // starts a chain of its own.
        assert!(grow(&[tool("nxt")], "bxt").is_none());
        assert!(grow(&[tool("nxt")], "noise-model").is_none());
        assert!(grow(&[tool("unknown")], "sxt").is_none());
    }
}
