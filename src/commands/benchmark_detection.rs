//! Batch star-detection benchmark over a frame manifest.
//!
//! Runs the HocusFocus detector over every frame in a JSON manifest,
//! recording star count, HFR statistics, and wall time per frame, and — when
//! the manifest carries them — N.I.N.A.'s own DetectedStars/HFR for the same
//! frame as ground truth. Detector variants (structure removal, detection
//! binning, saturated-star policy) are selected by flags so before/after
//! comparisons run the same code path users run.

use crate::hocus_focus_star_detection::{
    detect_stars_hocus_focus, HocusFocusParams, StructureRemovalMethod, TelescopeClass,
};
use crate::image_analysis::FitsImage;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

/// One frame to benchmark. `nina_*` fields are optional ground truth.
#[derive(Debug, Deserialize)]
struct ManifestEntry {
    path: String,
    #[serde(default)]
    telescope: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    grading_status: Option<i64>,
    #[serde(default)]
    nina_stars: Option<i64>,
    #[serde(default)]
    nina_hfr: Option<f64>,
}

#[allow(clippy::too_many_arguments)]
pub fn benchmark_detection(
    manifest_path: &str,
    output_path: &str,
    structure: &str,
    binning: usize,
    keep_saturated: bool,
    psf_type: &str,
    runs: usize,
    noise_reduction: Option<usize>,
    preset: &str,
) -> Result<()> {
    let manifest_text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("reading manifest {manifest_path}"))?;
    let entries: Vec<ManifestEntry> =
        serde_json::from_str(&manifest_text).context("parsing manifest JSON")?;

    let structure_removal = match structure {
        "filtered" => StructureRemovalMethod::Filtered,
        "atrous" => StructureRemovalMethod::Atrous,
        other => anyhow::bail!("unknown structure removal {other:?} (filtered|atrous)"),
    };
    enum PresetMode {
        Fixed,
        Class(TelescopeClass),
        Auto,
    }
    let preset_mode = match preset {
        "fixed" => PresetMode::Fixed,
        "wide" => PresetMode::Class(TelescopeClass::WideField),
        "standard" => PresetMode::Class(TelescopeClass::Standard),
        "long" => PresetMode::Class(TelescopeClass::LongFocalLength),
        "auto" => PresetMode::Auto,
        other => anyhow::bail!("unknown preset {other:?} (fixed|auto|wide|standard|long)"),
    };

    // Preset base first, explicit flags on top, per frame.
    let apply_flags = |mut params: HocusFocusParams| -> HocusFocusParams {
        params.structure_removal = structure_removal;
        params.detection_binning = binning.max(1);
        params.keep_saturated_stars = keep_saturated;
        params.psf_type = psf_type
            .parse()
            .unwrap_or(crate::psf_fitting::PSFType::None);
        if let Some(radius) = noise_reduction {
            params.noise_reduction_radius = radius;
        }
        params
    };
    let params_for_frame = |path: &Path| -> (HocusFocusParams, &'static str) {
        let class = match &preset_mode {
            PresetMode::Fixed => return (apply_flags(HocusFocusParams::default()), "fixed"),
            PresetMode::Class(class) => *class,
            PresetMode::Auto => HocusFocusParams::for_frame_path(path).1,
        };
        let label = match class {
            TelescopeClass::WideField => "wide",
            TelescopeClass::Standard => "standard",
            TelescopeClass::LongFocalLength => "long",
        };
        (
            apply_flags(HocusFocusParams::for_telescope_class(class)),
            label,
        )
    };

    let mut output =
        std::fs::File::create(output_path).with_context(|| format!("creating {output_path}"))?;
    writeln!(
        output,
        "telescope,target,filter,grading_status,path,width,height,\
         nina_stars,nina_hfr,det_stars,det_saturated,det_hfr,det_hfr_std,\
         load_ms,detect_ms,structure,binning,keep_saturated,preset_class"
    )?;

    let mut detect_times = Vec::new();
    let mut count_ratios = Vec::new();
    let mut hfr_deltas = Vec::new();

    for (index, entry) in entries.iter().enumerate() {
        let path = Path::new(&entry.path);
        let load_start = Instant::now();
        let fits = match FitsImage::from_file(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("SKIP {}: {e}", entry.path);
                continue;
            }
        };
        let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;
        let (params, preset_class) = params_for_frame(path);

        // Median-of-N detect wall time; the result is identical across runs.
        let mut times = Vec::with_capacity(runs.max(1));
        let mut result = None;
        for _ in 0..runs.max(1) {
            let start = Instant::now();
            let r = detect_stars_hocus_focus(&fits.data, fits.width, fits.height, &params);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            result = Some(r);
        }
        times.sort_by(|a, b| a.total_cmp(b));
        let detect_ms = times[times.len() / 2];
        let result = result.unwrap();

        let unsaturated: Vec<f64> = result
            .stars
            .iter()
            .filter(|s| !s.saturated)
            .map(|s| s.hfr)
            .collect();
        let hfr_pool: &[f64] = if unsaturated.len() >= 3 {
            &unsaturated
        } else {
            // Fall back to every star, matching the detector's own average.
            &[]
        };
        let (hfr_mean, hfr_std) = if hfr_pool.is_empty() {
            let all: Vec<f64> = result.stars.iter().map(|s| s.hfr).collect();
            mean_std(&all)
        } else {
            mean_std(hfr_pool)
        };
        let saturated_count = result.stars.iter().filter(|s| s.saturated).count();

        writeln!(
            output,
            "{},{},{},{},{},{},{},{},{},{},{},{:.4},{:.4},{:.1},{:.1},{},{},{},{}",
            entry.telescope.as_deref().unwrap_or(""),
            csv_escape(entry.target.as_deref().unwrap_or("")),
            entry.filter.as_deref().unwrap_or(""),
            entry
                .grading_status
                .map(|s| s.to_string())
                .unwrap_or_default(),
            csv_escape(&entry.path),
            fits.width,
            fits.height,
            entry.nina_stars.map(|s| s.to_string()).unwrap_or_default(),
            entry
                .nina_hfr
                .map(|h| format!("{h:.4}"))
                .unwrap_or_default(),
            result.stars.len(),
            saturated_count,
            hfr_mean,
            hfr_std,
            load_ms,
            detect_ms,
            structure,
            params.detection_binning,
            keep_saturated,
            preset_class,
        )?;

        detect_times.push(detect_ms);
        if let Some(nina_stars) = entry.nina_stars
            && nina_stars > 0
        {
            count_ratios.push(result.stars.len() as f64 / nina_stars as f64);
        }
        if let Some(nina_hfr) = entry.nina_hfr
            && nina_hfr > 0.0
            && hfr_mean > 0.0
        {
            hfr_deltas.push(hfr_mean - nina_hfr);
        }
        eprintln!(
            "[{}/{}] {} stars={} hfr={:.2} detect={:.0}ms",
            index + 1,
            entries.len(),
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            result.stars.len(),
            hfr_mean,
            detect_ms
        );
    }

    detect_times.sort_by(|a, b| a.total_cmp(b));
    count_ratios.sort_by(|a, b| a.total_cmp(b));
    hfr_deltas.sort_by(|a, b| a.total_cmp(b));
    println!(
        "\n=== Benchmark summary ({} frames) ===",
        detect_times.len()
    );
    println!(
        "variant: structure={structure} binning={} keep_saturated={keep_saturated} preset={preset}",
        binning.max(1)
    );
    if !detect_times.is_empty() {
        println!(
            "detect ms: p50={:.0} p95={:.0} max={:.0}",
            percentile(&detect_times, 0.50),
            percentile(&detect_times, 0.95),
            detect_times.last().unwrap()
        );
    }
    if !count_ratios.is_empty() {
        println!(
            "stars vs N.I.N.A. (ratio): p10={:.3} p50={:.3} p90={:.3}",
            percentile(&count_ratios, 0.10),
            percentile(&count_ratios, 0.50),
            percentile(&count_ratios, 0.90),
        );
    }
    if !hfr_deltas.is_empty() {
        println!(
            "HFR - N.I.N.A. (px): p10={:.3} p50={:.3} p90={:.3}",
            percentile(&hfr_deltas, 0.10),
            percentile(&hfr_deltas, 0.50),
            percentile(&hfr_deltas, 0.90),
        );
    }
    println!("CSV: {output_path}");
    Ok(())
}

fn mean_std(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance =
        values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / values.len() as f64;
    (mean, variance.sqrt())
}

/// Nearest-rank percentile of an ascending-sorted slice.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let index = ((sorted.len() as f64 - 1.0) * q).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}
