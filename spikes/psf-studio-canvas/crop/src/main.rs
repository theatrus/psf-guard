//! Investigation 3 of docs/design/psf-studio.md: a heavy step previewed on a
//! full-resolution crop, with Seiza's Richardson-Lucy deconvolution.
//!
//! `bench <file>` measures the step in this process and writes JSON.
//! `serve [port] [results-dir]` answers the same previews over HTTP, for the
//! page in `web/crop.html`.

use half::f16;
use rayon::prelude::*;
use seiza_deconvolution::{deconvolve, DeconvolutionConfig};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Instant;
use studio_render_spike::synth::{generate_tile, TileKey, TILE};

const WIDTH: usize = 6144;
const HEIGHT: usize = 4096;

/// A linear mono image cut from band 0 of the synthetic document.
fn source_image() -> Vec<f32> {
    let tile = TILE as usize;
    let (across, down) = (WIDTH / tile, HEIGHT / tile);
    let tiles: Vec<Vec<u16>> = (0..across * down)
        .into_par_iter()
        .map(|index| {
            generate_tile(TileKey {
                level: 0,
                x: (20 + index % across) as u16,
                y: (12 + index / across) as u16,
            })
        })
        .collect();
    let mut image = vec![0f32; WIDTH * HEIGHT];
    for (index, samples) in tiles.iter().enumerate() {
        let (tile_x, tile_y) = (index % across, index / across);
        for row in 0..tile {
            let start = (tile_y * tile + row) * WIDTH + tile_x * tile;
            for (target, bits) in image[start..start + tile]
                .iter_mut()
                .zip(&samples[row * tile..(row + 1) * tile])
            {
                *target = f16::from_bits(*bits).to_f32();
            }
        }
    }
    image
}

fn crop(image: &[f32], x: usize, y: usize, width: usize, height: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(width * height);
    for row in y..y + height {
        out.extend_from_slice(&image[row * WIDTH + x..row * WIDTH + x + width]);
    }
    out
}

/// Runs the step on a square crop with `padding` extra pixels on each side
/// and returns the crop without the padding.
fn preview(
    image: &[f32],
    x: usize,
    y: usize,
    size: usize,
    padding: usize,
    config: &DeconvolutionConfig,
) -> Vec<f32> {
    let padded = size + 2 * padding;
    let input = crop(image, x - padding, y - padding, padded, padded);
    let restored = deconvolve(&input, padded, padded, 1, config)
        .expect("valid settings")
        .data;
    let mut out = Vec::with_capacity(size * size);
    for row in padding..padding + size {
        out.extend_from_slice(&restored[row * padded + padding..row * padded + padding + size]);
    }
    out
}

fn milliseconds(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

/// The median of `runs` timings of `work`, in milliseconds.
fn median_ms(runs: usize, mut work: impl FnMut()) -> f64 {
    let mut times: Vec<f64> = (0..runs)
        .map(|_| {
            let started = Instant::now();
            work();
            milliseconds(started)
        })
        .collect();
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn settings(iterations: usize, fwhm: f32, amount: f32) -> DeconvolutionConfig {
    DeconvolutionConfig {
        iterations,
        amount,
        ..DeconvolutionConfig::conservative(fwhm)
    }
}

fn bench(output: &str) {
    let started = Instant::now();
    let image = source_image();
    eprintln!("source image in {:.0} ms", milliseconds(started));
    let (x, y) = (2048, 1024);
    let threads = rayon::current_num_threads();

    // 1. How long one preview takes, by crop size and iteration count.
    let mut preview_times = Vec::new();
    for size in [512usize, 1024, 2048] {
        for iterations in [4usize, 10, 20, 50] {
            let config = settings(iterations, 3.0, 1.0);
            let all = median_ms(7, || {
                std::hint::black_box(preview(&image, x, y, size, 32, &config));
            });
            let four = rayon::ThreadPoolBuilder::new()
                .num_threads(4)
                .build()
                .expect("a pool")
                .install(|| {
                    median_ms(7, || {
                        std::hint::black_box(preview(&image, x, y, size, 32, &config));
                    })
                });
            eprintln!("{size} px, {iterations} iterations: {all:.1} ms, {four:.1} ms on 4 threads");
            preview_times.push(json!({
                "crop": size, "iterations": iterations,
                "ms_all_threads": round(all), "ms_4_threads": round(four),
            }));
        }
    }

    // 2. PSF width changes the kernel size, and so the cost.
    let fwhm_times: Vec<Value> = [1.5f32, 3.0, 6.0, 12.0]
        .iter()
        .map(|fwhm| {
            let config = settings(10, *fwhm, 1.0);
            let ms = median_ms(7, || {
                std::hint::black_box(preview(&image, x, y, 1024, 32, &config));
            });
            json!({ "fwhm": fwhm, "ms": round(ms) })
        })
        .collect();

    // 3. Is `amount` a plain blend? Then the canvas can apply it as a light
    //    step from one cached result.
    let size = 1024;
    let original = crop(&image, x, y, size, size);
    let full = preview(&image, x, y, size, 32, &settings(10, 3.0, 1.0));
    let partial = preview(&image, x, y, size, 32, &settings(10, 3.0, 0.35));
    let blend_error = original
        .iter()
        .zip(&full)
        .zip(&partial)
        .map(|((original, full), partial)| {
            (original + 0.35 * (full - original) - partial).abs() as f64
        })
        .fold(0.0, f64::max);

    // 4. How far a crop's result is from the same pixels of the whole image.
    let config = settings(10, 3.0, 1.0);
    let whole_started = Instant::now();
    let whole = deconvolve(&image, WIDTH, HEIGHT, 1, &config)
        .expect("valid settings")
        .data;
    let whole_ms = milliseconds(whole_started);
    let reference = crop(&whole, x, y, size, size);
    let change: f64 = original
        .iter()
        .zip(&reference)
        .map(|(a, b)| ((a - b) as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    let fidelity: Vec<Value> = [0usize, 8, 16, 32, 64, 128]
        .iter()
        .map(|padding| {
            let result = preview(&image, x, y, size, *padding, &config);
            let (mut max, mut sum) = (0f64, 0f64);
            for (a, b) in result.iter().zip(&reference) {
                let difference = (a - b).abs() as f64;
                max = max.max(difference);
                sum += difference * difference;
            }
            json!({
                "padding": padding,
                "max_difference": max,
                // The crop's error as a share of what the step changed.
                "error_over_change": sum.sqrt() / change,
            })
        })
        .collect();

    // The step sets its noise floor from the range of the samples it is
    // given. Give the crop the whole image's floor and compare again.
    let range = |samples: &[f32]| {
        let (low, high) = samples
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), value| {
                (low.min(*value), high.max(*value))
            });
        high - low
    };
    let whole_range = range(&image);
    let matched_fidelity: Vec<Value> = [0usize, 8, 16, 32, 64]
        .iter()
        .map(|padding| {
            let padded = size + 2 * padding;
            let crop_range = range(&crop(&image, x - padding, y - padding, padded, padded));
            let matched = DeconvolutionConfig {
                noise_fraction: config.noise_fraction * whole_range / crop_range,
                ..config
            };
            let result = preview(&image, x, y, size, *padding, &matched);
            let (mut max, mut sum) = (0f64, 0f64);
            for (a, b) in result.iter().zip(&reference) {
                let difference = (a - b).abs() as f64;
                max = max.max(difference);
                sum += difference * difference;
            }
            json!({
                "padding": padding,
                "max_difference": max,
                "error_over_change": sum.sqrt() / change,
            })
        })
        .collect();

    // 5. A slider drag: 40 positions of the iteration count, then back. The
    //    way back is answered from the cache.
    let mut cache: HashMap<(usize, u32), Vec<f32>> = HashMap::new();
    let positions: Vec<usize> = (1..=40).chain((1..=40).rev()).collect();
    let mut first_visit = Vec::new();
    let mut revisit = Vec::new();
    for iterations in positions {
        let started = Instant::now();
        let key = (iterations, 3.0f32.to_bits());
        let hit = cache.contains_key(&key);
        let result = cache
            .entry(key)
            .or_insert_with(|| preview(&image, x, y, size, 32, &settings(iterations, 3.0, 1.0)));
        // What the canvas needs: half floats, ready to upload.
        let upload: Vec<u16> = result
            .iter()
            .map(|value| f16::from_f32(*value).to_bits())
            .collect();
        std::hint::black_box(upload);
        if hit { &mut revisit } else { &mut first_visit }.push(milliseconds(started));
    }
    let summarize = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        json!({
            "p50": round(values[values.len() / 2]),
            "max": round(values[values.len() - 1]),
        })
    };

    let report = json!({
        "image": { "width": WIDTH, "height": HEIGHT },
        "threads": threads,
        "preview_ms": preview_times,
        "preview_ms_by_fwhm_1024_10_iterations": fwhm_times,
        "amount_is_a_blend_max_error": blend_error,
        "whole_image_ms": round(whole_ms),
        "whole_image_megapixels": (WIDTH * HEIGHT) as f64 / 1.0e6,
        "crop_against_whole_image": fidelity,
        "crop_against_whole_image_with_its_noise_floor": matched_fidelity,
        "slider_drag_1024": {
            "first_visit_ms": summarize(first_visit),
            "revisit_from_cache_ms": summarize(revisit),
        },
    });
    std::fs::write(output, serde_json::to_string_pretty(&report).expect("JSON"))
        .expect("writing the report");
    println!("{}", serde_json::to_string_pretty(&report).expect("JSON"));
}

/// Answers previews over HTTP, as a web editor's server would.
fn serve(port: u16, results: &str) {
    let image = source_image();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let server = tiny_http::Server::http(("127.0.0.1", port)).expect("binding the port");
    eprintln!("http://localhost:{port}/web/crop.html");
    for mut request in server.incoming_requests() {
        let url = request.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let parameter = |name: &str| {
            query
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        };
        let header = |name: &str, value: &str| {
            tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a header")
        };
        if path == "/preview" {
            let number = |name: &str, default: f32| {
                parameter(name)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(default)
            };
            let size = (number("size", 1024.0) as usize).clamp(64, 2048);
            let config = settings(
                number("iterations", 10.0) as usize,
                number("fwhm", 3.0),
                number("amount", 1.0),
            );
            let started = Instant::now();
            let result = preview(&image, 2048, 1024, size, 32, &config);
            let bytes: Vec<u8> = result
                .iter()
                .flat_map(|value| f16::from_f32(*value).to_bits().to_le_bytes())
                .collect();
            let compute = format!("{:.2}", milliseconds(started));
            let response = tiny_http::Response::from_data(bytes)
                .with_header(header("Content-Type", "application/octet-stream"))
                .with_header(header("X-Compute-Ms", &compute))
                .with_header(header("Cache-Control", "no-store"));
            let _ = request.respond(response);
        } else if request.method() == &tiny_http::Method::Post {
            let name: String = path
                .rsplit('/')
                .next()
                .unwrap_or("result")
                .chars()
                .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
                .collect();
            let mut body = Vec::new();
            let _ = request.as_reader().read_to_end(&mut body);
            let _ = std::fs::create_dir_all(results);
            let _ = std::fs::write(format!("{results}/{name}.json"), body);
            let _ = request.respond(tiny_http::Response::empty(204));
        } else {
            // Static files of the spike, for the page itself.
            let relative = path.trim_start_matches('/');
            let file = root.join(relative);
            let allowed = !relative.contains("..") && file.is_file();
            match allowed.then(|| std::fs::read(&file).ok()).flatten() {
                Some(bytes) => {
                    let kind = match file.extension().and_then(|extension| extension.to_str()) {
                        Some("html") => "text/html; charset=utf-8",
                        Some("js") => "text/javascript",
                        _ => "application/octet-stream",
                    };
                    let _ = request.respond(
                        tiny_http::Response::from_data(bytes)
                            .with_header(header("Content-Type", kind)),
                    );
                }
                None => {
                    let _ = request.respond(tiny_http::Response::empty(404));
                }
            }
        }
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("bench") => bench(arguments.get(1).map_or("crop-preview.json", String::as_str)),
        Some("serve") => serve(
            arguments
                .get(1)
                .and_then(|port| port.parse().ok())
                .unwrap_or(8766),
            arguments.get(2).map_or("web/results", String::as_str),
        ),
        _ => {
            eprintln!("usage: studio-crop-preview-spike bench <file> | serve [port] [results-dir]");
            std::process::exit(2);
        }
    }
}
