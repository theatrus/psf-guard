//! The scripted measurement. Each phase drives the canvas for a fixed number
//! of frames and records what every frame cost.

use crate::studio::CanvasState;
use serde_json::{json, Value};
use std::time::Instant;
use studio_render_spike::synth::{DOC_HEIGHT, DOC_WIDTH};
use studio_render_spike::{FrameStats, Segment};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Wait for the view's tiles before a measured phase.
    Settle(&'static str),
    StretchFit,
    Stretch1to1,
    Brush,
    BrushManyStrokes,
    Pan(f64),
    ZoomSweep,
    Done,
}

const SCRIPT: &[Phase] = &[
    Phase::Settle("fit"),
    Phase::StretchFit,
    Phase::Settle("1:1"),
    Phase::Stretch1to1,
    Phase::Brush,
    Phase::BrushManyStrokes,
    Phase::Settle("pan_start"),
    Phase::Pan(1500.0),
    Phase::Settle("after_pan_1500"),
    Phase::Settle("pan_start"),
    Phase::Pan(6000.0),
    Phase::Settle("after_pan_6000"),
    Phase::ZoomSweep,
    Phase::Settle("after_zoom_sweep"),
    Phase::Done,
];

#[derive(Default)]
struct Samples {
    interval_ms: Vec<f32>,
    encode_ms: Vec<f32>,
    acquire_ms: Vec<f32>,
    gpu_ms: Vec<f32>,
    coarse_frames: u32,
    max_coarse: u32,
    max_blank: u32,
    uploads: u32,
    max_drawn: u32,
}

pub struct Bench {
    step: usize,
    frame_in_phase: u32,
    frames_per_phase: u32,
    phase_started: Instant,
    last_frame: Option<Instant>,
    samples: Samples,
    quiet_frames: u32,
    sharp_since: Option<Instant>,
    results: Vec<Value>,
    pub finished: bool,
}

impl Bench {
    pub fn new(frames_per_phase: u32) -> Self {
        Self {
            step: 0,
            frame_in_phase: 0,
            frames_per_phase,
            phase_started: Instant::now(),
            last_frame: None,
            samples: Samples::default(),
            quiet_frames: 0,
            sharp_since: None,
            results: Vec::new(),
            finished: false,
        }
    }

    pub fn label(&self) -> String {
        format!("{:?} frame {}", SCRIPT[self.step], self.frame_in_phase)
    }

    /// Changes the canvas for the next frame.
    pub fn before_frame(&mut self, canvas: &mut CanvasState) {
        canvas.measure_gpu = !matches!(SCRIPT[self.step], Phase::Settle(_) | Phase::Done)
            && self.frame_in_phase + 1 == self.frames_per_phase;
        let frame = self.frame_in_phase as f64;
        let progress = frame / self.frames_per_phase as f64;
        let (width, _) = canvas.surface_size;
        let center = [DOC_WIDTH as f64 / 2.0, DOC_HEIGHT as f64 / 2.0];
        match SCRIPT[self.step] {
            Phase::Settle("fit") => canvas.fit(),
            Phase::Settle("1:1") => {
                canvas.view.zoom = 1.0;
                canvas.view.center = center;
            }
            Phase::Settle("pan_start") => {
                canvas.strokes.clear();
                canvas.view.zoom = 1.0;
                canvas.view.center = pan_position(0.0);
            }
            Phase::Settle(_) | Phase::Done => {}
            Phase::StretchFit | Phase::Stretch1to1 => {
                // Every frame moves every band's midtone, as a slider drag would.
                for (index, slider) in canvas.sliders.iter_mut().enumerate() {
                    *slider = 0.5 + 0.35 * (frame * 0.05 + index as f64).sin() as f32;
                }
            }
            Phase::Brush | Phase::BrushManyStrokes => {
                if SCRIPT[self.step] == Phase::BrushManyStrokes && self.frame_in_phase == 0 {
                    // A document with a long painting history.
                    for index in 0..20_000 {
                        let t = index as f32 * 0.37;
                        let from = [
                            center[0] as f32 + t.cos() * (200.0 + index as f32 * 0.4),
                            center[1] as f32 + t.sin() * (200.0 + index as f32 * 0.2),
                        ];
                        canvas.strokes.push(Segment {
                            from,
                            to: [from[0] + 30.0, from[1] + 12.0],
                            radius: 14.0,
                            hardness: 0.5,
                            value: 1.0,
                            _pad: 0.0,
                        });
                    }
                }
                let point = |t: f64| {
                    [
                        (center[0] + (t * 0.021).sin() * width as f64 * 0.4) as f32,
                        (center[1] + (t * 0.034).cos() * 700.0) as f32,
                    ]
                };
                canvas.strokes.push(Segment {
                    from: point(frame),
                    to: point(frame + 1.0),
                    radius: 24.0,
                    hardness: 0.6,
                    value: 1.0,
                    _pad: 0.0,
                });
            }
            Phase::Pan(speed) => {
                // Screen pixels per second, by the clock, so a slow frame
                // does not slow the pan.
                let elapsed = self.phase_started.elapsed().as_secs_f64();
                canvas.view.center = pan_position(speed * elapsed / canvas.view.zoom);
            }
            Phase::ZoomSweep => {
                let fit = canvas.fit_zoom();
                canvas.view.center = center;
                canvas.view.zoom = fit * (1.0 / fit).powf(progress);
            }
        }
    }

    /// Records the frame and moves to the next phase when this one is done.
    pub fn after_frame(&mut self, canvas: &mut CanvasState, stats: &FrameStats) {
        let now = Instant::now();
        let phase = SCRIPT[self.step];
        // The frame that also measured the GPU offscreen took longer for
        // that reason alone.
        if let Some(last) = self.last_frame
            && canvas.gpu_frame_ms.is_none()
        {
            self.samples
                .interval_ms
                .push((now - last).as_secs_f32() * 1000.0);
        }
        self.last_frame = Some(now);
        self.samples.encode_ms.push(stats.encode_ms);
        self.samples.acquire_ms.push(stats.acquire_ms);
        if let Some(gpu) = stats.gpu_ms {
            self.samples.gpu_ms.push(gpu);
        }
        if stats.tiles_coarse > 0 {
            self.samples.coarse_frames += 1;
        }
        self.samples.max_coarse = self.samples.max_coarse.max(stats.tiles_coarse);
        self.samples.max_blank = self.samples.max_blank.max(stats.tiles_blank);
        self.samples.max_drawn = self.samples.max_drawn.max(stats.tiles_drawn);
        self.samples.uploads += stats.tiles_uploaded;
        self.frame_in_phase += 1;

        let done = match phase {
            Phase::Settle(_) => {
                if stats.tiles_coarse == 0 {
                    self.sharp_since.get_or_insert(now);
                    self.quiet_frames += 1;
                } else {
                    self.sharp_since = None;
                    self.quiet_frames = 0;
                }
                self.quiet_frames >= 20 || self.frame_in_phase > 3000
            }
            Phase::Done => true,
            _ => self.frame_in_phase >= self.frames_per_phase,
        };
        if !done {
            return;
        }

        let samples = std::mem::take(&mut self.samples);
        if matches!(phase, Phase::StretchFit | Phase::Brush | Phase::Pan(_)) {
            canvas.capture = Some(format!("{phase:?}"));
        }
        let slow_frames = samples
            .interval_ms
            .iter()
            .filter(|interval| **interval > 20.0)
            .count();
        let frames = self.frame_in_phase;
        let mut entry = json!({
            "phase": format!("{phase:?}"),
            "frames": frames,
            "wall_ms": self.phase_started.elapsed().as_secs_f64() * 1000.0,
            "level": stats.level,
            "zoom": canvas.view.zoom,
            "tiles_drawn_max": samples.max_drawn,
            "frames_with_coarse_tiles": samples.coarse_frames,
            "coarse_tiles_max": samples.max_coarse,
            "blank_tiles_max": samples.max_blank,
            "frames_over_20_ms": slow_frames,
            "gpu_offscreen_ms_per_frame": canvas.gpu_frame_ms.take(),
            "tiles_uploaded": samples.uploads,
            "strokes": canvas.strokes.len(),
            "cache_mb": stats.cache_bytes / (1 << 20),
            "frame_interval_ms": summary(samples.interval_ms),
            "encode_ms": summary(samples.encode_ms),
            "acquire_ms": summary(samples.acquire_ms),
            "gpu_ms": summary(samples.gpu_ms),
        });
        if let Phase::Settle(_) = phase {
            // The time from the end of the motion until every visible tile
            // came from its own level.
            entry["sharp_after_ms"] = json!(self
                .sharp_since
                .map(|since| (since - self.phase_started).as_secs_f64() * 1000.0));
        }
        self.results.push(entry);

        self.step += 1;
        self.frame_in_phase = 0;
        self.quiet_frames = 0;
        self.sharp_since = None;
        self.phase_started = Instant::now();
        self.last_frame = None;
        if matches!(
            SCRIPT[self.step],
            Phase::BrushManyStrokes | Phase::Settle("1:1")
        ) {
            canvas.strokes.clear();
        }
        if SCRIPT[self.step] == Phase::Done {
            self.finished = true;
        }
    }

    pub fn report(
        &self,
        canvas: &CanvasState,
        renderer: &str,
        vsync: bool,
        mean_tile_ms: f32,
        layer: Option<String>,
    ) -> Value {
        json!({
            "metal_layer": layer,
            "mean_tile_compute_ms": mean_tile_ms,
            "document": {
                "width": DOC_WIDTH,
                "height": DOC_HEIGHT,
                "megapixels": DOC_WIDTH as f64 * DOC_HEIGHT as f64 / 1.0e6,
                "bands": studio_render_spike::synth::BANDS,
            },
            "surface": {
                "width": canvas.surface_size.0,
                "height": canvas.surface_size.1,
                "renderer": renderer,
                "vsync": vsync,
            },
            "frames_per_phase": self.frames_per_phase,
            "phases": self.results,
        })
    }
}

/// A closed path through the document for the pan phases: an ellipse, by
/// distance travelled in document pixels.
fn pan_position(distance: f64) -> [f64; 2] {
    let angle = distance / 9000.0;
    [
        DOC_WIDTH as f64 / 2.0 + 11_000.0 * angle.cos(),
        DOC_HEIGHT as f64 / 2.0 + 7000.0 * angle.sin(),
    ]
}

fn summary(mut values: Vec<f32>) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    values.sort_by(f32::total_cmp);
    let at = |fraction: f64| values[((values.len() - 1) as f64 * fraction).round() as usize];
    let round = |value: f32| (value as f64 * 100.0).round() / 100.0;
    json!({
        "p50": round(at(0.5)),
        "p95": round(at(0.95)),
        "p99": round(at(0.99)),
        "max": round(at(1.0)),
    })
}
