//! Exporting the final picture: Seiza's CPU render of the picture the canvas
//! shows, at full resolution, saved as a 16-bit TIFF or PNG, or a 32-bit
//! FITS or XISF.

use super::panes::scene;
use super::Studio;
use crate::store_tiles::StoreSource;
use crate::studio_model::Place;
use gpui::Context;
use seiza_compose::picture::Picture;
use seiza_compose::render::{render_image, RenderOptions};
use seiza_compose::ReferenceRegion;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// An export under way.
pub(super) struct ExportRun {
    pub path: PathBuf,
    /// The share of the picture rendered, in thousandths.
    progress: AtomicU32,
    stop: AtomicBool,
    /// Set when the export has ended: the file written, or why not.
    done: Mutex<Option<Result<(), String>>>,
}

impl ExportRun {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Studio {
    /// Shows a line about the export under the Export button.
    fn tell_export(&mut self, line: &str) {
        self.export_line = line.to_string();
    }

    /// Asks where to save the final picture, then exports it there.
    pub(super) fn pick_export(&mut self, cx: &mut Context<Self>) {
        if let Err(reason) = self.export_ready() {
            self.tell_export(&reason);
            return;
        }
        let store = self.store_path().map(Path::to_path_buf);
        let directory = store
            .as_deref()
            .and_then(Path::parent)
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let name = format!("{}.tif", self.document.name);
        let picked = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.start_export(path);
                cx.notify();
            });
        })
        .detach();
    }

    /// Why the final picture cannot be exported now, if it cannot.
    fn export_ready(&self) -> Result<(), String> {
        if self.export.is_some() {
            return Err("An export is under way.".to_string());
        }
        if self.store_path().is_none() {
            return Err("Only a picture opened from a store can be exported.".to_string());
        }
        // An output stretch resolved from the picture needs the samples of
        // the picture as it is now, as the canvas shows it.
        if self.picture_wanted.is_some()
            && self.picture_samples.as_ref().map(|(id, _)| *id) != self.picture_wanted
        {
            return Err(
                "The picture is still being sampled for its output stretch. Export when the canvas shows it."
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Renders the final picture on the CPU and saves it to `path`, on a
    /// thread of its own.
    fn start_export(&mut self, path: PathBuf) {
        if let Err(reason) = self.export_ready() {
            self.tell_export(&reason);
            return;
        }
        let Some(store) = self.store_path().map(Path::to_path_buf) else {
            return;
        };
        let (mut final_scene, states) = scene(
            &self.document,
            &self.band_info,
            &Place::Final.view(),
            self.scene_version,
            self.picture_samples
                .as_ref()
                .map(|(id, samples)| (*id, samples.as_slice())),
            &mut self.resolved_chains,
        );
        final_scene.stretches = self.analysis(&states);
        let picture = final_scene.picture(&[]);
        let (width, height) = self.canvas.read(|shared| shared.canvas.document_size);
        let rect = match self.crop_rect() {
            Some([left, top, right, bottom]) => [
                left.max(0.0) as u32,
                top.max(0.0) as u32,
                (right - left).max(1.0) as u32,
                (bottom - top).max(1.0) as u32,
            ],
            None => [0, 0, width, height],
        };
        let region = within(rect, (width, height));
        // A TIFF or PNG names the working space its values are in; other
        // files hold no profile.
        let raster = extension(&path)
            .is_some_and(|extension| matches!(extension.as_str(), "tif" | "tiff" | "png"));
        let profile = match self.working_profile.as_ref().filter(|_| raster) {
            Some(path) => match std::fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(error) => {
                    self.tell_export(&format!(
                        "The working space's profile {} could not be read: {error}",
                        path.display()
                    ));
                    return;
                }
            },
            None => None,
        };
        let working = self.working_profile.clone();
        let tiff_float = self.tiff_float;
        let run = Arc::new(ExportRun {
            path: path.clone(),
            progress: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            done: Mutex::new(None),
        });
        self.export = Some(run.clone());
        self.tell_export(&format!("Exporting {}…", file_name(&path)));
        std::thread::spawn(move || {
            let result = export(
                &picture,
                &store,
                region,
                profile.as_deref(),
                working.as_deref(),
                tiff_float,
                &run,
            );
            *run.done.lock().expect("export result") = Some(result);
        });
    }

    /// Shows how far the export has got, and its result when it ends.
    pub(super) fn follow_export(&mut self) {
        let Some(run) = self.export.clone() else {
            return;
        };
        let done = run.done.lock().expect("export result").take();
        let name = file_name(&run.path);
        match done {
            None => {
                let share = run.progress.load(Ordering::Relaxed) as f32 / 10.0;
                self.tell_export(&format!("Exporting {name}: {share:.0}%"));
            }
            Some(Ok(())) => {
                self.export = None;
                self.tell_export(&format!("Exported {name}."));
            }
            Some(Err(reason)) => {
                self.export = None;
                self.tell_export(&format!("{name} was not exported: {reason}"));
            }
        }
    }
}

/// `rect`, as x, y, width and height, kept within a document of `size`.
fn within(rect: [u32; 4], size: (u32, u32)) -> ReferenceRegion {
    let x = rect[0].min(size.0.saturating_sub(1));
    let y = rect[1].min(size.1.saturating_sub(1));
    ReferenceRegion {
        x: x as usize,
        y: y as usize,
        width: rect[2].clamp(1, size.0 - x) as usize,
        height: rect[3].clamp(1, size.1 - y) as usize,
    }
}

/// The export itself: the store read again in 32-bit floats, the picture
/// rendered by Seiza on the CPU, and the file written.
fn export(
    picture: &Picture,
    store: &Path,
    region: ReferenceRegion,
    profile: Option<&[u8]>,
    working: Option<&Path>,
    tiff_float: bool,
    run: &ExportRun,
) -> Result<(), String> {
    let (store, _) = crate::store_tiles::open_store(store)?;
    let source = StoreSource::new(store);
    let image = render_image(
        picture,
        &source,
        region,
        &RenderOptions {
            stop: Some(&|| run.stop.load(Ordering::Relaxed)),
            progress: Some(&|share| {
                run.progress
                    .store((share * 1000.0) as u32, Ordering::Relaxed)
            }),
            ..RenderOptions::default()
        },
    )
    .map_err(|error| match error {
        seiza_compose::Error::Cancelled => "it was stopped".to_string(),
        error => error.to_string(),
    })?;
    match extension(&run.path).as_deref() {
        Some("avif") => {
            return save_hdr(&run.path, image.width, image.height, &image.data, working);
        }
        Some("tif" | "tiff") if tiff_float => {
            // Uncovered pixels as black; every other value as it is.
            let rgb: Vec<f32> = image
                .data
                .iter()
                .map(|value| if value.is_finite() { *value } else { 0.0 })
                .collect();
            return seiza_image_io::save_float_tiff(
                &run.path,
                image.width,
                image.height,
                seiza_fits::F32ImageData::RgbInterleaved(&rgb),
                profile,
            )
            .map_err(|error| error.to_string());
        }
        _ => {}
    }
    save(&run.path, image.width, image.height, &image.data, profile)
}

/// A path's extension, in lower case.
fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_ascii_lowercase())
}

/// Writes interleaved RGB in the working space, where 1 is SDR white, as
/// an HDR AVIF: converted to linear Rec. 2020 through the working space's
/// ICC profile (sRGB when Studio was given none). Above 1 a value is linear
/// light already, a multiple of SDR white's, as Tone's HDR headroom is.
fn save_hdr(
    path: &Path,
    width: usize,
    height: usize,
    rgb: &[f32],
    working: Option<&Path>,
) -> Result<(), String> {
    let system = Path::new(psf_studio_color::SYSTEM_PROFILES);
    let source = working.map_or_else(|| system.join("sRGB Profile.icc"), Path::to_path_buf);
    let curves = psf_studio_color::curves_between(&source, &system.join("ITU-2020.icc"))?;
    let linear = to_linear(rgb, &curves);
    seiza_image_io::save_hdr_avif(
        path,
        width,
        height,
        &linear,
        &seiza_image_io::HdrAvifOptions::default(),
    )
    .map_err(|error| error.to_string())
}

/// `rgb` in the working space as linear light in the curves' target: each
/// channel decoded by the curve within 0 to 1 and taken as linear light
/// above it, then through the matrix. Uncovered pixels are black.
fn to_linear(rgb: &[f32], curves: &psf_studio_render::display::DisplayCurves) -> Vec<f32> {
    let last = (curves.width.max(2) - 1) as f32;
    let decode = |value: f32, channel: usize| -> f32 {
        if !value.is_finite() || value <= 0.0 {
            return 0.0;
        }
        if value > 1.0 {
            return value;
        }
        let place = value * last;
        let low = place.floor() as usize;
        let high = (low + 1).min(curves.decode.len() - 1);
        let share = place - low as f32;
        curves.decode[low][channel] * (1.0 - share) + curves.decode[high][channel] * share
    };
    let matrix = curves.to_target;
    rgb.as_chunks::<3>()
        .0
        .iter()
        .flat_map(|pixel| {
            let linear: [f32; 3] = std::array::from_fn(|channel| decode(pixel[channel], channel));
            // The matrix is column-major: column c is source channel c.
            std::array::from_fn::<f32, 3, _>(|row| {
                (0..3)
                    .map(|column| matrix[column][row] * linear[column])
                    .sum::<f32>()
                    .max(0.0)
            })
        })
        .collect()
}

/// Writes interleaved RGB to `path` in the format its extension names,
/// through Seiza: TIFF or PNG at 16 bits per channel, kept within 0 to 1,
/// with `profile` embedded when there is one, or FITS or XISF as 32-bit
/// floats as they are, FITS rows top-down. A pixel no input covers is
/// written as black.
fn save(
    path: &Path,
    width: usize,
    height: usize,
    rgb: &[f32],
    profile: Option<&[u8]>,
) -> Result<(), String> {
    let rgb: Vec<f32> = rgb
        .iter()
        .map(|value| if value.is_finite() { *value } else { 0.0 })
        .collect();
    seiza_image_io::save_f32_with_profile(
        path,
        width,
        height,
        seiza_fits::F32ImageData::RgbInterleaved(&rgb),
        &seiza_image_io::SaveOptions {
            fits_top_down: true,
            ..Default::default()
        },
        profile,
    )
    .map_err(|error| error.to_string())
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl Studio {
    /// Renders the patches the canvas sampled on the CPU, compares them with
    /// the canvas's samples, and writes how far apart they are to `report`,
    /// on a thread of its own.
    pub(super) fn write_parity_report(&mut self, report: PathBuf, gpu: Arc<Vec<f32>>) {
        let Some(store) = self.store_path().map(Path::to_path_buf) else {
            return;
        };
        // The scene the canvas sampled: the final picture's layers.
        let (mut probe, states) = scene(
            &self.document,
            &self.band_info,
            &Place::Final.view(),
            self.scene_version,
            None,
            &mut crate::output_stretch::ResolvedChains::default(),
        );
        probe.stretches = self.analysis(&states);
        let combined = probe.combined(&[]);
        let (width, height) = self.canvas.read(|shared| shared.canvas.document_size);
        let rect = self
            .crop_rect()
            .unwrap_or([0.0, 0.0, width as f32, height as f32]);
        let (patches, size) = super::picture_patches(rect);
        let [left, top, right, bottom] = rect.map(|value| value.max(0.0) as u32);
        let region = within(
            [
                left,
                top,
                right.saturating_sub(left),
                bottom.saturating_sub(top),
            ],
            (width, height),
        );
        std::thread::spawn(move || {
            let text = parity(&combined, &store, region, &patches, size, &gpu)
                .unwrap_or_else(|error| format!("{{\"error\": {error:?}}}"));
            if let Err(error) = std::fs::write(&report, text) {
                eprintln!("could not write {}: {error}", report.display());
            }
        });
    }
}

/// How far Seiza's CPU render of the combined picture is from the canvas's
/// samples of the same patches, as JSON. The CPU renders all of `region`,
/// as the canvas draws the patches from the tiles around them.
fn parity(
    combined: &Picture,
    store: &Path,
    region: ReferenceRegion,
    patches: &[[u32; 2]],
    size: u32,
    gpu: &[f32],
) -> Result<String, String> {
    let (store, _) = crate::store_tiles::open_store(store)?;
    let source = StoreSource::new(store);
    let image = render_image(combined, &source, region, &RenderOptions::default())
        .map_err(|error| error.to_string())?;
    let size = size as usize;
    let mut differences = Vec::with_capacity(gpu.len());
    for (index, origin) in patches.iter().enumerate() {
        let start = index * size * size * 3;
        let canvas = gpu
            .get(start..start + size * size * 3)
            .ok_or("the canvas gave fewer samples than the patches")?;
        for row in 0..size {
            let y = origin[1] as usize + row - region.y;
            let x = origin[0] as usize - region.x;
            let from = (y * region.width + x) * 3;
            let cpu = image
                .data
                .get(from..from + size * 3)
                .ok_or("a patch lies outside the picture")?;
            let line = &canvas[row * size * 3..(row + 1) * size * 3];
            differences.extend(cpu.iter().zip(line).map(|(a, b)| (a - b).abs()));
        }
    }
    differences.sort_by(f32::total_cmp);
    let at = |share: f64| differences[((differences.len() - 1) as f64 * share) as usize];
    let mean = differences
        .iter()
        .map(|value| f64::from(*value))
        .sum::<f64>()
        / differences.len().max(1) as f64;
    Ok(format!(
        "{{\"samples\": {}, \"mean\": {mean:e}, \"median\": {:e}, \"p99\": {:e}, \"p999\": {:e}, \"max\": {:e}}}\n",
        differences.len(),
        at(0.5),
        at(0.99),
        at(0.999),
        at(1.0)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tiff_keeps_sixteen_bits() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("picture.tif");
        // Two values one 16-bit step apart, which 8 bits could not tell apart.
        let step = 1.0 / 65535.0;
        let rgb = [0.5, 0.5, 0.5, 0.5 + step, 0.5 + step, 0.5 + step];
        save(&path, 2, 1, &rgb, None).unwrap();
        let read = seiza_image_io::open(&path).unwrap();
        let samples = &read.samples;
        assert!((samples[0] - 0.5).abs() < step);
        assert!(samples[3] > samples[0]);
    }

    #[test]
    fn an_hdr_export_is_linear_rec_2020_with_sdr_white_at_one() {
        let system = Path::new(psf_studio_color::SYSTEM_PROFILES);
        let Ok(curves) = psf_studio_color::curves_between(
            &system.join("sRGB Profile.icc"),
            &system.join("ITU-2020.icc"),
        ) else {
            eprintln!("no system profiles: not checked");
            return;
        };
        // SDR white, mid gray, and a highlight two stops up, in sRGB.
        let linear = to_linear(&[1.0, 1.0, 1.0, 0.5, 0.5, 0.5, 2.0, 2.0, 2.0], &curves);
        for channel in &linear[..3] {
            assert!((channel - 1.0).abs() < 0.01, "{linear:?}");
        }
        assert!((linear[3] - 0.214).abs() < 0.01, "{linear:?}");
        // Above SDR white a value is linear light: twice SDR white's.
        for channel in &linear[6..9] {
            assert!((channel - 2.0).abs() < 0.03, "{linear:?}");
        }
        // Pure sRGB red is inside Rec. 2020: mostly red, some green.
        let red = to_linear(&[1.0, 0.0, 0.0], &curves);
        assert!(red[0] > 0.6 && red[1] > 0.03 && red[2] < 0.05, "{red:?}");
    }
}
