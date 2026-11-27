//! Building the canvas's scenes off the UI thread.
//!
//! A scene needs work on the CPU that can take long on a large picture:
//! each band's stretch analysis, a layered stretch's maps, Texture's noise
//! measure and the output stretches resolved from the picture. The UI
//! thread hands the worker what it needs, a snapshot of the document and
//! the view, and goes on; a newer request replaces one not yet started.
//! The worker writes the scenes to the canvas, which the render thread
//! draws, and leaves what the UI thread needs to know for it to pick up.

use super::panes::{needs_picture, scene};
use super::{picture_patches, CanvasHandle, PictureRequest, Scene};
use crate::output_stretch::ResolvedChains;
use crate::store_tiles::{BandState, BandStretch, Bands};
use crate::studio_model::{Document, Place};
use seiza_stretch::StretchModel;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

/// What the worker builds scenes from.
pub(super) struct SceneJob {
    pub document: Document,
    pub band_info: Arc<Bands>,
    /// Changes when the bands' analyses must be made again: a new store,
    /// or new samples of it.
    pub bands_generation: u64,
    pub places: Vec<Place>,
    pub view_stretch: Vec<StretchModel>,
    pub picture_samples: Option<(u64, Arc<Vec<f32>>)>,
    pub scene_version: u64,
    pub histogram_wanted: bool,
    pub source_generation: u64,
    pub crop: Option<[f32; 4]>,
    pub document_size: (u32, u32),
}

/// What the UI thread needs to know about the scenes last built.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SceneOutcome {
    /// The sampling of the combined picture the scenes ask for, if any.
    pub picture_wanted: Option<u64>,
    /// The sampling of the final picture the histogram asks for, if any.
    pub final_wanted: Option<u64>,
    /// The brightest value the final picture shows.
    pub final_peak: f32,
}

#[derive(Default)]
struct Pending {
    job: Option<SceneJob>,
    outcome: Option<SceneOutcome>,
}

/// The thread that builds scenes, and what passes between it and the UI.
pub(super) struct SceneWorker {
    shared: Arc<(Mutex<Pending>, Condvar)>,
}

impl SceneWorker {
    pub fn start(canvas: CanvasHandle) -> Self {
        let shared = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("scenes".into())
            .spawn(move || run(&worker, &canvas))
            .expect("spawning the scene worker");
        Self { shared }
    }

    /// Asks for scenes; a request not started yet is replaced.
    pub fn submit(&self, job: SceneJob) {
        let (lock, wake) = &*self.shared;
        lock.lock().expect("scene worker").job = Some(job);
        wake.notify_one();
    }

    /// What the worker last built, once.
    pub fn take_outcome(&self) -> Option<SceneOutcome> {
        self.shared.0.lock().expect("scene worker").outcome.take()
    }
}

/// The worker's own state, kept from one request to the next.
#[derive(Default)]
struct Caches {
    bands_generation: u64,
    /// Stretch analyses already made, by each band's state.
    analysed: Vec<(Vec<BandState>, Arc<Vec<BandStretch>>)>,
    chains: ResolvedChains,
    picture_wanted: Option<u64>,
    final_wanted: Option<u64>,
    final_peak: f32,
}

impl Caches {
    fn analysis(&mut self, bands: &Bands, states: &[BandState]) -> Arc<Vec<BandStretch>> {
        if let Some((_, known)) = self.analysed.iter().find(|(at, _)| at == states) {
            return known.clone();
        }
        let started = Instant::now();
        let stretches = Arc::new(bands.stretches(states).unwrap_or_default());
        if super::timing() {
            eprintln!(
                "scenes: analysed the bands in {:.1} ms",
                started.elapsed().as_secs_f64() * 1e3
            );
        }
        self.analysed
            .insert(0, (states.to_vec(), stretches.clone()));
        self.analysed.truncate(8);
        stretches
    }
}

fn run(shared: &(Mutex<Pending>, Condvar), canvas: &CanvasHandle) {
    let mut caches = Caches {
        final_peak: 1.0,
        ..Caches::default()
    };
    loop {
        let job = {
            let (lock, wake) = shared;
            let mut pending = lock.lock().expect("scene worker");
            loop {
                if let Some(job) = pending.job.take() {
                    break job;
                }
                pending = wake.wait(pending).expect("scene worker");
            }
        };
        let started = Instant::now();
        // Scenes a newer request replaces are not drawn: they may be of a
        // store the canvas no longer shows.
        let superseded = || shared.0.lock().expect("scene worker").job.is_some();
        let outcome = build(&job, &mut caches, canvas, &superseded);
        if super::timing() {
            eprintln!(
                "scenes: built in {:.1} ms",
                started.elapsed().as_secs_f64() * 1e3
            );
        }
        if let Some(outcome) = outcome {
            shared.0.lock().expect("scene worker").outcome = Some(outcome);
        }
    }
}

/// The scenes of `job`'s places, written to the canvas with the samplings
/// they ask for. `None` when a newer request replaces them before they are
/// written.
fn build(
    job: &SceneJob,
    caches: &mut Caches,
    canvas: &CanvasHandle,
    superseded: &dyn Fn() -> bool,
) -> Option<SceneOutcome> {
    if caches.bands_generation != job.bands_generation {
        caches.bands_generation = job.bands_generation;
        caches.analysed.clear();
    }
    let samples = job
        .picture_samples
        .as_ref()
        .map(|(id, samples)| (*id, samples.as_slice()));
    let probe = picture_request(job, caches);
    let mut chains = std::mem::take(&mut caches.chains);
    let scenes: Vec<Scene> = job
        .places
        .iter()
        .map(|place| {
            let (mut scene, states) = scene(
                &job.document,
                &job.band_info,
                &place.view(),
                job.scene_version,
                samples,
                &mut chains,
                &job.view_stretch,
            );
            // The stretch is applied to the corrected samples, so each
            // band's analysis follows its background strength.
            scene.stretches = caches.analysis(&job.band_info, &states);
            scene
        })
        .collect();
    // The histogram counts the final picture as the GPU draws it, at the
    // patches the combined picture is sampled at.
    let final_request = job.histogram_wanted.then_some(()).and_then(|()| {
        let picture = caches.picture_wanted?;
        let (mut scene, states) = scene(
            &job.document,
            &job.band_info,
            &Place::Final.view(),
            job.scene_version,
            samples,
            &mut chains,
            &job.view_stretch,
        );
        scene.stretches = caches.analysis(&job.band_info, &states);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&picture, &mut hasher);
        std::hash::Hash::hash(
            &format!("{:?}{:?}", scene.finish, scene.headroom),
            &mut hasher,
        );
        Some((std::hash::Hasher::finish(&hasher), scene))
    });
    caches.chains = chains;
    let mut final_probe: Option<Option<Arc<PictureRequest>>> = None;
    match final_request {
        Some((id, scene)) if caches.final_wanted != Some(id) => {
            caches.final_wanted = Some(id);
            caches.final_peak = seiza_compose::picture::Picture {
                finish: scene.finish.clone(),
                space: crate::canvas::WORKING_SPACE,
                headroom: scene.headroom,
                ..Default::default()
            }
            .peak();
            let (patches, size) = picture_patches(patch_rect(job));
            final_probe = Some(Some(Arc::new(PictureRequest {
                id,
                scene,
                patches,
                size,
            })));
        }
        Some(_) => {}
        None => {
            caches.final_wanted = None;
            final_probe = Some(None);
        }
    }
    let scenes = Arc::new(scenes);
    if superseded() {
        // The samplings asked for are asked again with the newer scenes.
        caches.picture_wanted = None;
        caches.final_wanted = None;
        return None;
    }
    canvas.edit(|state| {
        if let Some(probe) = probe {
            state.probe = probe;
        }
        if let Some(final_probe) = final_probe {
            state.final_probe = final_probe;
        }
        state.scenes = scenes;
    });
    Some(SceneOutcome {
        picture_wanted: caches.picture_wanted,
        final_wanted: caches.final_wanted,
        final_peak: caches.final_peak,
    })
}

/// The part of the document the picture's samples are read from.
fn patch_rect(job: &SceneJob) -> [f32; 4] {
    let (width, height) = job.document_size;
    job.crop.unwrap_or([0.0, 0.0, width as f32, height as f32])
}

/// The sampling of the combined picture the document asks for: `None` when
/// it asks for the one in hand, `Some(None)` when it asks for none.
#[allow(clippy::option_option)]
fn picture_request(job: &SceneJob, caches: &mut Caches) -> Option<Option<Arc<PictureRequest>>> {
    let wanted = needs_picture(&job.document) || job.histogram_wanted;
    if !wanted {
        return caches.picture_wanted.take().map(|_| None);
    }
    let mut chains = ResolvedChains::default();
    let (mut scene, states) = scene(
        &job.document,
        &job.band_info,
        &Place::Final.view(),
        job.scene_version,
        None,
        &mut chains,
        &job.view_stretch,
    );
    scene.stretches = caches.analysis(&job.band_info, &states);
    let (patches, size) = picture_patches(patch_rect(job));
    let id = scene.picture_id(job.source_generation, &states, job.crop);
    if caches.picture_wanted == Some(id) {
        return None;
    }
    caches.picture_wanted = Some(id);
    Some(Some(Arc::new(PictureRequest {
        id,
        scene,
        patches,
        size,
    })))
}
