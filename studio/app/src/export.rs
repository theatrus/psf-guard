//! Exporting the final picture: the CPU render of what the canvas shows, at
//! full resolution, saved as a 16-bit TIFF or PNG, or a 32-bit FITS or XISF.

use super::panes::scene;
use super::{Scene, Studio};
use crate::studio_model::Place;
use gpui::Context;
use psf_studio_canvas::cpu;
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
        let tables = final_scene.tables();
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
        let run = Arc::new(ExportRun {
            path: path.clone(),
            progress: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            done: Mutex::new(None),
        });
        self.export = Some(run.clone());
        self.tell_export(&format!("Exporting {}…", file_name(&path)));
        std::thread::spawn(move || {
            let result = export(&final_scene, &tables, &store, (width, height), rect, &run);
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

/// The export itself: the store read again in 32-bit floats, the picture
/// rendered on the CPU, and the file written.
fn export(
    scene: &Scene,
    tables: &[Vec<f32>],
    store: &Path,
    document: (u32, u32),
    rect: [u32; 4],
    run: &ExportRun,
) -> Result<(), String> {
    let (reader, _) = crate::store_tiles::open_store(store)?;
    let pane = scene.pane(tables);
    let image = cpu::render(
        &pane,
        &reader,
        document,
        rect,
        &|| run.stop.load(Ordering::Relaxed),
        &|share| {
            run.progress
                .store((share * 1000.0) as u32, Ordering::Relaxed)
        },
    )?;
    psf_studio_canvas::export::save(&run.path, &image)
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
            &mut psf_studio_canvas::output_stretch::ResolvedChains::default(),
        );
        probe.stretches = self.analysis(&states);
        let tables = probe.tables();
        let (width, height) = self.canvas.read(|shared| shared.canvas.document_size);
        let rect = self
            .crop_rect()
            .unwrap_or([0.0, 0.0, width as f32, height as f32]);
        let (patches, size) = super::picture_patches(rect);
        std::thread::spawn(move || {
            let text = parity(
                &probe,
                &tables,
                &store,
                (width, height),
                &patches,
                size,
                &gpu,
            )
            .unwrap_or_else(|error| format!("{{\"error\": {error:?}}}"));
            if let Err(error) = std::fs::write(&report, text) {
                eprintln!("could not write {}: {error}", report.display());
            }
        });
    }
}

/// How far the CPU's combined picture is from the canvas's samples of the
/// same patches, as JSON.
fn parity(
    scene: &Scene,
    tables: &[Vec<f32>],
    store: &Path,
    document: (u32, u32),
    patches: &[[u32; 2]],
    size: u32,
    gpu: &[f32],
) -> Result<String, String> {
    let (reader, _) = crate::store_tiles::open_store(store)?;
    let pane = scene.pane(tables);
    let mut differences = Vec::with_capacity(gpu.len());
    for (index, origin) in patches.iter().enumerate() {
        let cpu =
            cpu::render_combined(&pane, &reader, document, [origin[0], origin[1], size, size])?;
        let start = index * (size * size * 3) as usize;
        let canvas = gpu
            .get(start..start + cpu.len())
            .ok_or("the canvas gave fewer samples than the patches")?;
        differences.extend(cpu.iter().zip(canvas).map(|(a, b)| (a - b).abs()));
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
