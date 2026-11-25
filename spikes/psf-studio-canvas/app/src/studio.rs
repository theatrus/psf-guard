use crate::bench::Bench;
use crate::studio_model::{Document, StageKind, Stroke, StrokeMode};
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
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use studio_render_spike::synth::{BANDS, DOC_HEIGHT, DOC_WIDTH};
use studio_render_spike::{FinishStep, Frame, FrameStats, Layer, Output, Renderer, Segment, View};
use uuid::Uuid;

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
    /// Where the benchmark writes PNG captures of selected frames.
    pub capture_prefix: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            bench_output: None,
            bench_frames: 600,
            vsync: true,
            capture_prefix: None,
            output: String::new(),
            working_profile: None,
        }
    }
}

const BAND_NAMES: [&str; BANDS] = ["L", "R", "G", "B", "Ha", "OIII", "SII", "NII"];
const BAND_COLORS: [[f32; 3]; BANDS] = [
    [0.30, 0.30, 0.30],
    [0.45, 0.02, 0.02],
    [0.02, 0.45, 0.02],
    [0.02, 0.02, 0.45],
    [0.55, 0.16, 0.04],
    [0.02, 0.40, 0.42],
    [0.50, 0.03, 0.08],
    [0.40, 0.04, 0.38],
];
const PANEL_WIDTH: f32 = 320.0;
const STEPS_WIDTH: f32 = 260.0;
/// Scales the band colors so eight bands at once stay within range.
const COLOR_GAIN: f32 = 0.6;

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Pan,
    Brush,
    Heal,
}

/// A slider in a pane.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum SliderId {
    /// A band's midtone, in the Stretch stage.
    Midtone(usize),
    /// A layer's opacity, in the Combine stage.
    Opacity(usize),
    /// The selected step's amount.
    Amount,
}

#[derive(Clone, Copy)]
enum Drag {
    Slider(SliderId),
    Canvas(Point<Pixels>),
}

/// What the canvas draws: the view, the band settings and the strokes.
///
/// The UI thread owns one copy and edits it. The render thread keeps its own
/// copy and draws from that, so neither waits for the other.
pub struct CanvasState {
    pub view: View,
    /// Slider positions in 0..=1, one per band.
    pub sliders: [f32; BANDS],
    /// What the document asks the canvas to draw.
    scene: Arc<Scene>,
    /// Strokes shown over the image: a stroke being drawn, or the selected
    /// step's mask.
    pub strokes: Vec<Segment>,
    pub surface_size: (u32, u32),
    fitted: bool,
    /// The name of a capture to take on the next frame.
    pub capture: Option<String>,
    /// Set to measure the GPU's cost of the next frame with no display.
    pub measure_gpu: bool,
    pub gpu_frame_ms: Option<f32>,
}

impl CanvasState {
    fn new() -> Self {
        Self {
            view: View {
                center: [0.0, 0.0],
                zoom: 1.0,
            },
            sliders: [0.5; BANDS],
            scene: Arc::new(Scene::default()),
            strokes: Vec::new(),
            surface_size: (1, 1),
            fitted: false,
            capture: None,
            measure_gpu: false,
            gpu_frame_ms: None,
        }
    }

    pub fn fit_zoom(&self) -> f64 {
        let (width, height) = self.surface_size;
        (width as f64 / DOC_WIDTH as f64).min(height as f64 / DOC_HEIGHT as f64)
    }

    pub fn fit(&mut self) {
        self.view = View {
            center: [DOC_WIDTH as f64 / 2.0, DOC_HEIGHT as f64 / 2.0],
            zoom: self.fit_zoom(),
        };
    }

    /// The scene's layers, each with the midtone its band's slider gives.
    fn layers(&self) -> Vec<Layer> {
        self.scene
            .layers
            .iter()
            .map(|layer| Layer {
                // The slider covers midtone balances from 0.001 to 0.5.
                midtone: 10f32.powf(-3.0 + 2.7 * self.sliders[layer.band]),
                ..*layer
            })
            .collect()
    }
}

/// What the document asks the canvas to draw: the Combine stage's layers and
/// the Finish stage's steps with their masks.
#[derive(Default)]
pub struct Scene {
    layers: Vec<Layer>,
    finish: Vec<FinishStep>,
    masks: Vec<Vec<Segment>>,
    /// Different for every scene, so the renderer uploads masks once.
    version: u64,
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
    /// Set by the UI when the canvas changed; cleared by the render thread.
    dirty: bool,
    /// When the oldest input not yet drawn arrived.
    input_at: Option<Instant>,
    readout: Readout,
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
    let mut animate = true;
    loop {
        let input_at = {
            let mut shared = lock.lock().expect("canvas state");
            while !shared.dirty && !animate {
                shared = wake.wait(shared).expect("canvas state");
                last_frame = None;
            }
            shared.dirty = false;
            canvas.surface_size = shared.canvas.surface_size;
            canvas.scene = shared.canvas.scene.clone();
            if !shared.canvas.fitted && canvas.surface_size.0 > 1 {
                canvas.fit();
                shared.canvas.view = canvas.view;
                shared.canvas.fitted = true;
            } else if bench.is_none() {
                canvas.view = shared.canvas.view;
            }
            if bench.is_none() {
                canvas.sliders = shared.canvas.sliders;
                // Strokes only grow or reset, so copy the new tail.
                if shared.canvas.strokes.len() < canvas.strokes.len() {
                    canvas.strokes.clear();
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
        let layers = canvas.layers();
        let frame = Frame {
            view: canvas.view,
            layers: &layers,
            finish: &canvas.scene.finish,
            masks: &canvas.scene.masks,
            masks_version: canvas.scene.version,
            overlay: &canvas.strokes,
            overlay_strength: 1.0,
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
    /// The stage whose panes are showing.
    stage: StageKind,
    /// The selected step of the showing stage.
    selected_step: Option<Uuid>,
    /// The selected layer of the Combine stage, counted from the bottom.
    selected_layer: usize,
    /// The brush stroke being drawn.
    stroke: Option<Stroke>,
    /// Counts the scenes sent to the canvas.
    scene_version: u64,
}

impl Studio {
    fn new(options: Options, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
                dirty: true,
                input_at: None,
                readout: Readout::default(),
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
        let renderer = surface
            .map_err(|error| error.to_string())
            .and_then(|surface| Ok((surface, output(&options)?)))
            .and_then(|(surface, output)| {
                pollster::block_on(Renderer::new(
                    &instance,
                    surface,
                    1,
                    1,
                    2 << 30,
                    workers,
                    output,
                ))
            });
        match renderer {
            Ok(renderer) => {
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

        let document = panes::demo_document();
        let mut studio = Self {
            native_view,
            _ns_view: ns_view,
            canvas,
            canvas_bounds: Rc::default(),
            slider_bounds: (0..BANDS)
                .flat_map(|band| [SliderId::Midtone(band), SliderId::Opacity(band)])
                .chain([SliderId::Amount])
                .map(|id| (id, Rc::default()))
                .collect(),
            tool: Tool::Pan,
            drag: None,
            scale_factor: window.scale_factor(),
            selected_step: document
                .stage(StageKind::Stretch)
                .steps
                .first()
                .map(|step| step.id),
            document,
            stage: StageKind::Stretch,
            selected_layer: 0,
            stroke: None,
            scene_version: 0,
        };
        studio.refresh_canvas();
        studio
    }

    /// Converts a window position to document pixels.
    fn to_document(&self, position: Point<Pixels>) -> [f64; 2] {
        let bounds = self.canvas_bounds.get();
        let scale = self.scale_factor as f64;
        let x = f64::from(position.x - bounds.origin.x) * scale;
        let y = f64::from(position.y - bounds.origin.y) * scale;
        self.canvas.read(|shared| {
            let canvas = &shared.canvas;
            let (width, height) = canvas.surface_size;
            [
                canvas.view.center[0] + (x - width as f64 / 2.0) / canvas.view.zoom,
                canvas.view.center[1] + (y - height as f64 / 2.0) / canvas.view.zoom,
            ]
        })
    }

    fn set_slider(&mut self, id: SliderId, x: Pixels) {
        let bounds = self.slider_bounds[&id].get();
        let value = (f32::from(x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0))
            .clamp(0.0, 1.0);
        match id {
            SliderId::Midtone(band) => self.canvas.edit(|canvas| canvas.sliders[band] = value),
            SliderId::Amount => {
                if let Some(id) = self.selected_step
                    && let Some((_, step)) = self.document.step(id)
                {
                    let amount = panes::amount_range(step.kind.name())
                        .map_or(value, |(low, high)| low + (high - low) * value);
                    self.document
                        .set_step_settings(id, serde_json::json!({ "amount": amount }));
                    self.refresh_canvas();
                }
            }
            SliderId::Opacity(band) => {
                if let Some(id) = self.document.layers().get(band).map(|layer| layer.id)
                    && let Some(layer) = self.document.layer_mut(id)
                {
                    layer.opacity = value;
                }
                self.refresh_canvas();
            }
        }
    }

    /// The brush's radius in document pixels at the current zoom.
    fn brush_radius(&self) -> f32 {
        let zoom = self.canvas.read(|shared| shared.canvas.view.zoom);
        (24.0 * self.scale_factor as f64 / zoom) as f32
    }

    fn canvas_down(&mut self, event: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag = Some(Drag::Canvas(event.position));
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
            Some(Drag::Canvas(last)) if last != event.position => {
                let from = self.to_document(last);
                let to = self.to_document(event.position);
                let (tool, radius) = (self.tool, self.brush_radius());
                self.canvas.edit(|canvas| match tool {
                    Tool::Pan => {
                        canvas.view.center[0] -= to[0] - from[0];
                        canvas.view.center[1] -= to[1] - from[1];
                    }
                    // The canvas shows a stroke as mask coverage while it is
                    // drawn, for the brush and for healing alike.
                    Tool::Brush | Tool::Heal => canvas.strokes.push(Segment {
                        from: [from[0] as f32, from[1] as f32],
                        to: [to[0] as f32, to[1] as f32],
                        radius,
                        hardness: 0.6,
                        value: 1.0,
                        _pad: 0.0,
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

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.drag = None;
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
            let target = std::path::Path::new(studio_color_spike::SYSTEM_PROFILES).join(target);
            studio_color_spike::curves_between(working, &target)
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
    gpui_platform::application().run(move |cx: &mut App| {
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

#[path = "panes.rs"]
mod panes;
