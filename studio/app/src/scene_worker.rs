//! Building the canvas's scenes off the UI thread.
//!
//! A scene needs work on the CPU that can take long on a large picture:
//! each band's stretch analysis, a layered stretch's maps, Texture's noise
//! measure and the output stretches resolved from the picture. The UI
//! thread hands the worker what it needs, a snapshot of the document and
//! the view, and goes on; a newer request replaces one not yet started.
//! The worker writes the scenes to the canvas, which the render thread
//! draws, and leaves what the UI thread needs to know for it to pick up.

use super::panes::{beneath_other_layers, needs_picture, scene};
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
    /// The store and the planes worked out from it, which the samples of
    /// what is beneath a stretch are rendered from.
    pub store: Option<std::path::PathBuf>,
    pub planes: Vec<Arc<crate::derived::DerivedPlane>>,
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
    /// Whether the worker is building a job's scenes now.
    building: bool,
}

/// The thread that builds scenes, and what passes between it and the UI.
pub(super) struct SceneWorker {
    shared: Arc<(Mutex<Pending>, Condvar)>,
    beneath: Arc<BeneathSampler>,
}

/// Samples of the picture beneath output stretches that have other layers
/// above them, rendered on the CPU at the patches the canvas samples, one
/// sampling at a time on a thread of its own.
#[derive(Default)]
struct BeneathSampler {
    state: Mutex<BeneathState>,
    /// Set when a sampling is done, until the UI rebuilds the scenes.
    fresh: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct BeneathState {
    /// Samplings done, newest first, by the id of what they sampled.
    ready: Vec<(u64, Arc<Vec<f32>>)>,
    /// The id being sampled now.
    running: Option<u64>,
    /// The newest sampling asked for while another runs.
    queued: Option<BeneathJob>,
}

/// One sampling of the picture beneath a stretch.
struct BeneathJob {
    id: u64,
    picture: seiza_compose::picture::Picture,
    store: std::path::PathBuf,
    planes: Vec<Arc<crate::derived::DerivedPlane>>,
    patches: Vec<[u32; 2]>,
    size: u32,
}

impl BeneathSampler {
    /// The samples of `id`, when they are done.
    fn ready(&self, id: u64) -> Option<Arc<Vec<f32>>> {
        let state = self.state.lock().expect("beneath samples");
        state
            .ready
            .iter()
            .find(|(known, _)| *known == id)
            .map(|(_, samples)| samples.clone())
    }

    /// Starts sampling `job`, or keeps it to start next: the newest asked
    /// for wins.
    fn request(self: &Arc<Self>, job: BeneathJob) {
        let mut state = self.state.lock().expect("beneath samples");
        if state.running == Some(job.id) || state.ready.iter().any(|(id, _)| *id == job.id) {
            return;
        }
        if state.running.is_some() {
            state.queued = Some(job);
            return;
        }
        state.running = Some(job.id);
        drop(state);
        let sampler = self.clone();
        std::thread::Builder::new()
            .name("beneath samples".into())
            .spawn(move || sampler.run(job))
            .expect("spawning a sampling");
    }

    fn run(self: Arc<Self>, mut job: BeneathJob) {
        loop {
            let started = Instant::now();
            let samples = sample_patches(&job);
            if super::timing() {
                eprintln!(
                    "scenes: sampled what is beneath a stretch in {:.1} ms",
                    started.elapsed().as_secs_f64() * 1e3
                );
            }
            let mut state = self.state.lock().expect("beneath samples");
            if let Some(samples) = samples {
                state.ready.insert(0, (job.id, Arc::new(samples)));
                state.ready.truncate(8);
                self.fresh.store(true, std::sync::atomic::Ordering::Release);
            }
            match state.queued.take() {
                Some(next) => {
                    state.running = Some(next.id);
                    job = next;
                }
                None => {
                    state.running = None;
                    return;
                }
            }
        }
    }
}

/// The picture of `job` at its patches, rendered on the CPU: red, green and
/// blue interleaved, patch after patch, as the canvas samples them. `None`
/// when the store cannot be read.
fn sample_patches(job: &BeneathJob) -> Option<Vec<f32>> {
    use seiza_compose::render::{render_image, RenderOptions};
    let (tiles, _) = crate::store_tiles::open_store(&job.store).ok()?;
    let source = crate::store_tiles::StoreSource::new(tiles).with_planes(job.planes.clone());
    let size = job.size as usize;
    let mut samples = Vec::with_capacity(job.patches.len() * size * size * 3);
    for origin in &job.patches {
        let region = seiza_compose::ReferenceRegion {
            x: origin[0] as usize,
            y: origin[1] as usize,
            width: size,
            height: size,
        };
        let image = render_image(&job.picture, &source, region, &RenderOptions::default()).ok()?;
        if image.channels == 3 {
            samples.extend_from_slice(&image.data);
        } else {
            samples.extend(image.data.iter().flat_map(|value| [*value; 3]));
        }
    }
    Some(samples)
}

impl SceneWorker {
    pub fn start(canvas: CanvasHandle) -> Self {
        let shared = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker = shared.clone();
        let beneath = Arc::new(BeneathSampler::default());
        std::thread::Builder::new()
            .name("scenes".into())
            .spawn({
                let beneath = beneath.clone();
                move || run(&worker, &canvas, &beneath)
            })
            .expect("spawning the scene worker");
        Self { shared, beneath }
    }

    /// Whether samples of what is beneath a stretch came in since this was
    /// last asked: the scenes are then built again.
    pub fn take_beneath_fresh(&self) -> bool {
        self.beneath
            .fresh
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    /// Asks for scenes; a request not started yet is replaced.
    pub fn submit(&self, job: SceneJob) {
        let (lock, wake) = &*self.shared;
        lock.lock().expect("scene worker").job = Some(job);
        wake.notify_one();
    }

    /// Whether a request waits or is being built.
    pub fn busy(&self) -> bool {
        let pending = self.shared.0.lock().expect("scene worker");
        pending.job.is_some() || pending.building
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

fn run(shared: &(Mutex<Pending>, Condvar), canvas: &CanvasHandle, beneath: &Arc<BeneathSampler>) {
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
                    pending.building = true;
                    break job;
                }
                pending = wake.wait(pending).expect("scene worker");
            }
        };
        let started = Instant::now();
        // Scenes a newer request replaces are not drawn: they may be of a
        // store the canvas no longer shows.
        let superseded = || shared.0.lock().expect("scene worker").job.is_some();
        prepare_beneath(&job, &mut caches, beneath);
        let outcome = build(&job, &mut caches, canvas, &superseded);
        if super::timing() {
            eprintln!(
                "scenes: built in {:.1} ms",
                started.elapsed().as_secs_f64() * 1e3
            );
        }
        let mut pending = shared.0.lock().expect("scene worker");
        pending.building = false;
        if let Some(outcome) = outcome {
            pending.outcome = Some(outcome);
        }
    }
}

/// Puts the samples of what is beneath each output stretch that has other
/// layers above it into the chains, bottom first, and asks for the first
/// sampling not done. A stretch above one still waiting waits too.
fn prepare_beneath(job: &SceneJob, caches: &mut Caches, sampler: &Arc<BeneathSampler>) {
    let wanted = beneath_other_layers(&job.document);
    caches
        .chains
        .beneath
        .retain(|step, _| wanted.contains(step));
    let Some(store) = job.store.clone() else {
        return;
    };
    let layers = job.document.layers();
    for (at, layer) in layers.iter().enumerate() {
        let Some(step) = layer.source.adjustment_step() else {
            continue;
        };
        if !wanted.contains(&step.id) {
            continue;
        }
        let needs = match &step.kind {
            crate::studio_model::StepKind::Operation { settings, .. } => {
                crate::output_stretch::config_of(settings)
                    .is_some_and(|config| crate::output_stretch::needs_samples(&config))
            }
            _ => false,
        };
        if !needs {
            continue;
        }
        // What is beneath it: the layers up to the one below it, or in a
        // group its members up to the one below it.
        let Some(below) = at
            .checked_sub(1)
            .filter(|below| layers[*below].group == layer.group)
        else {
            continue;
        };
        let subject = match layer.group {
            Some(_) => crate::studio_model::Subject::MembersUpTo(layers[below].id),
            None => crate::studio_model::Subject::LayersUpTo(layers[below].id),
        };
        let view = crate::studio_model::View {
            subject,
            up_to: crate::studio_model::UpTo::EndOf(crate::studio_model::StageKind::Combine),
            stretch: crate::studio_model::DisplayStretch::Document,
            own_until: None,
        };
        let (mut beneath, states) = scene(
            &job.document,
            &job.band_info,
            &view,
            job.scene_version,
            None,
            &mut caches.chains,
            &job.view_stretch,
        );
        // A stretch beneath this one waits for its samples first.
        if beneath
            .omitted
            .iter()
            .any(|words| words.contains("until the layers beneath it have been sampled"))
        {
            return;
        }
        beneath.stretches = caches.analysis(&job.band_info, &states);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(
            &beneath.picture_id(job.source_generation, &states, job.crop),
            &mut hasher,
        );
        std::hash::Hash::hash(&beneath.finish_words(), &mut hasher);
        let id = std::hash::Hasher::finish(&hasher);
        match sampler.ready(id) {
            Some(samples) => {
                caches.chains.beneath.insert(step.id, (id, samples));
            }
            None => {
                let (patches, size) = picture_patches(patch_rect(job));
                sampler.request(BeneathJob {
                    id,
                    picture: beneath.picture(&[]),
                    store,
                    planes: job.planes.clone(),
                    patches,
                    size,
                });
                return;
            }
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
            scene.place = Some(place.clone());
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
    // Stretches beneath other layers are drawn in the sampled picture, from
    // the samples of what is beneath them.
    let mut chains = ResolvedChains::default();
    chains.beneath = caches.chains.beneath.clone();
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
