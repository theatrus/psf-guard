//! `psf-guard noise-model`: download Seiza's noise model and ONNX Runtime
//! to this computer, or show what is installed. A worker computer has no
//! Settings page, so this command is how it gets the model; a server can
//! use it too.

use anyhow::{anyhow, Result};
use std::io::Write;
use std::path::PathBuf;

use crate::cli::NoiseModelAction;
use crate::remote_work::noise_model::{install, Config, SeizaEngine};

pub fn run(action: NoiseModelAction) -> Result<()> {
    match action {
        NoiseModelAction::Download {
            skip_runtime,
            skip_model,
        } => download(!skip_runtime, !skip_model),
        NoiseModelAction::Status { onnxruntime } => {
            status(&Config::new(onnxruntime.map(PathBuf::from), true));
            Ok(())
        }
    }
}

fn download(runtime: bool, model: bool) -> Result<()> {
    let config = Config::new(None, true);
    println!("Downloading to {}", config.folder.display());
    let report = |step: install::Step| {
        match (step.bytes_done, step.bytes_total) {
            (Some(done), Some(total)) if total > 0 => print!(
                "\r{} {:.0} of {:.0} MB",
                step.message,
                done as f64 / 1_048_576.0,
                total as f64 / 1_048_576.0
            ),
            _ => print!("\n{}", step.message),
        }
        let _ = std::io::stdout().flush();
    };
    let ended = tokio::runtime::Runtime::new()?.block_on(async {
        if runtime {
            let library = install::download_runtime(&config, &report).await?;
            println!("\nONNX Runtime is at {}", library.display());
        }
        if model {
            let name = install::download_model(&config, &report).await?;
            println!("\nInstalled the noise model {name}");
        }
        Ok::<(), String>(())
    });
    println!();
    ended.map_err(|error| anyhow!(error))?;
    status(&config);
    println!("Restart the worker, or the server, on this computer to offer the noise model.");
    Ok(())
}

/// Print what is installed. Opens ONNX Runtime when its library is on disk.
fn status(config: &Config) {
    let installed = SeizaEngine::new(config.clone()).installed();
    println!("Folder: {}", installed.folder);
    match (
        &installed.runtime_version,
        &installed.runtime_path,
        &installed.runtime_problem,
    ) {
        (Some(version), Some(path), _) => println!("ONNX Runtime {version} at {path}"),
        (_, _, Some(problem)) => println!("ONNX Runtime: {problem}"),
        _ => println!("ONNX Runtime: not installed"),
    }
    match (&installed.model, &installed.model_problem) {
        (Some(model), None) => println!("Model: {model}"),
        (Some(model), Some(problem)) => println!("Model: {model} ({problem})"),
        (None, problem) => println!("Model: {}", problem.as_deref().unwrap_or("not installed")),
    }
    for line in &installed.backends {
        match (&line.unavailable, line.automatic) {
            (None, true) => println!("  {:<14} available", line.backend),
            (None, false) => println!(
                "  {:<14} available, but not used: Seiza has not measured it on this hardware",
                line.backend
            ),
            (Some(reason), _) => println!("  {:<14} {reason}", line.backend),
        }
    }
    match &installed.identity {
        Some(identity) => println!(
            "This computer runs {}",
            crate::remote_work::noise_model::display_version(&identity.version())
        ),
        None => println!(
            "This computer cannot run the noise model yet. Run `psf-guard noise-model download`."
        ),
    }
}
