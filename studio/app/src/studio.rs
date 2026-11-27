use crate::bench::Bench;
use crate::canvas::{ComputedMask, Layer, SceneParts};
use crate::store_tiles::{BandState, BandStretch, Bands};
use crate::studio_model::{CropRecord, Document, Place, Row, Split, Stroke, StrokeMode};
use gpui::{
    prelude::*, px, rgb, size, App, Bounds, Context, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, NativeView, Pixels, Point, ScrollDelta, ScrollEvent, Window, WindowBounds,
    WindowOptions,
};
use objc2::{
    class, msg_send,
    rc::{Allocated, Retained},
    runtime::AnyObject,
};
use psf_studio_render::{pane_rect, Frame, FrameStats, Output, Pane, Renderer, Sampled, View};
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use seiza_compose::gpu::{GpuInput, GpuScene, MAX_LAYERS};
use seiza_compose::picture::{FinishStep, LayerHeal, Picture, Segment, Transfer};
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub struct Options {
    pub bench_output: Option<PathBuf>,
    pub bench_frames: u32,
    pub vsync: bool,
    /// The surface to present to: `srgb8` (the default), `p3-8`, `p3-float`
    /// or `p3-extended`.
    pub output: String,
    /// The ICC profile of the document's working space. With one, the canvas
    /// converts from it to the surface's color space.
    pub working_profile: Option<PathBuf>,
    /// A tile store to open in place of the synthetic document.
    pub open: Option<PathBuf>,
    /// A PSF-Studio document to open with the store, in place of PSF
    /// Guard's default flow over the store's inputs.
    pub document: Option<PathBuf>,
    /// The `psf-guard` program, which lists the processing tools and
    /// prepares a store again when a person runs them.
    pub psf_guard: Option<PathBuf>,
    /// Where the benchmark writes PNG captures of selected frames.
    pub capture_prefix: Option<PathBuf>,
    /// Where to write how far the CPU render of the picture's sampled
    /// patches is from the canvas's, once the canvas has sampled them.
    pub parity_report: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            bench_output: None,
            bench_frames: 600,
            vsync: true,
            capture_prefix: None,
            open: None,
            document: None,
            psf_guard: None,
            output: String::new(),
            working_profile: None,
            parity_report: None,
        }
    }
}

const PANEL_WIDTH: f32 = 320.0;
const STEPS_WIDTH: f32 = 300.0;

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Pan,
    Brush,
    Heal,
    /// Draws the rectangle the picture keeps.
    Crop,
}

/// A slider in a pane.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum SliderId {
    /// One setting of the selected stretch step, by its place in the list
    /// of that stretch's settings.
    StretchParam(usize),
    /// The selected layer's opacity.
    Opacity,
    /// The selected layer's hue.
    Hue,
    /// The selected layer's saturation.
    Saturation,
    /// The selected step's amount.
    Amount,
    /// The selected adjustment layer's amount.
    LayerAmount,
    /// One setting of one processing tool of the selected input, by its
    /// place in the list of the input's tool settings.
    ToolParam(usize),
    /// One control of the selected Tone and color step, by its place in
    /// [`seiza_compose::tone::CONTROLS`].
    FinishParam(usize),
    /// The selected shared mask's reach.
    Reach,
    /// How far the selected shared mask is smoothed.
    Smoothing,
    /// The brush's size on screen.
    BrushSize,
    /// How far the selected painted mask's strokes are softened.
    MaskBlur,
    /// The hue of one layer, by its place in the stack, in the palette's
    /// list of tones.
    ToneHue(usize),
    /// The saturation of one layer, by its place in the stack, in the
    /// palette's list of tones.
    ToneSaturation(usize),
}

impl SliderId {
    const ALL: [SliderId; 9] = [
        SliderId::Opacity,
        SliderId::Hue,
        SliderId::Saturation,
        SliderId::Amount,
        SliderId::LayerAmount,
        SliderId::Reach,
        SliderId::Smoothing,
        SliderId::BrushSize,
        SliderId::MaskBlur,
    ];
}

#[derive(Clone, Copy)]
enum Drag {
    Slider(SliderId),
    Canvas(Point<Pixels>),
    /// The divider of a wipe.
    Divider,
}

/// What the canvas draws: the view, the band settings and the strokes.
///
/// The UI thread owns one copy and edits it. The render thread keeps its own
/// copy and draws from that, so neither waits for the other.
pub struct CanvasState {
    pub view: View,
    /// One per band, in 0..=1: what the benchmark moves to time a stretch
    /// change. A person's sliders write the document instead.
    pub sliders: [f32; MAX_LAYERS],
    /// The document's size in pixels.
    pub document_size: (u32, u32),
    /// The number of bands in the document.
    pub band_count: usize,
    /// Where each tile channel is on the GPU, for Seiza's compositor.
    pub inputs: Arc<Vec<GpuInput>>,
    /// What the document asks the canvas to draw: one scene per pane, left
    /// to right.
    scenes: Arc<Vec<Scene>>,
    /// For a wipe: where the divider is, as a fraction of the surface's
    /// width. The two scenes then share one pane, the first on its left.
    pub wipe: Option<f32>,
    /// Strokes shown over the image: a stroke being drawn, or the selected
    /// step's mask.
    pub strokes: Vec<Segment>,
    /// Counts the times `strokes` was replaced rather than grown, so the
    /// render thread copies all of it then, and only the new tail while a
    /// stroke is drawn.
    pub strokes_replaced: u64,
    pub surface_size: (u32, u32),
    /// The part of the document the picture keeps, as left, top, right and
    /// bottom in document pixels, or `None` for all of it.
    pub crop: Option<[f32; 4]>,
    fitted: bool,
    /// The name of a capture to take on the next frame.
    pub capture: Option<String>,
    /// Set to measure the GPU's cost of the next frame with no display.
    pub measure_gpu: bool,
    pub gpu_frame_ms: Option<f32>,
    /// The combined picture to sample for the output stretches that Seiza
    /// resolves from the picture, when the document has one.
    probe: Option<Arc<PictureRequest>>,
}

/// A sampling of the combined picture that the UI asks the render thread
/// for: the scene of the whole picture, and where its patches are.
pub struct PictureRequest {
    /// Changes when anything that changes the combined picture does.
    id: u64,
    scene: Scene,
    /// The top-left corner of each patch, in document pixels.
    patches: Vec<[u32; 2]>,
    /// Each patch's width and height, in document pixels.
    size: u32,
}

/// How many patches a sampling of the picture reads: an eight by eight grid.
pub(super) const PICTURE_PATCHES: usize = 64;

/// Patches of the combined picture sampled for its stretch: an eight by
/// eight grid of squares spread across `rect`, as each input is sampled.
pub(super) fn picture_patches(rect: [f32; 4]) -> (Vec<[u32; 2]>, u32) {
    let [left, top, right, bottom] = rect.map(|value| value.max(0.0) as u32);
    let (width, height) = (right.saturating_sub(left), bottom.saturating_sub(top));
    let size = 128.min(width).min(height).max(1);
    let mut patches = Vec::with_capacity(PICTURE_PATCHES);
    for row in 0..8 {
        for column in 0..8 {
            patches.push([
                left + (width.saturating_sub(size)) * column / 7,
                top + (height.saturating_sub(size)) * row / 7,
            ]);
        }
    }
    (patches, size)
}

impl CanvasState {
    fn new() -> Self {
        Self {
            view: View {
                center: [0.0, 0.0],
                zoom: 1.0,
            },
            // A third of the way is at rest.
            sliders: [1.0 / 3.0; MAX_LAYERS],
            document_size: (1, 1),
            band_count: 0,
            inputs: Arc::default(),
            scenes: Arc::new(vec![Scene::default()]),
            wipe: None,
            strokes: Vec::new(),
            strokes_replaced: 0,
            surface_size: (1, 1),
            crop: None,
            fitted: false,
            capture: None,
            measure_gpu: false,
            gpu_frame_ms: None,
            probe: None,
        }
    }

    /// A wipe's divider, in surface pixels from the left.
    pub fn divider(&self) -> Option<u32> {
        let fraction = self.wipe.filter(|_| self.scenes.len() == 2)?;
        Some((fraction.clamp(0.02, 0.98) * self.surface_size.0 as f32) as u32)
    }

    /// Where pane `index` is in the surface, each as x, y, width and height
    /// in pixels: where its image is laid out, and the part it draws to.
    /// The two differ for a wipe, whose scenes share one layout.
    pub fn pane_rects(&self, index: usize) -> ([u32; 4], [u32; 4]) {
        let (width, height) = self.surface_size;
        match self.divider() {
            Some(divider) => {
                let whole = [0, 0, width, height];
                let part = if index == 0 {
                    [0, 0, divider.saturating_sub(1), height]
                } else {
                    [divider + 1, 0, width.saturating_sub(divider + 1), height]
                };
                (whole, part)
            }
            None => {
                let rect = pane_rect(index, self.scenes.len(), self.surface_size);
                (rect, rect)
            }
        }
    }

    /// Where pane `index`'s image is laid out in the surface.
    pub fn pane(&self, index: usize) -> [u32; 4] {
        self.pane_rects(index).0
    }

    /// What a fit shows: the crop, or the whole document.
    fn fitted_rect(&self) -> [f64; 4] {
        match self.crop {
            Some(crop) => crop.map(f64::from),
            None => [
                0.0,
                0.0,
                self.document_size.0 as f64,
                self.document_size.1 as f64,
            ],
        }
    }

    pub fn fit_zoom(&self) -> f64 {
        let [_, _, width, height] = self.pane(0);
        let [left, top, right, bottom] = self.fitted_rect();
        (width as f64 / (right - left).max(1.0)).min(height as f64 / (bottom - top).max(1.0))
    }

    pub fn fit(&mut self) {
        let [left, top, right, bottom] = self.fitted_rect();
        self.view = View {
            center: [(left + right) / 2.0, (top + bottom) / 2.0],
            zoom: self.fit_zoom(),
        };
    }

    /// Each pane's scene packed for the GPU, with its stretches: Seiza's
    /// stretch of each layer's band, with the background placed where the
    /// scene says.
    fn gpu_scenes(&self) -> Vec<GpuScene> {
        self.scenes
            .iter()
            .map(|scene| GpuScene::new(&scene.picture(&self.nudges()), &self.inputs))
            .collect()
    }

    /// What the benchmark adds to each band's background target, to time a
    /// stretch change: nothing while its slider is at rest, a third of the
    /// way.
    fn nudges(&self) -> [f64; MAX_LAYERS] {
        self.sliders
            .map(|slider| 0.45 * f64::from(slider - 1.0 / 3.0))
    }
}

/// What the document asks the canvas to draw: the Combine stage's layers and
/// the Finish stage's steps with their masks.
#[derive(Default)]
pub struct Scene {
    layers: Vec<Layer>,
    /// The document band each stretch is of, in order: one per layer that
    /// shows a band, then the ones only computed masks read.
    layer_bands: Vec<usize>,
    /// The stretches, in the same order. None is a preview stretch.
    layer_stretches: Vec<Vec<seiza_stretch::StretchModel>>,
    finish: Vec<FinishStep>,
    /// Heals of single layers and inputs.
    heals: Vec<LayerHeal>,
    /// The masks computed from inputs that the layers refer to.
    computed: Vec<ComputedMask>,
    masks: Vec<Vec<Segment>>,
    /// What the scene leaves out because one frame cannot draw more.
    omitted: Vec<String>,
    /// Each band's stretch analysis, at the background strength this scene
    /// draws the band with.
    stretches: Arc<Vec<BandStretch>>,
    /// Different for every scene, so the renderer uploads masks once.
    version: u64,
}

impl Scene {
    /// The scene's stretches as Seiza transfers, in order, with `nudges`
    /// added to each band's background target. A stretch whose band has
    /// no analysis yet draws nothing.
    fn transfers(&self, nudges: &[f64]) -> Vec<Transfer> {
        self.layer_bands
            .iter()
            .zip(&self.layer_stretches)
            .map(|(band, stages)| {
                // A plane the store stretched is shown as it is.
                if *band == panes::DISPLAY_ROW {
                    return Transfer::identity();
                }
                let nudge = nudges.get(*band).copied().unwrap_or(0.0);
                self.stretches.get(*band).map_or_else(
                    || Transfer::of([seiza_compose::picture::TransferStage::Scale(0.0)]),
                    |stretch| stretch.transfer(stages, nudge),
                )
            })
            .collect()
    }

    /// The scene as the Seiza picture the canvas draws, with each band's
    /// stretch moved by `nudges`.
    pub(super) fn picture(&self, nudges: &[f64]) -> Picture {
        crate::canvas::picture(
            &SceneParts {
                layers: &self.layers,
                computed: &self.computed,
                masks: &self.masks,
                heals: &self.heals,
                finish: &self.finish,
            },
            &self.transfers(nudges),
        )
    }

    /// The combined picture, before the Finish steps: what the canvas
    /// samples for the output stretches resolved from it.
    pub(super) fn combined(&self, nudges: &[f64]) -> Picture {
        Picture {
            finish: Vec::new(),
            ..self.picture(nudges)
        }
    }
}

/// What the render thread reports back for the panel.
#[derive(Default)]
struct Readout {
    stats: FrameStats,
    intervals: VecDeque<f32>,
    input_to_submit_ms: VecDeque<f32>,
    bench: Option<String>,
    error: Option<String>,
    /// What the canvas's Metal layer is set to, read back from AppKit.
    layer: Option<String>,
}

struct Shared {
    canvas: CanvasState,
    /// A document for the canvas to draw from now on, in place of the one
    /// it has: set by the UI, taken by the render thread.
    new_source: Option<Arc<dyn psf_studio_render::TileSource>>,
    /// Set by the UI when the canvas changed; cleared by the render thread.
    dirty: bool,
    /// When the oldest input not yet drawn arrived.
    input_at: Option<Instant>,
    readout: Readout,
    /// The samples of the combined picture last read, with the id of the
    /// request they answer: set by the render thread, read by the UI.
    picture: Option<(u64, Arc<Vec<f32>>)>,
}

/// The UI thread's side of the canvas.
#[derive(Clone)]
struct CanvasHandle(Arc<(Mutex<Shared>, Condvar)>);

impl CanvasHandle {
    /// Changes the canvas and wakes the render thread.
    fn edit<R>(&self, edit: impl FnOnce(&mut CanvasState) -> R) -> R {
        let mut shared = self.0 .0.lock().expect("canvas state");
        let result = edit(&mut shared.canvas);
        shared.dirty = true;
        shared.input_at.get_or_insert_with(Instant::now);
        self.0 .1.notify_one();
        result
    }

    fn read<R>(&self, read: impl FnOnce(&Shared) -> R) -> R {
        read(&self.0 .0.lock().expect("canvas state"))
    }
}

/// Draws frames until the process exits. wgpu waits for the display here, so
/// the UI thread never does.
fn render_thread(handle: CanvasHandle, mut renderer: Renderer, options: Options) {
    let (lock, wake) = &*handle.0;
    let mut canvas = CanvasState::new();
    let mut bench = options
        .bench_output
        .is_some()
        .then(|| Bench::new(options.bench_frames));
    let mut last_frame: Option<Instant> = None;
    let mut gpu_scenes: Vec<GpuScene> = Vec::new();
    let mut stretched_for = None;
    let mut stretches_version = 0u64;
    let mut animate = true;
    // The sampling of the combined picture last delivered, and the scene
    // of the one under way, packed for the GPU.
    let mut sampled: Option<u64> = None;
    let mut probe_scene: Option<(u64, GpuScene)> = None;
    loop {
        let input_at = {
            let mut shared = lock.lock().expect("canvas state");
            while !shared.dirty && !animate {
                shared = wake.wait(shared).expect("canvas state");
                last_frame = None;
            }
            shared.dirty = false;
            if let Some(source) = shared.new_source.take() {
                renderer.replace_source(source);
            }
            canvas.surface_size = shared.canvas.surface_size;
            canvas.document_size = shared.canvas.document_size;
            canvas.band_count = shared.canvas.band_count;
            canvas.inputs = shared.canvas.inputs.clone();
            canvas.scenes = shared.canvas.scenes.clone();
            canvas.probe = shared.canvas.probe.clone();
            canvas.wipe = shared.canvas.wipe;
            canvas.crop = shared.canvas.crop;
            if !shared.canvas.fitted && canvas.surface_size.0 > 1 {
                canvas.fit();
                shared.canvas.view = canvas.view;
                shared.canvas.fitted = true;
            } else if bench.is_none() {
                canvas.view = shared.canvas.view;
            }
            if bench.is_none() {
                canvas.sliders = shared.canvas.sliders;
                // Strokes grow while one is drawn; otherwise they are
                // replaced, and copied whole.
                if shared.canvas.strokes_replaced != canvas.strokes_replaced
                    || shared.canvas.strokes.len() < canvas.strokes.len()
                {
                    canvas.strokes.clear();
                    canvas.strokes_replaced = shared.canvas.strokes_replaced;
                }
                let known = canvas.strokes.len();
                canvas
                    .strokes
                    .extend_from_slice(&shared.canvas.strokes[known..]);
            }
            shared.input_at.take()
        };
        let (width, height) = canvas.surface_size;
        if width <= 1 || height <= 1 {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        renderer.resize(width, height);

        if let Some(bench) = &mut bench {
            bench.before_frame(&mut canvas);
        }
        // The scenes are packed again when a slider or the scenes change.
        let version = canvas.scenes.first().map_or(0, |scene| scene.version);
        let stretch_inputs = (canvas.sliders, version);
        if stretched_for != Some(stretch_inputs) {
            gpu_scenes = canvas.gpu_scenes();
            stretched_for = Some(stretch_inputs);
            stretches_version += 1;
        }
        let panes: Vec<Pane> = gpu_scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                let (viewport, scissor) = canvas.pane_rects(index);
                Pane {
                    scene,
                    // Each pane's packing is its own. The compositor keeps
                    // one pane's tables and masks, so a single pane uploads
                    // them only when they change; with several panes they
                    // go up for each pane every frame.
                    version: Some(stretches_version * 16 + index as u64),
                    viewport,
                    scissor,
                }
            })
            .collect();
        let frame = Frame {
            view: canvas.view,
            panes: &panes,
            overlay: &canvas.strokes,
            overlay_strength: 1.0,
            crop: canvas.crop,
        };
        let capture = canvas.capture.take();
        renderer.capture_next = capture.is_some() && options.capture_prefix.is_some();
        let result = renderer.render(&frame);
        if let (Some(name), Some(prefix), Some((width, height, pixels))) =
            (capture, &options.capture_prefix, renderer.captured.take())
        {
            let name: String = name
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .collect();
            let path = format!("{}-{name}.png", prefix.display());
            if let Err(error) = write_png(&path, width, height, &pixels) {
                eprintln!("could not write {path}: {error}");
            }
        }
        if std::mem::take(&mut canvas.measure_gpu) {
            canvas.gpu_frame_ms = Some(renderer.offscreen_frame_ms(40));
        }
        let now = Instant::now();
        let interval = last_frame
            .replace(now)
            .map(|last| (now - last).as_secs_f32() * 1000.0);

        let stats = match result {
            Ok(stats) => stats,
            Err(error) => {
                // The surface has no texture while the window is covered.
                lock.lock().expect("canvas state").readout.error = Some(error);
                std::thread::sleep(Duration::from_millis(16));
                continue;
            }
        };
        animate = stats.tiles_coarse > 0 || stats.tiles_in_flight > 0 || stats.tiles_uploaded > 0;
        // A sampling no document asks for any more is stopped.
        if canvas.probe.is_none() && probe_scene.take().is_some() {
            renderer.stop_sampling();
            sampled = None;
        }
        // The combined picture is sampled between frames, a few patches at
        // a time, until every patch is read.
        if let Some(request) = canvas.probe.clone()
            && sampled != Some(request.id)
        {
            if probe_scene.as_ref().is_none_or(|(id, _)| *id != request.id) {
                let combined = request.scene.combined(&canvas.nudges());
                probe_scene = Some((request.id, GpuScene::new(&combined, &canvas.inputs)));
            }
            let scene = &probe_scene.as_ref().expect("the request's scene").1;
            match renderer.sample_picture(
                request.id,
                scene,
                canvas.crop,
                &request.patches,
                request.size,
            ) {
                Sampled::Ready(samples) => {
                    sampled = Some(request.id);
                    lock.lock().expect("canvas state").picture =
                        Some((request.id, Arc::new(samples)));
                }
                Sampled::Pending { .. } => animate = true,
            }
        }
        if let Some(bench) = &mut bench {
            bench.after_frame(&mut canvas, &stats);
            animate = true;
            if bench.finished {
                let report = bench.report(
                    &canvas,
                    &format!(
                        "{}; surface offers {}",
                        renderer.adapter_summary(),
                        renderer.surface_offers()
                    ),
                    options.vsync,
                    renderer.mean_tile_ms(),
                    lock.lock().expect("canvas state").readout.layer.clone(),
                );
                if let Some(path) = &options.bench_output {
                    let text = serde_json::to_string_pretty(&report).expect("a JSON report");
                    if let Err(error) = std::fs::write(path, text) {
                        eprintln!("could not write {}: {error}", path.display());
                    }
                }
                std::process::exit(0);
            }
        }

        let mut shared = lock.lock().expect("canvas state");
        let readout = &mut shared.readout;
        readout.error = None;
        readout.stats = stats;
        readout.bench = bench.as_ref().map(Bench::label);
        if let Some(interval) = interval {
            push(&mut readout.intervals, interval);
        }
        if let Some(input_at) = input_at {
            push(
                &mut readout.input_to_submit_ms,
                (now - input_at).as_secs_f32() * 1000.0,
            );
        }
    }
}

pub struct Studio {
    native_view: NativeView,
    /// Keeps the `NSView` that wgpu draws into alive.
    _ns_view: Retained<AnyObject>,
    canvas: CanvasHandle,
    canvas_bounds: Rc<Cell<Bounds<Pixels>>>,
    slider_bounds: HashMap<SliderId, Rc<Cell<Bounds<Pixels>>>>,
    tool: Tool,
    drag: Option<Drag>,
    scale_factor: f32,
    /// The document the panes show and edit.
    document: Document,
    /// The selected row of the tree: what the right pane edits, and what
    /// a pane that is not pinned shows.
    selected: Row,
    /// Whether a selected layer is shown without the layers beneath it.
    alone: bool,
    /// Whether the selected row's mask is drawn over the image.
    mask_shown: bool,
    /// Whether the brush takes away from a mask, in place of adding to it.
    brush_removes: bool,
    /// The brush's radius on screen, in points.
    brush_size: f32,
    /// Where a crop being drawn started, in document pixels.
    crop_from: Option<[f64; 2]>,
    /// Whether the canvas shows the final picture while a mask is selected,
    /// in place of the mask itself.
    mask_shows_picture: bool,
    /// Where a wipe's divider is, as a fraction of the canvas's width.
    wipe: f32,
    /// How the canvas is divided.
    split: Split,
    /// Per pane: the place a pinned pane keeps while the selection moves.
    pins: Vec<Option<Place>>,
    /// What each pane shows now, left to right.
    places: Vec<Place>,
    /// The inputs whose steps a person opened in the tree.
    unfolded: HashSet<String>,
    /// The menu that is open, by its key.
    open_menu: Option<String>,
    /// The window's title as last set.
    window_title: String,
    /// The sections a person folded or unfolded, by their keys.
    toggled: HashSet<&'static str>,
    /// The brush stroke being drawn.
    stroke: Option<Stroke>,
    /// Counts the scenes sent to the canvas.
    scene_version: u64,
    /// Where each band is in a tile, and its samples for the stretch.
    band_info: Arc<Bands>,
    /// Stretch analyses already made, by each band's background strength.
    analysed: Vec<(Vec<BandState>, Arc<Vec<BandStretch>>)>,
    /// The latest samples of the combined picture, with the id of the
    /// sampling they came from, for the output stretches Seiza resolves
    /// from the picture.
    picture_samples: Option<(u64, Arc<Vec<f32>>)>,
    /// The id of the sampling last asked for.
    picture_wanted: Option<u64>,
    /// Counts the stores shown, so a sampling of the picture follows a store
    /// that is prepared again.
    source_generation: u64,
    /// Output stretches resolved lately.
    resolved_chains: crate::output_stretch::ResolvedChains,
    /// The histogram of the final picture, while the inspector shows it.
    histogram: Option<Arc<crate::histogram::Histogram>>,
    /// The export of the final picture under way, if any.
    export: Option<Arc<export::ExportRun>>,
    /// What the last export said: how far it has got, or how it ended.
    export_line: String,
    /// Where to write the parity report, until it is written.
    parity_report: Option<PathBuf>,
    /// The ICC profile of the document's working space, which an export
    /// embeds, when Studio was given one.
    working_profile: Option<PathBuf>,
    /// The levels in the document's pyramid.
    levels: u8,
    /// The processing tools: which this computer has, and the run in hand.
    tools: tools::Tools,
}

impl Studio {
    fn new(options: Options, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let parity_report = options.parity_report.clone();
        let working_profile = options.working_profile.clone();
        // SAFETY: `NSView` is an AppKit class, windows are opened on the main
        // thread, and the new view is in no other view hierarchy.
        let (native_view, ns_view) = unsafe {
            let view: Allocated<AnyObject> = msg_send![class!(NSView), alloc];
            let view: Retained<AnyObject> = msg_send![view, init];
            let native_view = NativeView::new(NonNull::from(&*view).cast(), window, cx)
                .expect("macOS windows composite native views");
            (native_view, view)
        };
        native_view.set_background(rgb(0x08080a), cx);

        let canvas = CanvasHandle(Arc::new((
            Mutex::new(Shared {
                canvas: CanvasState::new(),
                new_source: None,
                dirty: true,
                input_at: None,
                readout: Readout::default(),
                picture: None,
            }),
            Condvar::new(),
        )));

        let instance = wgpu::Instance::default();
        // SAFETY: the view outlives the surface, because `Studio` holds both
        // and drops the renderer's owner, `canvas`, with itself.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(RawDisplayHandle::AppKit(AppKitDisplayHandle::new())),
                raw_window_handle: RawWindowHandle::AppKit(AppKitWindowHandle::new(
                    NonNull::from(&*ns_view).cast(),
                )),
            })
        };
        let workers = std::thread::available_parallelism()
            .map(|count| count.get().saturating_sub(4).max(2))
            .unwrap_or(4);
        // The document the picture keeps: the one given, or the one beside
        // the store. It is read from, and saved to, the same path.
        let document_path = options
            .document
            .clone()
            .or_else(|| Some(options.open.as_ref()?.with_extension("json")));
        let tools = tools::Tools::new(&options);
        let source = crate::store_tiles::source(options.open.as_deref());
        let (source, band_info) = match source {
            Ok((source, bands)) => (Ok(source), Arc::new(bands)),
            Err(error) => (Err(error), Arc::<Bands>::default()),
        };
        let bands = band_info.len();
        let mut levels = 1;
        let renderer = surface
            .map_err(|error| error.to_string())
            .and_then(|surface| Ok((surface, output(&options)?, source?)))
            .and_then(|(surface, output, source)| {
                pollster::block_on(Renderer::new(
                    &instance,
                    surface,
                    2 << 30,
                    workers,
                    source,
                    output,
                ))
            });
        match renderer {
            Ok(renderer) => {
                let pyramid = renderer.pyramid();
                levels = pyramid.levels;
                {
                    let mut shared = canvas.0 .0.lock().expect("canvas state");
                    shared.canvas.document_size = (pyramid.width, pyramid.height);
                    shared.canvas.band_count = bands;
                    shared.canvas.inputs = Arc::new(band_info.gpu_inputs());
                }
                let handle = canvas.clone();
                std::thread::Builder::new()
                    .name("canvas".into())
                    .spawn(move || render_thread(handle, renderer, options))
                    .expect("spawning the render thread");
            }
            Err(error) => canvas.0 .0.lock().expect("canvas state").readout.error = Some(error),
        }

        // The render thread draws on its own; the panel's numbers follow it.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let updated = this.update(cx, |this, cx| {
                this.follow_tools();
                this.follow_links();
                this.follow_picture();
                this.follow_export();
                // Every change is kept: the document is saved as soon as it
                // differs from the file.
                this.save_document();
                // SAFETY: the view is alive and this runs on the main thread.
                let layer = unsafe { layer_description(&this._ns_view) };
                this.canvas.0 .0.lock().expect("canvas state").readout.layer = layer;
                cx.notify();
            });
            if updated.is_err() {
                break;
            }
        })
        .detach();

        // A document that cannot be read is reported and the default flow
        // opens in its place, unsaved, so the file is left as it is. A
        // picture with no document yet gets one.
        let mut unreadable = None;
        let opened = document_path.as_ref().and_then(|path| {
            if !path.exists() {
                return None;
            }
            std::fs::read_to_string(path)
                .map_err(|error| error.to_string())
                .and_then(|text| Document::from_json(&text).map_err(|error| error.to_string()))
                .map_err(|error| unreadable = Some(format!("{}: {error}", path.display())))
                .ok()
        });
        let was_opened = opened.is_some();
        let document = opened.unwrap_or_else(|| panes::flow_document(&band_info.names));
        let mut studio =
            Self {
                native_view,
                _ns_view: ns_view,
                canvas,
                canvas_bounds: Rc::default(),
                slider_bounds: SliderId::ALL
                    .into_iter()
                    .chain((0..MAX_LAYERS).flat_map(|layer| {
                        [SliderId::ToneHue(layer), SliderId::ToneSaturation(layer)]
                    }))
                    .chain((0..8).map(SliderId::StretchParam))
                    .chain((0..=seiza_compose::tone::CONTROLS.len()).map(SliderId::FinishParam))
                    .chain((0..tools::MAX_TOOL_SETTINGS).map(SliderId::ToolParam))
                    .map(|id| (id, Rc::default()))
                    .collect(),
                tool: Tool::Pan,
                drag: None,
                scale_factor: window.scale_factor(),
                document,
                selected: Row::Combined,
                alone: false,
                mask_shown: false,
                brush_removes: false,
                brush_size: 24.0,
                crop_from: None,
                mask_shows_picture: false,
                wipe: 0.5,
                split: Split::WithPicture,
                pins: Vec::new(),
                places: Vec::new(),
                unfolded: HashSet::new(),
                open_menu: None,
                window_title: String::new(),
                toggled: HashSet::new(),
                stroke: None,
                scene_version: 0,
                band_info,
                analysed: Vec::new(),
                picture_samples: None,
                picture_wanted: None,
                source_generation: 0,
                resolved_chains: Default::default(),
                histogram: None,
                export: None,
                export_line: String::new(),
                parity_report,
                working_profile,
                levels,
                tools,
            };
        // The store was prepared for the document it is opened with, and
        // may hold inputs the document has not listed yet.
        if was_opened {
            studio.document_is_saved();
        }
        if let Some(why) = unreadable {
            studio.keep_document_unsaved(format!(
                "Not saved: the document could not be read ({why})"
            ));
        }
        studio.take_in_new_inputs();
        studio.tools_are_prepared();
        studio.refresh_canvas();
        studio
    }

    /// Converts a window position to document pixels, in the pane the
    /// position is over.
    fn to_document(&self, position: Point<Pixels>) -> [f64; 2] {
        let bounds = self.canvas_bounds.get();
        let scale = self.scale_factor as f64;
        let x = f64::from(position.x - bounds.origin.x) * scale;
        let y = f64::from(position.y - bounds.origin.y) * scale;
        self.canvas.read(|shared| {
            let canvas = &shared.canvas;
            let count = canvas.scenes.len().max(1);
            let pane = (0..count)
                .rev()
                .map(|index| canvas.pane(index))
                .find(|pane| x >= pane[0] as f64 && y >= pane[1] as f64)
                .unwrap_or_else(|| canvas.pane(0));
            [
                canvas.view.center[0]
                    + (x - pane[0] as f64 - pane[2] as f64 / 2.0) / canvas.view.zoom,
                canvas.view.center[1]
                    + (y - pane[1] as f64 - pane[3] as f64 / 2.0) / canvas.view.zoom,
            ]
        })
    }

    fn set_slider(&mut self, id: SliderId, x: Pixels) {
        let bounds = self.slider_bounds[&id].get();
        let value = (f32::from(x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0))
            .clamp(0.0, 1.0);
        self.slider_moved(id, value);
        self.refresh_canvas();
    }

    /// The brush's radius in document pixels at the current zoom.
    fn brush_radius(&self) -> f32 {
        let zoom = self.canvas.read(|shared| shared.canvas.view.zoom);
        (f64::from(self.brush_size) * self.scale_factor as f64 / zoom) as f32
    }

    /// A window position as a fraction of the canvas's width.
    fn across_canvas(&self, position: Point<Pixels>) -> f32 {
        let bounds = self.canvas_bounds.get();
        f32::from(position.x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0)
    }

    fn canvas_down(&mut self, event: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
        // A press on a wipe's divider moves the divider.
        let on_divider = self.split == Split::Wipe && {
            let reach = 8.0 / f32::from(self.canvas_bounds.get().size.width).max(1.0);
            (self.across_canvas(event.position) - self.wipe).abs() < reach
        };
        if on_divider {
            self.drag = Some(Drag::Divider);
            return;
        }
        self.drag = Some(Drag::Canvas(event.position));
        if self.tool == Tool::Crop {
            self.crop_from = Some(self.to_document(event.position));
            return;
        }
        if self.tool != Tool::Pan {
            let at = self.to_document(event.position);
            self.stroke = Some(Stroke {
                points: vec![[at[0] as f32, at[1] as f32]],
                radius: self.brush_radius(),
                hardness: 0.6,
                mode: StrokeMode::Add,
            });
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        match self.drag {
            Some(Drag::Slider(id)) => {
                self.set_slider(id, event.position.x);
                cx.notify();
            }
            Some(Drag::Divider) => {
                self.wipe = self.across_canvas(event.position).clamp(0.02, 0.98);
                let wipe = self.wipe;
                self.canvas.edit(|canvas| canvas.wipe = Some(wipe));
                cx.notify();
            }
            Some(Drag::Canvas(last)) if last != event.position => {
                let from = self.to_document(last);
                let to = self.to_document(event.position);
                let (tool, radius) = (self.tool, self.brush_radius());
                // The crop being drawn shows as its outline.
                if let (Tool::Crop, Some(start)) = (tool, self.crop_from) {
                    let zoom = self.canvas.read(|shared| shared.canvas.view.zoom);
                    let line = (1.5 * self.scale_factor as f64 / zoom) as f32;
                    let (a, b) = (start.map(|v| v as f32), to.map(|v| v as f32));
                    let corners = [[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]];
                    self.canvas.edit(|canvas| {
                        canvas.strokes_replaced += 1;
                        canvas.strokes = (0..4)
                            .map(|side| Segment {
                                from: corners[side],
                                to: corners[(side + 1) % 4],
                                radius: line,
                                hardness: 1.0,
                                value: 1.0,
                            })
                            .collect();
                    });
                    self.drag = Some(Drag::Canvas(event.position));
                    cx.notify();
                    return;
                }
                self.canvas.edit(|canvas| match tool {
                    Tool::Pan => {
                        canvas.view.center[0] -= to[0] - from[0];
                        canvas.view.center[1] -= to[1] - from[1];
                    }
                    // The canvas shows a stroke as mask coverage while it is
                    // drawn, for the brush and for healing alike.
                    Tool::Crop => {}
                    Tool::Brush | Tool::Heal => canvas.strokes.push(Segment {
                        from: [from[0] as f32, from[1] as f32],
                        to: [to[0] as f32, to[1] as f32],
                        radius,
                        hardness: 0.6,
                        value: 1.0,
                    }),
                });
                if let Some(stroke) = &mut self.stroke {
                    stroke.points.push([to[0] as f32, to[1] as f32]);
                }
                self.drag = Some(Drag::Canvas(event.position));
            }
            _ => {}
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.drag = None;
        if let Some(start) = self.crop_from.take() {
            let end = self.to_document(event.position);
            let size = self.canvas.read(|shared| shared.canvas.document_size);
            // A drawn crop stays inside what every input covers, so it never
            // brings back the empty edges registration leaves.
            let (mut start, mut end) = (start, end);
            if let Some([x, y, width, height]) = self.band_info.automatic_crop {
                let (left, top) = (f64::from(x), f64::from(y));
                let (right, bottom) = (left + f64::from(width), top + f64::from(height));
                for corner in [&mut start, &mut end] {
                    corner[0] = corner[0].clamp(left, right);
                    corner[1] = corner[1].clamp(top, bottom);
                }
            }
            if let Some(crop) = CropRecord::from_corners(start, end, size) {
                self.document.crop = crop;
            }
            self.refresh_canvas();
            self.canvas.edit(|canvas| canvas.fit());
            cx.notify();
            return;
        }
        if let Some(stroke) = self.stroke.take() {
            self.commit_stroke(stroke);
            cx.notify();
        }
    }

    fn scroll(&mut self, event: &ScrollEvent, _: &mut Window, _: &mut Context<Self>) {
        let anchor = self.to_document(event.position);
        let scale_factor = self.scale_factor as f64;
        let (delta, zooms) = (event.delta, event.modifiers.platform);
        self.canvas.edit(|canvas| match delta {
            // A trackpad scroll pans; with a modifier, or from a wheel, it zooms.
            ScrollDelta::Pixels(delta) if !zooms => {
                let scale = scale_factor / canvas.view.zoom;
                canvas.view.center[0] -= f64::from(delta.x) * scale;
                canvas.view.center[1] -= f64::from(delta.y) * scale;
            }
            delta => {
                let steps = match delta {
                    ScrollDelta::Pixels(delta) => f64::from(delta.y) * 0.01,
                    ScrollDelta::Lines(delta) => delta.y as f64 * 0.25,
                };
                let zoom = (canvas.view.zoom * steps.exp()).clamp(canvas.fit_zoom() * 0.5, 8.0);
                let ratio = canvas.view.zoom / zoom;
                canvas.view.center[0] = anchor[0] + (canvas.view.center[0] - anchor[0]) * ratio;
                canvas.view.center[1] = anchor[1] + (canvas.view.center[1] - anchor[1]) * ratio;
                canvas.view.zoom = zoom;
            }
        });
    }

    fn readout(&self) -> Vec<String> {
        self.canvas.read(|shared| {
            let (canvas, readout) = (&shared.canvas, &shared.readout);
            if let Some(error) = &readout.error {
                return vec![format!("Renderer: {error}")];
            }
            let stats = readout.stats;
            let percentile = |values: &VecDeque<f32>, fraction: f32| {
                let mut sorted: Vec<f32> = values.iter().copied().collect();
                sorted.sort_by(f32::total_cmp);
                sorted
                    .get(((sorted.len().saturating_sub(1)) as f32 * fraction) as usize)
                    .copied()
                    .unwrap_or(0.0)
            };
            let mut lines = vec![
                format!(
                    "Surface {} x {} px",
                    canvas.surface_size.0, canvas.surface_size.1
                ),
                format!(
                    "Zoom {:.1}%  level {}",
                    canvas.view.zoom * 100.0,
                    stats.level
                ),
                format!(
                    "Frame interval p50 {:.1} ms  p95 {:.1} ms",
                    percentile(&readout.intervals, 0.5),
                    percentile(&readout.intervals, 0.95)
                ),
                format!(
                    "Encode {:.2} ms  wait for surface {:.2} ms",
                    stats.encode_ms, stats.acquire_ms
                ),
                format!(
                    "Input to submit p50 {:.1} ms  p95 {:.1} ms",
                    percentile(&readout.input_to_submit_ms, 0.5),
                    percentile(&readout.input_to_submit_ms, 0.95)
                ),
                format!(
                    "Tiles drawn {}  from a coarser level {}",
                    stats.tiles_drawn, stats.tiles_coarse
                ),
                format!(
                    "Tiles on GPU {} ({} MB)  loading {}",
                    stats.cache_tiles,
                    stats.cache_bytes >> 20,
                    stats.tiles_in_flight
                ),
                format!("Brush segments {}", canvas.strokes.len()),
            ];
            if let Some(layer) = &readout.layer {
                lines.push(format!("Layer: {layer}"));
            }
            if let Some(bench) = &readout.bench {
                lines.push(format!("Bench: {bench}"));
            }
            lines
        })
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceCopyName(space: *const std::ffi::c_void) -> *mut AnyObject;
}

/// Describes the Metal layer wgpu draws the canvas into: its pixel format,
/// its color space, and whether it asks for extended range.
///
/// # Safety
///
/// `view` must be a live `NSView`, and the call must be on the main thread.
unsafe fn layer_description(view: &AnyObject) -> Option<String> {
    unsafe {
        let root: *mut AnyObject = msg_send![view, layer];
        let mut layers = vec![root];
        if !root.is_null() {
            let sublayers: *mut AnyObject = msg_send![root, sublayers];
            if !sublayers.is_null() {
                let count: usize = msg_send![sublayers, count];
                for index in 0..count {
                    layers.push(msg_send![sublayers, objectAtIndex: index]);
                }
            }
        }
        for layer in layers {
            if layer.is_null() {
                continue;
            }
            let is_metal: bool = msg_send![layer, isKindOfClass: class!(CAMetalLayer)];
            if !is_metal {
                continue;
            }
            let pixel_format: usize = msg_send![layer, pixelFormat];
            let extended: bool = msg_send![layer, wantsExtendedDynamicRangeContent];
            let space: *const std::ffi::c_void = msg_send![layer, colorspace];
            let name = if space.is_null() {
                "none set".to_string()
            } else {
                let name = CGColorSpaceCopyName(space);
                if name.is_null() {
                    "unnamed".to_string()
                } else {
                    let text: *const std::ffi::c_char = msg_send![name, UTF8String];
                    let text = std::ffi::CStr::from_ptr(text)
                        .to_string_lossy()
                        .into_owned();
                    let _: () = msg_send![name, release];
                    text
                }
            };
            return Some(format!(
                "MTLPixelFormat {pixel_format}, color space {name}, extended range {extended}"
            ));
        }
        None
    }
}

/// The surface and display transform the options ask for.
fn output(options: &Options) -> Result<Output, String> {
    use wgpu::{SurfaceColorSpace, TextureFormat};
    let (format, color_space, target) = match options.output.as_str() {
        "" | "srgb8" => (None, SurfaceColorSpace::Auto, "sRGB Profile.icc"),
        "p3-8" => (None, SurfaceColorSpace::DisplayP3, "Display P3.icc"),
        "p3-float" => (
            Some(TextureFormat::Rgba16Float),
            SurfaceColorSpace::DisplayP3,
            "Display P3.icc",
        ),
        "p3-extended" => (
            Some(TextureFormat::Rgba16Float),
            SurfaceColorSpace::ExtendedDisplayP3,
            "Display P3.icc",
        ),
        other => return Err(format!("unknown output: {other}")),
    };
    let curves = options
        .working_profile
        .as_ref()
        .map(|working| {
            let target = std::path::Path::new(psf_studio_color::SYSTEM_PROFILES).join(target);
            psf_studio_color::curves_between(working, &target)
        })
        .transpose()?;
    Ok(Output {
        format,
        color_space,
        curves,
        vsync: options.vsync,
    })
}

fn write_png(path: &str, width: u32, height: u32, bgra: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|error| error.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let rgb: Vec<u8> = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]])
        .collect();
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&rgb))
        .map_err(|error| error.to_string())
}

fn push(values: &mut VecDeque<f32>, value: f32) {
    if values.len() == 240 {
        values.pop_front();
    }
    values.push_back(value);
}

pub fn run(options: Options) {
    let application = gpui_platform::application();
    // A link from a PSF Guard page, whether it starts the program or
    // reaches it while it runs.
    application.on_open_urls(open::keep_links);
    application.run(move |cx: &mut App| {
        // The canvas is 1920 x 1080 points: a 4K surface on a 2x display.
        let bounds = Bounds::centered(
            None,
            size(px(1920.0 + PANEL_WIDTH + STEPS_WIDTH), px(1080.0 + 48.0)),
            cx,
        );
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Studio::new(options, window, cx)),
        )
        .expect("opening the window");
        cx.activate(true);
    });
}

#[path = "export.rs"]
mod export;
#[path = "inspector.rs"]
mod inspector;
#[path = "open.rs"]
mod open;
#[path = "panes.rs"]
mod panes;
#[path = "parts.rs"]
mod parts;
#[path = "tools.rs"]
mod tools;
#[path = "tree.rs"]
mod tree;
#[path = "widgets.rs"]
mod widgets;
