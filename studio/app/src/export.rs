//! Exporting the final picture: Seiza's CPU render of the picture the canvas
//! shows, at full resolution. A TIFF keeps the working space, Rec. 2020 on
//! the sRGB curve, and embeds its profile, at 16 bits or as 32-bit floats; a
//! PNG is mapped to Display P3 and embeds that profile; an AVIF is HDR in
//! Rec. 2020 with PQ; FITS and XISF hold the working space's values as
//! 32-bit floats.

use super::panes::scene;
use super::Studio;
use crate::canvas::WORKING_SPACE;
use crate::store_tiles::StoreSource;
use crate::studio_model::Place;
use gpui::Context;
use seiza_compose::picture::Picture;
use seiza_compose::render::{render_image, RenderOptions};
use seiza_compose::space::WorkingSpace;
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
    /// For an editor: the key for everything before the junction when the
    /// file was written. The file then opens in its default app.
    for_editor: Option<String>,
}

impl ExportRun {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Studio {
    /// Shows a line about the export under the Export button.
    pub(super) fn tell_export(&mut self, line: &str) {
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
        let format = self.export_format();
        let name = format!("{}.{format}", self.document.name);
        let picked = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            let path = with_format(path, format);
            let _ = this.update(cx, |this, cx| {
                this.start_export(path);
                cx.notify();
            });
        })
        .detach();
    }

    /// The format the export writes, by extension: the one chosen, or AVIF
    /// for an HDR picture and TIFF for another.
    pub(super) fn export_format(&self) -> &'static str {
        self.export_format
            .unwrap_or(if self.document.headroom.is_some() {
                "avif"
            } else {
                "tif"
            })
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
    pub(super) fn start_export(&mut self, path: PathBuf) {
        self.start_export_for(path, None);
    }

    /// Exports the final picture to `path`; with `for_editor`, a 16-bit
    /// TIFF for an editor, which opens in its default app once written.
    pub(super) fn start_export_for(&mut self, path: PathBuf, for_editor: Option<String>) {
        if let Err(reason) = self.export_ready() {
            self.tell_export(&reason);
            return;
        }
        let Some(store) = self.store_path().map(Path::to_path_buf) else {
            return;
        };
        // The scene is built on the export's thread, from the document as
        // it is now.
        let document = self.document.clone();
        let band_info = self.band_info.clone();
        let picture_samples = self.picture_samples.clone();
        let scene_version = self.scene_version;
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
        // An editor reads 16 bits.
        let tiff_float = self.tiff_float && for_editor.is_none();
        let planes = self.derived.clone();
        let resolved = self.scene_worker.resolved();
        let run = Arc::new(ExportRun {
            path: path.clone(),
            progress: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            done: Mutex::new(None),
            for_editor,
        });
        self.export = Some(run.clone());
        self.tell_export(&format!("Exporting {}…", file_name(&path)));
        std::thread::spawn(move || {
            let (mut final_scene, states) = scene(
                &document,
                &band_info,
                &Place::Final.view(),
                scene_version,
                picture_samples
                    .as_ref()
                    .map(|(id, samples)| (*id, samples.as_slice())),
                &mut crate::output_stretch::ResolvedChains::from_resolved(&resolved),
                // The view stretch is for looking only: an export leaves a
                // channel nothing stretches linear.
                &[],
            );
            final_scene.stretches = Arc::new(band_info.stretches(&states).unwrap_or_default());
            let picture = final_scene.picture(&[]);
            let result = export(&picture, &store, &planes, region, tiff_float, &run);
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
                match &run.for_editor {
                    Some(upstream) => match self.exported_for_editor(&run.path, upstream) {
                        Ok(()) => self.tell_export(&format!(
                            "Opened {name} in its default app. Save it there to bring the edit back."
                        )),
                        Err(reason) => self.tell_export(&format!("{name} was written, but {reason}")),
                    },
                    None => {
                        self.tell_export(&format!("Exported {name}."));
                        self.export_finished(&run.path);
                    }
                }
            }
            Some(Err(reason)) => {
                self.export = None;
                self.tell_export(&format!("{name} was not exported: {reason}"));
            }
        }
    }
}

/// The formats an export writes, by extension, with their names.
pub(super) const EXPORT_FORMATS: [(&str, &str); 5] = [
    ("tif", "TIFF"),
    ("png", "PNG"),
    ("avif", "AVIF, HDR"),
    ("fits", "FITS"),
    ("xisf", "XISF"),
];

/// `path` written in `format`: a name the save panel returned with no
/// format's extension takes `format`'s, and one the panel gave a second
/// extension, such as `Iris.avif.tif`, keeps the one chosen.
pub(super) fn with_format(mut path: PathBuf, format: &str) -> PathBuf {
    let known = |extension: &str| {
        EXPORT_FORMATS
            .iter()
            .any(|(known, _)| known.eq_ignore_ascii_case(extension))
            || extension.eq_ignore_ascii_case("tiff")
    };
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().into_owned());
    let inner = path
        .file_stem()
        .map(Path::new)
        .and_then(Path::extension)
        .map(|inner| inner.to_string_lossy().into_owned());
    match (extension, inner) {
        (Some(outer), Some(inner))
            if known(&outer)
                && inner.eq_ignore_ascii_case(format)
                && !outer.eq_ignore_ascii_case(format) =>
        {
            path.set_extension("");
            path
        }
        (Some(extension), _) if known(&extension) => path,
        _ => {
            let mut name = path
                .file_name()
                .map(|name| name.to_os_string())
                .unwrap_or_default();
            name.push(format!(".{format}"));
            path.set_file_name(name);
            path
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
    planes: &[Arc<crate::derived::DerivedPlane>],
    region: ReferenceRegion,
    tiff_float: bool,
    run: &ExportRun,
) -> Result<(), String> {
    let (store, _) = crate::store_tiles::open_store(store)?;
    let source = StoreSource::new(store).with_planes(planes.to_vec());
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
    // Values above the picture's peak, SDR white unless HDR Tone lifts it,
    // are clipped there, as the canvas shows them.
    let mut image = image;
    let peak = picture.peak();
    for value in &mut image.data {
        *value = value.min(peak);
    }
    // Uncovered pixels as black.
    for value in &mut image.data {
        if !value.is_finite() {
            *value = 0.0;
        }
    }
    let (width, height) = (image.width, image.height);
    match extension(&run.path).as_deref() {
        Some("avif") => save_hdr(&run.path, width, height, &image.data),
        Some("tif" | "tiff") => {
            let profile = psf_studio_color::space_profile(WORKING_SPACE)?;
            if tiff_float {
                // Every value as it is, HDR's above 1 included.
                seiza_image_io::save_float_tiff(
                    &run.path,
                    width,
                    height,
                    seiza_fits::F32ImageData::RgbInterleaved(&image.data),
                    Some(&profile),
                )
                .map_err(|error| error.to_string())
            } else {
                save(&run.path, width, height, &image.data, Some(&profile))
            }
        }
        Some("png") => {
            let shown = for_display(&image.data, WorkingSpace::DisplayP3);
            let profile = psf_studio_color::space_profile(WorkingSpace::DisplayP3)?;
            save(&run.path, width, height, &shown, Some(&profile))
        }
        _ => save(&run.path, width, height, &image.data, None),
    }
}

/// Interleaved RGB in the working space, mapped into `display`'s primaries
/// as the canvas maps it: a color the display cannot show keeps its
/// luminance and hue.
pub(super) fn for_display(rgb: &[f32], display: WorkingSpace) -> Vec<f32> {
    rgb.as_chunks::<3>()
        .0
        .iter()
        .flat_map(|pixel| WORKING_SPACE.map_to_display(*pixel, display))
        .collect()
}

/// A path's extension, in lower case.
fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_ascii_lowercase())
}

/// Writes interleaved RGB in the working space, where 1 is SDR white, as
/// an HDR AVIF. The working space has Rec. 2020's primaries, so decoding
/// its extended sRGB curve gives the linear Rec. 2020 light the writer
/// takes.
fn save_hdr(path: &Path, width: usize, height: usize, rgb: &[f32]) -> Result<(), String> {
    let linear: Vec<f32> = rgb
        .iter()
        .map(|value| seiza_compose::tone::extended_srgb_decode(value.max(0.0)))
        .collect();
    seiza_image_io::save_hdr_avif(
        path,
        width,
        height,
        &linear,
        &seiza_image_io::HdrAvifOptions::default(),
    )
    .map_err(|error| error.to_string())
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
        // The scene the canvas sampled, the final picture's layers, is
        // built on the check's thread.
        let document = self.document.clone();
        let band_info = self.band_info.clone();
        let view_stretch = self.view_stretch_stages();
        let scene_version = self.scene_version;
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
        let planes = self.derived.clone();
        let resolved = self.scene_worker.resolved();
        std::thread::spawn(move || {
            let (mut probe, states) = scene(
                &document,
                &band_info,
                &Place::Final.view(),
                scene_version,
                None,
                &mut crate::output_stretch::ResolvedChains::from_resolved(&resolved),
                &view_stretch,
            );
            probe.stretches = Arc::new(band_info.stretches(&states).unwrap_or_default());
            let combined = probe.combined(&[]);
            let text = parity(&combined, &store, &planes, region, &patches, size, &gpu)
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
    planes: &[Arc<crate::derived::DerivedPlane>],
    region: ReferenceRegion,
    patches: &[[u32; 2]],
    size: u32,
    gpu: &[f32],
) -> Result<String, String> {
    let (store, _) = crate::store_tiles::open_store(store)?;
    let source = StoreSource::new(store).with_planes(planes.to_vec());
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
    fn an_export_name_keeps_the_chosen_format() {
        let path = |name: &str| PathBuf::from(format!("/pictures/{name}"));
        assert_eq!(with_format(path("Iris.avif"), "avif"), path("Iris.avif"));
        // A name typed without an extension takes the chosen format's.
        assert_eq!(with_format(path("Iris"), "avif"), path("Iris.avif"));
        assert_eq!(with_format(path("Iris v2"), "tif"), path("Iris v2.tif"));
        // A second extension the save panel added goes.
        assert_eq!(
            with_format(path("Iris.avif.tif"), "avif"),
            path("Iris.avif")
        );
        // An extension a person typed wins over the chosen format.
        assert_eq!(with_format(path("Iris.png"), "avif"), path("Iris.png"));
        assert_eq!(with_format(path("Iris.tiff"), "tif"), path("Iris.tiff"));
    }

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
    fn layers_that_add_to_white_still_do_in_the_working_space() {
        let primaries = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let mut sum = [0.0_f32; 3];
        for color in primaries {
            let working = crate::canvas::working_color(color);
            for channel in 0..3 {
                sum[channel] += working[channel];
            }
        }
        assert!(
            sum.iter().all(|channel| (channel - 1.0).abs() < 2e-3),
            "{sum:?}"
        );
        // White in the working space is white on a Display P3 file.
        let white = for_display(&[0.5, 0.5, 0.5], WorkingSpace::DisplayP3);
        assert!(
            white.iter().all(|channel| (channel - 0.5).abs() < 1e-3),
            "{white:?}"
        );
        // Rec. 2020's green, which Display P3 cannot show, keeps its hue.
        let green = for_display(&[0.0, 1.0, 0.0], WorkingSpace::DisplayP3);
        assert!(green.iter().all(|channel| *channel >= 0.0), "{green:?}");
        assert!(green[1] > green[0] && green[1] > green[2], "{green:?}");
    }
}
