//! The canvas: Seiza's GPU compositor drawing a picture from the tiles in
//! view, then a present pass that applies the display transform.
//!
//! What a picture is and how it is drawn belong to Seiza
//! (`seiza_compose::gpu`), whose CPU render is the reference an export uses.
//! This file chooses and uploads tiles, lays panes out on the surface,
//! presents the finished picture, and samples it for the stretches resolved
//! from it.

use crate::display::{DisplayCurves, TableFormat};
use crate::synth::{TileKey, TILE};
use crate::tiles::{TileCache, TileProducer};
use crate::{Pyramid, TileSource};
use bytemuck::{Pod, Zeroable};
use seiza_compose::gpu::{DrawArea, GpuCompositor, GpuScene, GpuTargets, GpuView, TileDraw};
use seiza_compose::picture::Segment;
use std::sync::Arc;
use std::time::Instant;

/// The compositor's finished picture: float, so the working space's range
/// reaches the present pass and the sampling unclipped.
const FINISHED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The most input tiles one draw reads.
const MAX_TILE_DRAWS: usize = 4096;

/// Where the document sits in the surface.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// The document pixel at the center of the surface.
    pub center: [f64; 2],
    /// Surface pixels per document pixel.
    pub zoom: f64,
}

/// One pane of the canvas: a picture packed for the GPU, and where it is
/// drawn.
pub struct Pane<'a> {
    pub scene: &'a GpuScene,
    /// Changes whenever the scene's tables or masks do, so a pane drawn
    /// again unchanged is not uploaded again. `None` uploads every frame.
    pub version: Option<u64>,
    /// Where the pane's picture is laid out in the surface: x, y, width and
    /// height in pixels. The view's center is at its middle.
    pub viewport: [u32; 4],
    /// The part of the surface the pane draws to. It is the viewport for a
    /// pane of its own, and a part of it for one side of a wipe.
    pub scissor: [u32; 4],
}

pub struct Frame<'a> {
    /// Where the document sits in each pane. The panes share it, so they
    /// pan and zoom together.
    pub view: View,
    /// The panes, each with a viewport of the same size. At least one.
    pub panes: &'a [Pane<'a>],
    /// Strokes shown over the image: a stroke being drawn.
    pub overlay: &'a [Segment],
    /// How strongly the overlay shows, from 0 to 1.
    pub overlay_strength: f32,
    /// The part of the document the picture keeps, as left, top, right and
    /// bottom in document pixels. Nothing outside it is drawn. `None` keeps
    /// the whole document.
    pub crop: Option<[f32; 4]>,
}

/// The pixels between two panes.
const PANE_GAP: u32 = 2;

/// Where pane `index` of `count` is in a surface when the panes are a grid
/// of one size: x, y, width and height in pixels. Up to three panes are one
/// row; more are two rows.
pub fn pane_rect(index: usize, count: usize, surface: (u32, u32)) -> [u32; 4] {
    let count = count.max(1);
    let rows = if count > 3 { 2 } else { 1 };
    let columns = count.div_ceil(rows) as u32;
    let rows = rows as u32;
    let width = (surface.0.saturating_sub(PANE_GAP * (columns - 1)) / columns).max(1);
    let height = (surface.1.saturating_sub(PANE_GAP * (rows - 1)) / rows).max(1);
    let (column, row) = (index as u32 % columns, index as u32 / columns);
    let x = (column * (width + PANE_GAP)).min(surface.0.saturating_sub(1));
    let y = (row * (height + PANE_GAP)).min(surface.1.saturating_sub(1));
    [x, y, width.min(surface.0 - x), height.min(surface.1 - y)]
}

/// A rectangle kept inside a surface, so it is a valid viewport or scissor.
fn within_surface(rect: [u32; 4], surface: (u32, u32)) -> [u32; 4] {
    let x = rect[0].min(surface.0.saturating_sub(1));
    let y = rect[1].min(surface.1.saturating_sub(1));
    [
        x,
        y,
        rect[2].clamp(1, surface.0 - x),
        rect[3].clamp(1, surface.1 - y),
    ]
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    /// Time spent waiting for the surface's next texture.
    pub acquire_ms: f32,
    /// Time spent choosing tiles, uploading, encoding and submitting.
    pub encode_ms: f32,
    pub level: u8,
    pub tiles_drawn: u32,
    /// Visible tiles drawn from a coarser level because theirs is not loaded.
    pub tiles_coarse: u32,
    /// Visible tiles with nothing to draw at any level.
    pub tiles_blank: u32,
    pub tiles_uploaded: u32,
    pub tiles_in_flight: u32,
    pub cache_tiles: u32,
    pub cache_bytes: u64,
}

/// What the canvas presents to: the surface's format and color space, and
/// the transform from the document's working space to that target.
pub struct Output {
    /// The surface format to use when the surface offers it.
    pub format: Option<wgpu::TextureFormat>,
    /// The color space to set on the surface when the surface offers it.
    pub color_space: wgpu::SurfaceColorSpace,
    /// The display transform. `None` presents the working space unchanged.
    pub curves: Option<DisplayCurves>,
    /// Whether presenting waits for the display's refresh.
    pub vsync: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            format: None,
            color_space: wgpu::SurfaceColorSpace::Auto,
            curves: None,
            vsync: true,
        }
    }
}

/// What the present pass reads besides the finished picture.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PresentUniform {
    /// Linear working-space RGB to linear target RGB, as a std140
    /// `mat3x3f`.
    to_target: [f32; 12],
    /// x: samples per display curve, or 0 for no display transform.
    display: [f32; 4],
}

/// One input tile chosen for a draw: the tile whose textures are read, and
/// the rectangle drawn from them.
#[derive(Clone, Copy, Debug)]
struct ChosenTile {
    source: TileKey,
    rect: [f32; 4],
    uv_rect: [f32; 4],
}

/// What the last frame drew, for [`Renderer::offscreen_frame_ms`].
#[derive(Default)]
struct LastFrame {
    scene: Option<GpuScene>,
    /// The first pane's version, so an unchanged scene is not copied again.
    version: Option<u64>,
    view: Option<View>,
    tiles: Vec<ChosenTile>,
    overlay: Vec<Segment>,
    overlay_strength: f32,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    compositor: GpuCompositor,
    /// The compositor's textures, the size of the surface.
    targets: GpuTargets,
    present_pipeline: wgpu::RenderPipeline,
    present_layout: wgpu::BindGroupLayout,
    present_uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    display_curves: wgpu::TextureView,
    /// The finished picture, the uniform and the display curves, for the
    /// present pass. Made again with the targets.
    present_inputs: wgpu::BindGroup,
    surface_offers: String,
    cache: TileCache,
    /// The most bytes of tiles kept on the GPU.
    cache_budget_bytes: u64,
    producer: TileProducer,
    /// The threads that compute tiles.
    workers: usize,
    /// The document's size and levels.
    pyramid: Pyramid,
    frame: u64,
    /// The most tiles uploaded in one frame. Each is 4 MB.
    pub uploads_per_frame: u32,
    last: LastFrame,
    /// Set to copy the next frame into [`Self::captured`].
    pub capture_next: bool,
    /// The last captured frame: width, height, and BGRA bytes.
    pub captured: Option<(u32, u32, Vec<u8>)>,
    /// The sampling of the picture under way, if any.
    probe: Option<PictureProbe>,
    /// The tiles the sampling still needs, asked for after the view's.
    probe_wanted: Vec<TileKey>,
}

/// How far [`Renderer::sample_picture`] has got.
#[derive(Clone, Debug, PartialEq)]
pub enum Sampled {
    /// Some patches still wait for their tiles.
    Pending { done: usize, of: usize },
    /// The picture's samples, red, green and blue interleaved, patch after
    /// patch.
    Ready(Vec<f32>),
}

/// A sampling of the picture: the patches read so far, and the targets the
/// patches are drawn into.
struct PictureProbe {
    id: u64,
    size: u32,
    samples: Vec<Option<Vec<f32>>>,
    targets: GpuTargets,
}

/// The patches one call of [`Renderer::sample_picture`] draws, at most, so a
/// frame is not held up.
const PROBE_PATCHES_PER_CALL: usize = 8;
/// The tiles the sampling asks for at once, at most.
const PROBE_TILES_WANTED: usize = 16;

impl Renderer {
    pub async fn new(
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'static>,
        cache_budget_bytes: u64,
        workers: usize,
        source: Arc<dyn TileSource>,
        output: Output,
    ) -> Result<Self, String> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                ..Default::default()
            })
            .await
            .map_err(|error| format!("no GPU adapter: {error}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("studio canvas"),
                required_features: adapter.features() & wgpu::Features::FLOAT32_FILTERABLE,
                // The adapter's own limits: the default caps a texture at
                // 8192 pixels, which a window across two large displays
                // passes.
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .map_err(|error| format!("no GPU device: {error}"))?;

        let capabilities = surface.get_capabilities(&adapter);
        // The present pass writes display-referred values, so the surface
        // must not apply the sRGB curve a second time.
        let format = output
            .format
            .filter(|format| capabilities.formats.contains(format))
            .or_else(|| {
                capabilities
                    .formats
                    .iter()
                    .copied()
                    .find(|format| !format.is_srgb())
            })
            .unwrap_or(capabilities.formats[0]);
        let color_space = output
            .color_space
            .to_color_spaces()
            .filter(|wanted| capabilities.color_spaces(format).contains(*wanted))
            .map_or(wgpu::SurfaceColorSpace::Auto, |_| output.color_space);
        let surface_offers = format!(
            "formats {:?}; color spaces for {format:?}: {:?}",
            capabilities.formats,
            capabilities.color_spaces(format)
        );
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            // The host gives the size with `resize` once it has laid out.
            width: 1,
            height: 1,
            present_mode: if output.vsync {
                wgpu::PresentMode::AutoVsync
            } else {
                wgpu::PresentMode::AutoNoVsync
            },
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            color_space,
        };
        surface.configure(&device, &config);

        let compositor = GpuCompositor::new(&device, &queue, FINISHED_FORMAT);
        let targets = compositor.targets(config.width, config.height);

        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let present_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("present"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture_entry(2),
                texture_entry(3),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present"),
            source: wgpu::ShaderSource::Wgsl(present_source().into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("present"),
            bind_group_layouts: &[Some(&present_layout)],
            ..Default::default()
        });
        let present_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("present_vertex"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("present_fragment"),
                targets: &[Some(format.into())],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // With no transform the shader skips the lookup, and this table is
        // only there to fill the binding.
        let identity = DisplayCurves {
            width: 2,
            decode: vec![[0.0; 3], [1.0; 3]],
            encode: vec![[0.0; 3], [1.0; 3]],
            to_target: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        };
        let table_format = if device
            .features()
            .contains(wgpu::Features::FLOAT32_FILTERABLE)
        {
            TableFormat::Full
        } else {
            TableFormat::Half
        };
        let display_curves =
            output
                .curves
                .as_ref()
                .unwrap_or(&identity)
                .upload(&device, &queue, table_format);
        let uniform = output
            .curves
            .as_ref()
            .map_or(PresentUniform::zeroed(), |curves| PresentUniform {
                to_target: curves.matrix_uniform(),
                display: [curves.width as f32, 0.0, 0.0, 0.0],
            });
        let present_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("present"),
            size: size_of::<PresentUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&present_uniform, 0, bytemuck::bytes_of(&uniform));
        let present_inputs = present_bind_group(
            &device,
            &present_layout,
            &present_uniform,
            &sampler,
            targets.finished_view(),
            &display_curves,
        );

        let pyramid = source.pyramid();
        let bands = source.bands().max(1);
        let mut cache = TileCache::new(cache_budget_bytes, bands);
        load_coarsest(
            &mut cache,
            &device,
            &queue,
            compositor.tile_layout(),
            source.as_ref(),
            0,
        );
        Ok(Self {
            device,
            queue,
            surface,
            config,
            compositor,
            targets,
            present_pipeline,
            present_layout,
            present_uniform,
            sampler,
            display_curves,
            present_inputs,
            surface_offers,
            cache,
            cache_budget_bytes,
            producer: TileProducer::new(workers, source),
            workers,
            pyramid,
            frame: 0,
            uploads_per_frame: 6,
            last: LastFrame::default(),
            capture_next: false,
            captured: None,
            probe: None,
            probe_wanted: Vec::new(),
        })
    }

    /// Draws another document from now on, such as a store that was
    /// prepared again. Every tile of the document before is dropped.
    pub fn replace_source(&mut self, source: Arc<dyn TileSource>) {
        // A sampling of the old document's picture is of no use now.
        self.stop_sampling();
        let pyramid = source.pyramid();
        let bands = source.bands().max(1);
        let mut cache = TileCache::new(self.cache_budget_bytes, bands);
        load_coarsest(
            &mut cache,
            &self.device,
            &self.queue,
            self.compositor.tile_layout(),
            source.as_ref(),
            self.frame,
        );
        self.cache = cache;
        self.producer = TileProducer::new(self.workers, source);
        self.pyramid = pyramid;
        self.last = LastFrame::default();
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn adapter_summary(&self) -> String {
        format!(
            "{:?} in {:?}, {}x{}",
            self.config.format, self.config.color_space, self.config.width, self.config.height
        )
    }

    /// The formats and color spaces the surface offered.
    pub fn surface_offers(&self) -> &str {
        &self.surface_offers
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        // No texture may be larger than the device allows.
        let largest = self.device.limits().max_texture_dimension_2d;
        let (width, height) = (width.clamp(1, largest), height.clamp(1, largest));
        if (width, height) == self.size() {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.targets = self.compositor.targets(width, height);
        self.present_inputs = present_bind_group(
            &self.device,
            &self.present_layout,
            &self.present_uniform,
            &self.sampler,
            self.targets.finished_view(),
            &self.display_curves,
        );
    }

    /// Draws one frame and presents it.
    pub fn render(&mut self, frame: &Frame) -> Result<FrameStats, String> {
        self.frame += 1;
        let mut stats = FrameStats::default();
        let _ = self.device.poll(wgpu::PollType::Poll);

        let acquire_started = Instant::now();
        let target = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            other => {
                self.surface.configure(&self.device, &self.config);
                return Err(format!("the surface had no texture: {other:?}"));
            }
        };
        stats.acquire_ms = acquire_started.elapsed().as_secs_f32() * 1000.0;
        let encode_started = Instant::now();

        let surface = (self.config.width, self.config.height);
        // Every pane's viewport is the size of the first.
        let first_rect = frame
            .panes
            .first()
            .map_or([0, 0, surface.0, surface.1], |pane| {
                within_surface(pane.viewport, surface)
            });
        let (width, height) = (first_rect[2] as f64, first_rect[3] as f64);
        let view = frame.view;

        // Tiles: upload what the workers finished, then choose what to draw.
        while stats.tiles_uploaded < self.uploads_per_frame {
            let Some((key, samples)) = self.producer.take_finished() else {
                break;
            };
            self.cache.insert(
                &self.device,
                &self.queue,
                self.compositor.tile_layout(),
                key,
                &samples,
                self.frame,
            );
            self.producer.uploaded(key);
            stats.tiles_uploaded += 1;
        }

        let level = ((1.0 / view.zoom)
            .log2()
            .floor()
            .clamp(0.0, self.pyramid.max_level() as f64)) as u8;
        stats.level = level;
        let half = [width / (2.0 * view.zoom), height / (2.0 * view.zoom)];
        let visible = [
            view.center[0] - half[0],
            view.center[1] - half[1],
            view.center[0] + half[0],
            view.center[1] + half[1],
        ];
        let visible_keys = keys_in(&self.pyramid, level, visible, 0);
        let mut wanted: Vec<TileKey> = Vec::new();
        let fallback_level = (level + 3).min(self.pyramid.max_level());
        if fallback_level != level {
            wanted.extend(keys_in(&self.pyramid, fallback_level, visible, 0));
        }
        let mut by_distance = visible_keys.clone();
        by_distance.sort_by(|a, b| {
            center_distance(&self.pyramid, *a, view.center).total_cmp(&center_distance(
                &self.pyramid,
                *b,
                view.center,
            ))
        });
        wanted.extend(by_distance);
        // One ring of tiles past the edge, so a pan finds them loaded.
        wanted.extend(
            keys_in(&self.pyramid, level, visible, 1)
                .into_iter()
                .filter(|key| !visible_keys.contains(key)),
        );
        // The tiles a sampling of the picture waits for come after the
        // view's.
        wanted.extend(
            self.probe_wanted
                .iter()
                .filter(|key| !wanted.contains(key))
                .copied()
                .collect::<Vec<_>>(),
        );
        wanted.retain(|key| !self.cache.contains(*key));
        self.producer.set_wanted(wanted);

        let mut chosen: Vec<ChosenTile> = Vec::with_capacity(visible_keys.len());
        for key in visible_keys.iter().take(MAX_TILE_DRAWS) {
            let mut source = *key;
            while !self.cache.contains(source) {
                match self.pyramid.parent(source) {
                    Some(parent) => source = parent,
                    None => break,
                }
            }
            if self.cache.touch(source, self.frame).is_none() {
                stats.tiles_blank += 1;
                continue;
            }
            if source != *key {
                stats.tiles_coarse += 1;
            }
            if let Some(tile) = chosen_tile(&self.pyramid, *key, source, frame.crop) {
                chosen.push(tile);
            }
        }
        stats.tiles_drawn = chosen.len() as u32;

        for (index, pane) in frame.panes.iter().enumerate() {
            let rect = within_surface(pane.viewport, surface);
            let scissor = within_surface(pane.scissor, surface);
            let draws = tile_draws(&self.cache, &chosen);
            self.compositor.draw(
                pane.scene,
                pane.version,
                &self.targets,
                GpuView {
                    center: view.center,
                    zoom: view.zoom,
                },
                DrawArea {
                    viewport: rect,
                    scissor,
                },
                &draws,
                frame.overlay,
                frame.overlay_strength,
                index == 0,
            );
        }

        let target_view = target
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        // Captures are written as 8-bit PNG files, so only an 8-bit surface
        // is captured.
        let eight_bit = self.config.format == wgpu::TextureFormat::Bgra8Unorm;
        let capture_texture = (std::mem::take(&mut self.capture_next) && eight_bit)
            .then(|| self.surface_sized("capture", wgpu::TextureUsages::COPY_SRC));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.present(&mut encoder, &target_view);
        let capture = capture_texture.map(|texture| {
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.present(&mut encoder, &view);
            let row_bytes = (self.config.width * 4).next_multiple_of(256);
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("capture"),
                size: row_bytes as u64 * self.config.height as u64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes),
                        rows_per_image: None,
                    },
                },
                texture.size(),
            );
            (buffer, row_bytes as usize)
        });
        self.queue.submit([encoder.finish()]);
        self.queue.present(target);
        if let Some((buffer, row_bytes)) = capture {
            buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            if let Ok(mapped) = buffer.slice(..).get_mapped_range() {
                let width = self.config.width as usize;
                let mut pixels = Vec::with_capacity(width * 4 * self.config.height as usize);
                for row in mapped.chunks(row_bytes) {
                    pixels.extend_from_slice(&row[..width * 4]);
                }
                self.captured = Some((self.config.width, self.config.height, pixels));
            }
        }

        // The first pane's scene is copied only when it changed.
        let first = frame.panes.first();
        let kept = first.and_then(|pane| pane.version).is_some()
            && first.map(|pane| pane.version) == Some(self.last.version);
        let scene = if kept {
            self.last.scene.take()
        } else {
            first.map(|pane| pane.scene.clone())
        };
        self.last = LastFrame {
            scene,
            version: first.and_then(|pane| pane.version),
            view: Some(view),
            tiles: chosen,
            overlay: frame.overlay.to_vec(),
            overlay_strength: frame.overlay_strength,
        };
        self.cache.evict(self.frame);
        stats.encode_ms = encode_started.elapsed().as_secs_f32() * 1000.0;
        stats.tiles_in_flight = self.producer.in_flight() as u32;
        stats.cache_tiles = self.cache.len() as u32;
        stats.cache_bytes = self.cache.bytes();
        Ok(stats)
    }

    /// The present pass: the finished picture through the display
    /// transform, over the whole of `target`.
    fn present(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("present"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.present_pipeline);
        pass.set_bind_group(0, &self.present_inputs, &[]);
        pass.draw(0..3, 0..1);
    }

    /// A texture of the surface's size and format that can be drawn to.
    fn surface_sized(&self, label: &str, also: wgpu::TextureUsages) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: self.config.width,
                height: self.config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | also,
            view_formats: &[],
        })
    }

    /// Samples the picture `scene` draws, from square patches of `size`
    /// document pixels at full resolution whose top-left corners are
    /// `patches`. A scene with no Finish steps gives the combined picture.
    ///
    /// Full resolution, not a reduced level: a reduced level's samples are
    /// means, so its noise is lower than the image's and a stretch resolved
    /// from it would clip the shadows in the wrong place.
    ///
    /// Each call draws the patches whose tiles are loaded, a few at a time,
    /// and asks for the tiles of the others. Calls with the same `id` carry
    /// on one sampling; a new `id` starts again.
    pub fn sample_picture(
        &mut self,
        id: u64,
        scene: &GpuScene,
        crop: Option<[f32; 4]>,
        patches: &[[u32; 2]],
        size: u32,
    ) -> Sampled {
        let size = size.max(1);
        if self
            .probe
            .as_ref()
            .is_none_or(|probe| probe.id != id || probe.size != size)
        {
            self.probe = Some(PictureProbe {
                id,
                size,
                samples: vec![None; patches.len()],
                targets: self.compositor.targets(size, size),
            });
        }
        let mut probe = self.probe.take().expect("a sampling");
        probe.samples.resize(patches.len(), None);
        self.probe_wanted.clear();
        // The patches drawn in this call are copied into one buffer, read
        // once at the end, so the call waits for the GPU once.
        let row_bytes = (size * 8).next_multiple_of(256);
        let patch_bytes = u64::from(row_bytes) * u64::from(size);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("picture samples"),
            size: patch_bytes * PROBE_PATCHES_PER_CALL as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut drawn: Vec<usize> = Vec::new();
        for (index, origin) in patches.iter().enumerate() {
            if probe.samples[index].is_some() {
                continue;
            }
            let rect = [
                f64::from(origin[0]),
                f64::from(origin[1]),
                f64::from(origin[0] + size),
                f64::from(origin[1] + size),
            ];
            let keys = keys_in(&self.pyramid, 0, rect, 0);
            let missing: Vec<TileKey> = keys
                .iter()
                .filter(|key| !self.cache.contains(**key))
                .copied()
                .collect();
            if !missing.is_empty() {
                let room = PROBE_TILES_WANTED.saturating_sub(self.probe_wanted.len());
                self.probe_wanted.extend(missing.into_iter().take(room));
                continue;
            }
            for key in &keys {
                self.cache.touch(*key, self.frame);
            }
            if drawn.len() == PROBE_PATCHES_PER_CALL {
                continue;
            }
            let chosen: Vec<ChosenTile> = keys
                .iter()
                .filter_map(|key| chosen_tile(&self.pyramid, *key, *key, crop))
                .collect();
            let half = f64::from(size) / 2.0;
            let whole = [0, 0, size, size];
            let draws = tile_draws(&self.cache, &chosen);
            // Not versioned: the view's tables and masks go up again on
            // the next frame.
            self.compositor.draw(
                scene,
                None,
                &probe.targets,
                GpuView {
                    center: [f64::from(origin[0]) + half, f64::from(origin[1]) + half],
                    zoom: 1.0,
                },
                DrawArea {
                    viewport: whole,
                    scissor: whole,
                },
                &draws,
                &[],
                0.0,
                true,
            );
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            encoder.copy_texture_to_buffer(
                probe.targets.finished.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: patch_bytes * drawn.len() as u64,
                        bytes_per_row: Some(row_bytes),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit([encoder.finish()]);
            drawn.push(index);
        }
        if !drawn.is_empty() {
            buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            if let Ok(mapped) = buffer.slice(..).get_mapped_range() {
                for (place, index) in drawn.iter().enumerate() {
                    let start = place * patch_bytes as usize;
                    let patch = &mapped[start..start + patch_bytes as usize];
                    let mut samples = Vec::with_capacity(size as usize * size as usize * 3);
                    for row in patch.chunks(row_bytes as usize) {
                        let pixels: &[u16] = bytemuck::cast_slice(&row[..size as usize * 8]);
                        for pixel in pixels.as_chunks::<4>().0 {
                            samples.extend(
                                pixel[..3]
                                    .iter()
                                    .map(|bits| half::f16::from_bits(*bits).to_f32()),
                            );
                        }
                    }
                    probe.samples[*index] = Some(samples);
                }
            }
        }
        let done = probe
            .samples
            .iter()
            .filter(|samples| samples.is_some())
            .count();
        let result = if done == patches.len() {
            self.probe_wanted.clear();
            Sampled::Ready(probe.samples.iter().flatten().flatten().copied().collect())
        } else {
            Sampled::Pending {
                done,
                of: patches.len(),
            }
        };
        self.probe = Some(probe);
        result
    }

    /// Stops a sampling of the picture: forgets the patches read and asks
    /// for no more of its tiles.
    pub fn stop_sampling(&mut self) {
        self.probe = None;
        self.probe_wanted.clear();
    }

    /// Draws the last frame's first pane again `frames` times over the whole
    /// surface, into a texture of the surface's size, and waits for the GPU
    /// to finish, five times over. Returns the time per frame of the fastest
    /// batch, which is the GPU's cost with no display to wait for. 0 when
    /// no frame has been drawn.
    pub fn offscreen_frame_ms(&mut self, frames: u32) -> f32 {
        let (Some(scene), Some(view)) = (self.last.scene.take(), self.last.view) else {
            return 0.0;
        };
        let texture = self.surface_sized("offscreen", wgpu::TextureUsages::empty());
        let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let whole = [0, 0, self.config.width, self.config.height];
        // Other programs use the GPU too. The fastest of several batches is
        // the one they disturbed least.
        let mut best = f32::INFINITY;
        for _ in 0..5 {
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            let started = Instant::now();
            for _ in 0..frames {
                let draws = tile_draws(&self.cache, &self.last.tiles);
                self.compositor.draw(
                    &scene,
                    None,
                    &self.targets,
                    GpuView {
                        center: view.center,
                        zoom: view.zoom,
                    },
                    DrawArea {
                        viewport: whole,
                        scissor: whole,
                    },
                    &draws,
                    &self.last.overlay,
                    self.last.overlay_strength,
                    true,
                );
                let mut encoder = self
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                self.present(&mut encoder, &target);
                self.queue.submit([encoder.finish()]);
            }
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            best = best.min(started.elapsed().as_secs_f32() * 1000.0 / frames.max(1) as f32);
        }
        self.last.scene = Some(scene);
        best
    }

    /// The size and levels of the document being drawn.
    pub fn pyramid(&self) -> Pyramid {
        self.pyramid
    }

    /// The mean time a worker took to compute one tile.
    pub fn mean_tile_ms(&self) -> f32 {
        self.producer.mean_tile_ms()
    }
}

/// Loads every tile of the coarsest level and keeps it, so a view never
/// shows a hole.
fn load_coarsest(
    cache: &mut TileCache,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    source: &dyn TileSource,
    frame: u64,
) {
    let pyramid = source.pyramid();
    let (across, down) = pyramid.tiles_across(pyramid.max_level());
    for y in 0..down as u16 {
        for x in 0..across as u16 {
            let key = TileKey {
                level: pyramid.max_level(),
                x,
                y,
            };
            cache.insert(device, queue, layout, key, &source.tile(key), frame);
            cache.pin(key);
        }
    }
}

/// The part of `key`'s rectangle inside the crop, drawn from the textures
/// of `source`, which is `key` or a coarser tile that covers it. `None`
/// when no part of it is inside the crop.
fn chosen_tile(
    pyramid: &Pyramid,
    key: TileKey,
    source: TileKey,
    crop: Option<[f32; 4]>,
) -> Option<ChosenTile> {
    let mut rect = pyramid.doc_rect(key);
    if let Some(crop) = crop {
        rect = [
            rect[0].max(crop[0]),
            rect[1].max(crop[1]),
            rect[2].min(crop[2]),
            rect[3].min(crop[3]),
        ];
        if rect[0] >= rect[2] || rect[1] >= rect[3] {
            return None;
        }
    }
    let span = (TILE << source.level) as f32;
    let origin = [source.x as f32 * span, source.y as f32 * span];
    Some(ChosenTile {
        source,
        rect,
        uv_rect: [
            (rect[0] - origin[0]) / span,
            (rect[1] - origin[1]) / span,
            (rect[2] - origin[0]) / span,
            (rect[3] - origin[1]) / span,
        ],
    })
}

/// The chosen tiles as the compositor draws them. A tile evicted since it
/// was chosen is left out.
fn tile_draws<'a>(cache: &'a TileCache, chosen: &[ChosenTile]) -> Vec<TileDraw<'a>> {
    chosen
        .iter()
        .filter_map(|tile| {
            Some(TileDraw {
                inputs: &cache.get(tile.source)?.bind_group,
                rect: tile.rect,
                uv_rect: tile.uv_rect,
            })
        })
        .collect()
}

fn present_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    finished: &wgpu::TextureView,
    display_curves: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("present"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(finished),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(display_curves),
            },
        ],
    })
}

/// The present shader's whole source: the display transform, then the pass.
fn present_source() -> String {
    format!(
        "{}\n{}",
        crate::display::SHADER,
        include_str!("present.wgsl")
    )
}

/// The tiles of `level` that touch `rect`, grown by `ring` tiles on each side.
fn keys_in(pyramid: &Pyramid, level: u8, rect: [f64; 4], ring: i64) -> Vec<TileKey> {
    let span = (TILE << level) as f64;
    let (across, down) = pyramid.tiles_across(level);
    let x0 = ((rect[0] / span).floor() as i64 - ring).max(0);
    let y0 = ((rect[1] / span).floor() as i64 - ring).max(0);
    let x1 = ((rect[2] / span).ceil() as i64 + ring).min(across as i64);
    let y1 = ((rect[3] / span).ceil() as i64 + ring).min(down as i64);
    let mut keys = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            keys.push(TileKey {
                level,
                x: x as u16,
                y: y as u16,
            });
        }
    }
    keys
}

fn center_distance(pyramid: &Pyramid, key: TileKey, center: [f64; 2]) -> f64 {
    let rect = pyramid.doc_rect(key);
    let dx = (rect[0] + rect[2]) as f64 / 2.0 - center[0];
    let dy = (rect[1] + rect[3]) as f64 / 2.0 - center[1];
    dx * dx + dy * dy
}

#[cfg(test)]
mod shader_tests {
    use super::present_source;

    /// The present shader parses and validates, so a mistake in it is found
    /// by the tests rather than when the canvas starts. Seiza tests its
    /// compositor's shaders.
    #[test]
    fn the_present_shader_compiles() {
        let source = present_source();
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{error:?}"));
    }
}
