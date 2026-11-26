use crate::display::{DisplayCurves, TableFormat};
use crate::synth::{TileKey, TILE};
use crate::tiles::{TileCache, TileProducer};
use crate::{Pyramid, TileSource};
use bytemuck::{Pod, Zeroable};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// The most layers one frame combines.
pub const MAX_LAYERS: usize = 24;
/// The most Finish steps one frame applies.
pub const MAX_FINISH_STEPS: usize = 8;
/// The most masks one frame draws, besides the stroke overlay.
pub const MAX_MASKS: usize = 15;
/// The most heals of single layers one frame draws.
pub const MAX_HEALS: usize = 16;
/// The most layers with heals of their own one frame draws. Each needs the
/// layer's samples as a surface-sized image, to copy from.
pub const MAX_HEALED_LAYERS: usize = 4;
/// The most masks computed from bands one frame draws.
pub const MAX_COMPUTED_MASKS: usize = 8;
/// A computed mask while it is smoothed, from 0 to 1.
const SMOOTHED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// The mask layer that holds the stroke overlay.
const OVERLAY_LAYER: u32 = MAX_MASKS as u32;

/// Where the document sits in the surface.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// The document pixel at the center of the surface.
    pub center: [f64; 2],
    /// Surface pixels per document pixel.
    pub zoom: f64,
}

/// How a layer combines with the layers beneath it. The order matches the
/// shader's `blend`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Normal,
    Screen,
    Add,
    Multiply,
    Lighten,
    Darken,
    /// The layer's brightness with the color beneath it.
    Luminance,
}

/// What an entry in a pane's list of layers is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mark {
    /// A layer: one that shows a band, or an adjustment.
    #[default]
    Layer,
    /// The start of a group. The entries up to the group's end blend
    /// together on their own, starting from nothing. Only its place in the
    /// list is used.
    GroupBegin,
    /// The end of a group: the members' blend becomes one layer with this
    /// entry's opacity, blend and mask. Adjustments of the entry's own
    /// follow it, as they follow a layer.
    GroupEnd,
    /// A layer of one color everywhere, with no band: what a mask is drawn
    /// with, so the mask itself shows.
    Solid,
}

/// One layer of the Combine stage: a band, stretched, colored and blended.
#[derive(Clone, Copy, Debug)]
pub struct Layer {
    /// Whether the entry is a layer or one end of a group. Groups do not
    /// hold groups.
    pub mark: Mark,
    /// A second mask that limits the layer: the mask of the layer it is
    /// clipped to. `None` is not clipped.
    pub clip: Option<usize>,
    /// A mask computed from bands that limits the layer, as an index into
    /// [`Pane::computed`]. `None` is no such mask.
    pub computed: Option<usize>,
    /// The band the layer shows.
    pub band: usize,
    pub color: [f32; 3],
    /// From 0 to 1. A hidden layer has 0.
    pub opacity: f32,
    pub blend: Blend,
    /// The layer's own contrast curve, from -1 to 1. 0 is none.
    pub curve: f32,
    /// A second band to blend into the first before the stretch, and how
    /// much of it, from 0 to 1. A document uses it for a correction whose
    /// strength a person moves: the band uncorrected and corrected in full.
    pub blended: Option<(usize, f32)>,
    /// The mask that limits where the layer applies. `None` is everywhere.
    pub mask: Option<usize>,
    /// For an adjustment: what it does, and by how much. An adjustment has
    /// no pixels: `band`, `color`, `blend`, `curve` and `blended` are not
    /// used. `None` is a layer that shows a band.
    pub adjustment: Option<(Adjustment, f32)>,
    /// For an adjustment: whether it changes only the band layer before it
    /// in the list, before that layer is blended. Otherwise it changes the
    /// blend of every layer before it.
    pub own: bool,
}

/// What an adjustment layer does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adjustment {
    /// Brightens or darkens by the amount in stops.
    Exposure = 1,
    /// A contrast curve. The amount runs from -1 to 1.
    Curves = 2,
    /// Saturation. 1 leaves the color unchanged.
    Saturation = 3,
}

/// The entries in one layer's stretch table.
pub const STRETCH_TABLE_WIDTH: usize = 4096;

/// What a Finish step does.
#[derive(Clone, Copy, Debug)]
pub enum FinishKind {
    /// A contrast curve. The amount runs from -1 to 1.
    Curves,
    /// Saturation. An amount of 1 leaves the image as it is.
    Saturation,
    /// A heal: the masked area is replaced by the area `offset` document
    /// pixels away, with `shift` added.
    Heal { offset: [f32; 2], shift: [f32; 3] },
}

/// One step of the Finish stage, applied to the combined image.
#[derive(Clone, Copy, Debug)]
pub struct FinishStep {
    pub kind: FinishKind,
    pub amount: f32,
    /// How strongly the step applies, from 0 to 1.
    pub opacity: f32,
    /// The mask that limits the step, as an index into [`Pane::masks`].
    /// `None` is everywhere. A heal's mask is its stroke.
    pub mask: Option<usize>,
}

/// One band a computed mask reads, stretched.
#[derive(Clone, Copy, Debug)]
pub struct MaskBand {
    pub band: usize,
    /// A second band blended into the first before the stretch, and how
    /// much of it, as for a layer.
    pub blended: Option<(usize, f32)>,
    /// The row of the pane's stretch tables that holds the band's stretch.
    pub stretch_row: usize,
}

/// A mask computed at each pixel from the stretched samples of one or two
/// bands: their product, shaped.
#[derive(Clone, Copy, Debug)]
pub struct ComputedMask {
    pub bands: [Option<MaskBand>; 2],
    /// Whether the product is raised to the power of 1 minus itself.
    pub power_of_inverse: bool,
    /// The power the shaped product is raised to. 1 leaves it as it is.
    pub reach: f32,
    /// How far the mask is smoothed before the strokes: the standard
    /// deviation of a Gaussian blur, in document pixels. 0 is not at all.
    pub smoothing: f32,
    /// Strokes that set the mask to 1 under them, as an index into
    /// [`Pane::masks`].
    pub added: Option<usize>,
    /// Strokes that set the mask to 0 under them, as an index into
    /// [`Pane::masks`].
    pub removed: Option<usize>,
    /// Whether the mask is 1 minus all of the above.
    pub inverted: bool,
}

/// A heal of one layer: the masked area takes the layer's own samples from
/// `offset` document pixels away.
#[derive(Clone, Copy, Debug)]
pub struct LayerHeal {
    /// The layer healed, as an index into [`Pane::layers`].
    pub layer: usize,
    /// Whether the heal changes the band's samples before the stretch, as
    /// a heal of an input does. Otherwise it changes the stretched layer.
    pub before_stretch: bool,
    pub offset: [f32; 2],
    /// The brightness added to the copied samples.
    pub shift: f32,
    /// The heal's stroke, as an index into [`Pane::masks`].
    pub mask: usize,
}

/// One segment of a brush stroke, in document pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Segment {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub radius: f32,
    pub hardness: f32,
    /// 1 adds to the mask, 0 subtracts from it.
    pub value: f32,
    pub _pad: f32,
}

/// What one pane of the canvas draws.
pub struct Pane<'a> {
    /// The layers, bottom first. At most [`MAX_LAYERS`].
    pub layers: &'a [Layer],
    /// Each layer's stretch, in the layers' order: [`STRETCH_TABLE_WIDTH`]
    /// outputs from 0 to 1. Entry `i` is the stretch of the band sample
    /// `(i / (width - 1))²`, where a band's samples run from 0 to 1.
    pub stretches: &'a [Vec<f32>],
    /// The Finish steps, in order. At most [`MAX_FINISH_STEPS`].
    pub finish: &'a [FinishStep],
    /// The masks the layers and the Finish steps refer to. At most
    /// [`MAX_MASKS`].
    pub masks: &'a [Vec<Segment>],
    /// The masks computed from bands that the layers refer to. At most
    /// [`MAX_COMPUTED_MASKS`].
    pub computed: &'a [ComputedMask],
    /// Heals of single layers, in order. At most [`MAX_HEALS`], on at most
    /// [`MAX_HEALED_LAYERS`] layers.
    pub heals: &'a [LayerHeal],
    /// Where the pane's image is laid out in the surface: x, y, width and
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
    /// Changes whenever a pane's stretches do, so a single pane's unchanged
    /// tables are not uploaded again.
    pub stretches_version: u64,
    /// Changes whenever a pane's masks do, so a single pane's unchanged
    /// masks are not uploaded again.
    pub masks_version: u64,
    /// Strokes shown over the image: a stroke being drawn.
    pub overlay: &'a [Segment],
    /// How strongly the overlay shows, from 0 to 1.
    pub overlay_strength: f32,
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
    /// GPU time of the frame's passes, from the most recent frame whose
    /// timestamps have come back.
    pub gpu_ms: Option<f32>,
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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view: [f32; 4],
    screen: [f32; 4],
    counts: [f32; 4],
    layers: [[f32; 20]; MAX_LAYERS],
    finish: [[f32; 12]; MAX_FINISH_STEPS],
    healed: [[f32; 4]; MAX_HEALED_LAYERS],
    heals: [[f32; 8]; MAX_HEALS],
    computed: [[f32; 16]; MAX_COMPUTED_MASKS],
    display_matrix: [f32; 12],
    display: [f32; 4],
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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TileInstance {
    rect: [f32; 4],
    uv_rect: [f32; 4],
}

const MAX_TILE_INSTANCES: u64 = 4096;
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// The combined image, before the Finish steps. Float, so a layer's sum is
/// not clipped before a step that would bring it back into range.
const COMBINED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// A healed layer's samples, as they are in a tile.
const HEALED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

/// The textures that are the size of the surface.
struct Targets {
    combined: wgpu::TextureView,
    /// One view per mask layer, to draw strokes into.
    mask_layers: Vec<wgpu::TextureView>,
    /// The combined image, the masks and the display curves, for the finish
    /// pass.
    finish_inputs: wgpu::BindGroup,
    /// The masks and the healed layers' samples, for the layers pass.
    layer_masks: wgpu::BindGroup,
    /// One view per healed layer, to draw its samples into.
    healed_layers: Vec<wgpu::TextureView>,
    /// One view per computed mask, to draw the mask into and to hold it
    /// smoothed; and a second set, for the pass between the two blurs.
    smoothed: [Vec<wgpu::TextureView>; 2],
    /// Each set as what a blur pass reads.
    smoothed_sources: [wgpu::BindGroup; 2],
}

/// A growable buffer of stroke segments.
struct StrokeBuffer {
    buffer: wgpu::Buffer,
    capacity: usize,
}

impl StrokeBuffer {
    fn new(device: &wgpu::Device, capacity: usize) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stroke instances"),
            size: (capacity * size_of::<Segment>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, capacity }
    }

    /// Makes room for `count` segments. Returns whether the buffer was
    /// replaced, which loses what it held.
    fn reserve(&mut self, device: &wgpu::Device, count: usize) -> bool {
        if count <= self.capacity {
            return false;
        }
        *self = Self::new(device, count.next_power_of_two());
        true
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    globals: wgpu::Buffer,
    common: wgpu::BindGroup,
    stretch_table: wgpu::Texture,
    stretches_version: Option<u64>,
    finish_layout: wgpu::BindGroupLayout,
    masks_layout: wgpu::BindGroupLayout,
    targets: Targets,
    display_curves: wgpu::TextureView,
    display_uniform: Option<([f32; 12], [f32; 4])>,
    surface_offers: String,
    tile_layout: wgpu::BindGroupLayout,
    tile_pipeline: wgpu::RenderPipeline,
    /// Draws the samples of the layers that have heals of their own.
    healed_pipeline: wgpu::RenderPipeline,
    /// The healed layers the pane being drawn has.
    healed_count: usize,
    /// Draws the computed masks that are smoothed, before the blurs.
    smoothed_pipeline: wgpu::RenderPipeline,
    /// The blur across, then the blur down.
    blur_pipelines: [wgpu::RenderPipeline; 2],
    /// Whether the pane being drawn has a mask to smooth.
    smooths: bool,
    stroke_pipeline: wgpu::RenderPipeline,
    finish_pipeline: wgpu::RenderPipeline,
    tile_instances: wgpu::Buffer,
    /// The stroke being drawn. It only grows or resets, so only its new tail
    /// is uploaded.
    overlay: StrokeBuffer,
    overlay_uploaded: usize,
    /// Every mask's segments, one mask after another.
    masks: StrokeBuffer,
    /// Where each mask's segments are in `masks`: first index and count.
    mask_ranges: Vec<(u32, u32)>,
    masks_version: Option<u64>,
    cache: TileCache,
    /// The most bytes of tiles kept on the GPU.
    cache_budget_bytes: u64,
    producer: TileProducer,
    /// The threads that compute tiles.
    workers: usize,
    /// The document's size and levels.
    pyramid: Pyramid,
    /// The bands in the document.
    bands: usize,
    timing: Option<GpuTiming>,
    frame: u64,
    /// The most tiles uploaded in one frame. Each is 4 MB.
    pub uploads_per_frame: u32,
    /// What the last frame drew, for [`Self::offscreen_frame_ms`].
    last_sources: Vec<TileKey>,
    last_overlay: u32,
    /// Set to copy the next frame into [`Self::captured`].
    pub capture_next: bool,
    /// The last captured frame: width, height, and BGRA bytes.
    pub captured: Option<(u32, u32, Vec<u8>)>,
}

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
        let timestamps = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("studio canvas"),
                required_features: adapter.features()
                    & (wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::FLOAT32_FILTERABLE),
                // The adapter's own limits: the default caps a texture at
                // 8192 pixels, which a window across two large displays
                // passes.
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .map_err(|error| format!("no GPU device: {error}"))?;

        let capabilities = surface.get_capabilities(&adapter);
        // The shader writes display-referred values, so the surface must not
        // apply the sRGB curve a second time.
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

        let module = |label: &str, body: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(
                    format!(
                        "{}\n{}\n{body}",
                        crate::display::SHADER,
                        include_str!("globals.wgsl")
                    )
                    .into(),
                ),
            })
        };
        let layers_shader = module("layers", include_str!("layers.wgsl"));
        let finish_shader = module("finish", include_str!("finish.wgsl"));

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture_entry =
            |binding: u32, view_dimension: wgpu::TextureViewDimension| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension,
                    multisampled: false,
                },
                count: None,
            };
        let common_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals and sampler"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
                texture_entry(2, wgpu::TextureViewDimension::D2),
            ],
        });
        let tile_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tile"),
            entries: &[texture_entry(0, wgpu::TextureViewDimension::D2Array)],
        });
        let masks_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer masks"),
            entries: &[
                texture_entry(0, wgpu::TextureViewDimension::D2Array),
                texture_entry(1, wgpu::TextureViewDimension::D2Array),
                texture_entry(2, wgpu::TextureViewDimension::D2Array),
            ],
        });
        let finish_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("finish inputs"),
            entries: &[
                texture_entry(0, wgpu::TextureViewDimension::D2),
                texture_entry(1, wgpu::TextureViewDimension::D2Array),
                texture_entry(2, wgpu::TextureViewDimension::D2),
            ],
        });

        const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let pipeline = |label: &str,
                        shader: &wgpu::ShaderModule,
                        second: Option<&wgpu::BindGroupLayout>,
                        third: Option<&wgpu::BindGroupLayout>,
                        entries: (&str, &str),
                        instanced: bool,
                        targets: &[Option<wgpu::ColorTargetState>]| {
            let groups = [Some(&common_layout), second, third];
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &groups[..if third.is_some() { 3 } else { 2 }],
                ..Default::default()
            });
            let instances = [Some(wgpu::VertexBufferLayout {
                array_stride: 32,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &INSTANCE_ATTRIBUTES,
            })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(entries.0),
                    buffers: if instanced { &instances } else { &[] },
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(entries.1),
                    targets,
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let tile_pipeline = pipeline(
            "layers",
            &layers_shader,
            Some(&tile_layout),
            Some(&masks_layout),
            ("tile_vertex", "tile_fragment"),
            true,
            &[Some(COMBINED_FORMAT.into())],
        );
        let healed_pipeline = pipeline(
            "healed layers",
            &layers_shader,
            Some(&tile_layout),
            None,
            ("tile_vertex", "healed_fragment"),
            true,
            &std::array::from_fn::<_, MAX_HEALED_LAYERS, _>(|_| Some(HEALED_FORMAT.into())),
        );
        let smoothed_targets =
            std::array::from_fn::<_, MAX_COMPUTED_MASKS, _>(|_| Some(SMOOTHED_FORMAT.into()));
        let smoothed_pipeline = pipeline(
            "computed masks",
            &layers_shader,
            Some(&tile_layout),
            None,
            ("tile_vertex", "computed_fragment"),
            true,
            &smoothed_targets,
        );
        // A blur reads the masks through the tile binding: both are arrays
        // of single-channel images.
        let blur_pipelines = ["blur_across_fragment", "blur_down_fragment"].map(|fragment| {
            pipeline(
                "mask blur",
                &layers_shader,
                Some(&tile_layout),
                None,
                ("whole_vertex", fragment),
                false,
                &smoothed_targets,
            )
        });
        let stroke_pipeline = pipeline(
            "strokes",
            &layers_shader,
            None,
            None,
            ("stroke_vertex", "stroke_fragment"),
            true,
            &[Some(wgpu::ColorTargetState {
                format: MASK_FORMAT,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::RED,
            })],
        );
        let finish_pipeline = pipeline(
            "finish",
            &finish_shader,
            Some(&finish_layout),
            None,
            ("finish_vertex", "finish_fragment"),
            false,
            &[Some(format.into())],
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let stretch_table = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stretch table"),
            size: wgpu::Extent3d {
                width: STRETCH_TABLE_WIDTH as u32,
                height: MAX_LAYERS as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let stretch_view = stretch_table.create_view(&wgpu::TextureViewDescriptor::default());
        let common = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals and sampler"),
            layout: &common_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&stretch_view),
                },
            ],
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
        let display_uniform = output.curves.as_ref().map(|curves| {
            (
                curves.matrix_uniform(),
                [curves.width as f32, 0.0, 0.0, 0.0],
            )
        });
        let targets = create_targets(
            &device,
            &finish_layout,
            &masks_layout,
            &tile_layout,
            &display_curves,
            config.width,
            config.height,
        );

        let tile_instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tile instances"),
            size: MAX_TILE_INSTANCES * size_of::<TileInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // The coarsest level is always loaded, so a view never shows a hole.
        let pyramid = source.pyramid();
        let bands = source.bands().max(1);
        let mut cache = TileCache::new(cache_budget_bytes, bands);
        let (across, down) = pyramid.tiles_across(pyramid.max_level());
        for y in 0..down as u16 {
            for x in 0..across as u16 {
                let key = TileKey {
                    level: pyramid.max_level(),
                    x,
                    y,
                };
                cache.insert(&device, &queue, &tile_layout, key, &source.tile(key), 0);
                cache.pin(key);
            }
        }

        let timing = timestamps.then(|| GpuTiming::new(&device, queue.get_timestamp_period()));
        Ok(Self {
            overlay: StrokeBuffer::new(&device, 4096),
            masks: StrokeBuffer::new(&device, 4096),
            device,
            queue,
            surface,
            config,
            globals,
            common,
            stretch_table,
            stretches_version: None,
            finish_layout,
            masks_layout,
            targets,
            display_curves,
            display_uniform,
            surface_offers,
            tile_layout,
            tile_pipeline,
            healed_pipeline,
            healed_count: 0,
            smoothed_pipeline,
            blur_pipelines,
            smooths: false,
            stroke_pipeline,
            finish_pipeline,
            tile_instances,
            overlay_uploaded: 0,
            mask_ranges: Vec::new(),
            masks_version: None,
            cache,
            cache_budget_bytes,
            workers,
            producer: TileProducer::new(workers, source),
            pyramid,
            bands,
            timing,
            frame: 0,
            uploads_per_frame: 6,
            last_sources: Vec::new(),
            last_overlay: 0,
            capture_next: false,
            captured: None,
        })
    }

    /// Draws another document from now on, such as a store that was
    /// prepared again. Every tile of the document before is dropped.
    pub fn replace_source(&mut self, source: Arc<dyn TileSource>) {
        let pyramid = source.pyramid();
        let bands = source.bands().max(1);
        let mut cache = TileCache::new(self.cache_budget_bytes, bands);
        // The coarsest level is always loaded, so a view never shows a hole.
        let (across, down) = pyramid.tiles_across(pyramid.max_level());
        for y in 0..down as u16 {
            for x in 0..across as u16 {
                let key = TileKey {
                    level: pyramid.max_level(),
                    x,
                    y,
                };
                cache.insert(
                    &self.device,
                    &self.queue,
                    &self.tile_layout,
                    key,
                    &source.tile(key),
                    self.frame,
                );
                cache.pin(key);
            }
        }
        self.cache = cache;
        self.producer = TileProducer::new(self.workers, source);
        self.pyramid = pyramid;
        self.bands = bands;
        self.last_sources.clear();
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
        self.targets = create_targets(
            &self.device,
            &self.finish_layout,
            &self.masks_layout,
            &self.tile_layout,
            &self.display_curves,
            width,
            height,
        );
    }

    /// Draws one frame and presents it.
    pub fn render(&mut self, frame: &Frame) -> Result<FrameStats, String> {
        self.frame += 1;
        let mut stats = FrameStats::default();
        let _ = self.device.poll(wgpu::PollType::Poll);
        if let Some(timing) = &mut self.timing {
            timing.collect();
        }

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
        let pane_count = frame.panes.len().max(1);
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
                &self.tile_layout,
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
        wanted.retain(|key| !self.cache.contains(*key));
        self.producer.set_wanted(wanted);

        let mut instances: Vec<TileInstance> = Vec::with_capacity(visible_keys.len());
        let mut sources: Vec<TileKey> = Vec::with_capacity(visible_keys.len());
        for key in visible_keys.iter().take(MAX_TILE_INSTANCES as usize) {
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
            let rect = self.pyramid.doc_rect(*key);
            let span = (TILE << source.level) as f32;
            let origin = [source.x as f32 * span, source.y as f32 * span];
            instances.push(TileInstance {
                rect,
                uv_rect: [
                    (rect[0] - origin[0]) / span,
                    (rect[1] - origin[1]) / span,
                    (rect[2] - origin[0]) / span,
                    (rect[3] - origin[1]) / span,
                ],
            });
            sources.push(source);
        }
        stats.tiles_drawn = instances.len() as u32;
        self.queue
            .write_buffer(&self.tile_instances, 0, bytemuck::cast_slice(&instances));

        self.upload_overlay(frame);

        let target_view = target
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let timed = self.timing.as_mut().and_then(GpuTiming::begin);
        let overlay = frame.overlay.len() as u32;
        // Captures are written as 8-bit PNG files, so only an 8-bit surface
        // is captured.
        let eight_bit = self.config.format == wgpu::TextureFormat::Bgra8Unorm;
        let capture_texture = (std::mem::take(&mut self.capture_next) && eight_bit)
            .then(|| self.surface_sized("capture", wgpu::TextureUsages::COPY_SRC));
        let capture_view = capture_texture
            .as_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        // Several panes share the uniform buffer, the stretch table and the
        // mask textures, so each pane is written and submitted in turn.
        let alone = pane_count == 1;
        if !alone {
            self.stretches_version = None;
            self.masks_version = None;
        }
        for (index, pane) in frame.panes.iter().enumerate() {
            let rect = within_surface(pane.viewport, surface);
            let scissor = within_surface(pane.scissor, surface);
            self.write_globals(frame, pane, rect);
            self.upload_stretches(pane, alone.then_some(frame.stretches_version));
            self.upload_masks(pane, alone.then_some(frame.masks_version));
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            let timed = timed.filter(|_| index == 0);
            self.passes(
                &mut encoder,
                &target_view,
                &sources,
                overlay,
                timed.is_some(),
                (rect, scissor),
                index == 0,
            );
            if let Some(view) = &capture_view {
                self.finish_pass(&mut encoder, view, false, (rect, scissor), index == 0);
            }
            if let (Some(slot), Some(timing)) = (timed, &self.timing) {
                timing.resolve(&mut encoder, slot);
            }
            self.queue.submit([encoder.finish()]);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let capture = capture_texture.map(|texture| {
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
        if let (Some(slot), Some(timing)) = (timed, &self.timing) {
            timing.read_back(slot);
        }
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

        self.last_sources = sources;
        self.last_overlay = overlay;
        self.cache.evict(self.frame);
        stats.encode_ms = encode_started.elapsed().as_secs_f32() * 1000.0;
        stats.gpu_ms = self.timing.as_ref().and_then(|timing| timing.latest_ms);
        stats.tiles_in_flight = self.producer.in_flight() as u32;
        stats.cache_tiles = self.cache.len() as u32;
        stats.cache_bytes = self.cache.bytes();
        Ok(stats)
    }

    /// Writes what the shaders read for one pane. `rect` is the pane's place
    /// in the surface.
    fn write_globals(&mut self, frame: &Frame, pane: &Pane, rect: [u32; 4]) {
        let (width, height) = (rect[2] as f64, rect[3] as f64);
        let view = frame.view;
        let scale = [2.0 * view.zoom / width, -2.0 * view.zoom / height];
        let layers = &pane.layers[..pane.layers.len().min(MAX_LAYERS)];
        let finish = &pane.finish[..pane.finish.len().min(MAX_FINISH_STEPS)];
        let mut globals = Globals {
            view: [
                scale[0] as f32,
                scale[1] as f32,
                (-view.center[0] * scale[0]) as f32,
                (-view.center[1] * scale[1]) as f32,
            ],
            // Passes read the masks and the combined image by surface
            // pixel, so this is the surface's size, not the pane's.
            screen: [
                self.config.width as f32,
                self.config.height as f32,
                frame.overlay_strength,
                OVERLAY_LAYER as f32,
            ],
            counts: [layers.len() as f32, finish.len() as f32, 0.0, 0.0],
            layers: [[0.0; 20]; MAX_LAYERS],
            computed: [[0.0; 16]; MAX_COMPUTED_MASKS],
            finish: [[0.0; 12]; MAX_FINISH_STEPS],
            healed: [[-1.0, -1.0, 0.0, 0.0]; MAX_HEALED_LAYERS],
            heals: [[0.0; 8]; MAX_HEALS],
            display_matrix: self.display_uniform.map_or([0.0; 12], |display| display.0),
            display: self.display_uniform.map_or([0.0; 4], |display| display.1),
        };
        // A band layer's stretch is the next row of the stretch table; an
        // adjustment layer has none.
        let mut row = 0;
        for (slot, layer) in globals.layers.iter_mut().zip(layers) {
            // A mask the frame does not hold limits nothing.
            let mask = layer
                .mask
                .filter(|mask| *mask < pane.masks.len().min(MAX_MASKS))
                .map_or(-1.0, |mask| mask as f32);
            let (kind, amount) = match layer.mark {
                Mark::GroupBegin => (10.0, 0.0),
                Mark::GroupEnd => (11.0, 0.0),
                Mark::Solid => (12.0, 0.0),
                Mark::Layer => layer
                    .adjustment
                    .map_or((0.0, 0.0), |(kind, amount)| (kind as u32 as f32, amount)),
            };
            let clip = layer
                .clip
                .filter(|mask| *mask < pane.masks.len().min(MAX_MASKS))
                .map_or(-1.0, |mask| mask as f32);
            let shows_band = layer.mark == Mark::Layer && layer.adjustment.is_none();
            *slot = [
                layer.color[0],
                layer.color[1],
                layer.color[2],
                layer.opacity.clamp(0.0, 1.0),
                row as f32,
                // The layer's place among the healed layers; none yet.
                -1.0,
                clip,
                layer.curve.clamp(-1.0, 1.0),
                layer.band.min(self.bands - 1) as f32,
                layer.blend as u32 as f32,
                layer
                    .blended
                    .map_or(-1.0, |(band, _)| band.min(self.bands - 1) as f32),
                layer
                    .blended
                    .map_or(0.0, |(_, amount)| amount.clamp(0.0, 1.0)),
                kind,
                amount,
                mask,
                if layer.own { 1.0 } else { 0.0 },
                layer
                    .computed
                    .filter(|mask| *mask < pane.computed.len().min(MAX_COMPUTED_MASKS))
                    .map_or(-1.0, |mask| mask as f32),
                0.0,
                0.0,
                0.0,
            ];
            if shows_band {
                row += 1;
            }
        }
        for (slot, mask) in globals.computed.iter_mut().zip(pane.computed) {
            let band = |band: Option<MaskBand>| match band {
                Some(band) => [
                    band.band.min(self.bands - 1) as f32,
                    band.blended
                        .map_or(-1.0, |(other, _)| other.min(self.bands - 1) as f32),
                    band.blended
                        .map_or(0.0, |(_, amount)| amount.clamp(0.0, 1.0)),
                    band.stretch_row.min(MAX_LAYERS - 1) as f32,
                ],
                None => [-1.0, -1.0, 0.0, 0.0],
            };
            let (first, second) = (band(mask.bands[0]), band(mask.bands[1]));
            slot[..4].copy_from_slice(&first);
            slot[4..8].copy_from_slice(&second);
            let strokes = |mask: Option<usize>| {
                mask.filter(|mask| *mask < pane.masks.len().min(MAX_MASKS))
                    .map_or(-1.0, |mask| mask as f32)
            };
            slot[8] = if mask.power_of_inverse { 1.0 } else { 0.0 };
            slot[9] = if mask.inverted { 1.0 } else { 0.0 };
            slot[10] = mask.reach.clamp(0.05, 20.0);
            slot[12] = strokes(mask.added);
            slot[13] = strokes(mask.removed);
            // The blur's reach in surface pixels.
            slot[14] = (f64::from(mask.smoothing.max(0.0)) * view.zoom) as f32;
        }
        self.smooths = pane
            .computed
            .iter()
            .take(MAX_COMPUTED_MASKS)
            .any(|mask| mask.smoothing > 0.0);
        // Each layer with heals of its own gets a place among the healed
        // layers. A heal on a layer with no place left is not drawn.
        let mut healed: Vec<usize> = Vec::new();
        let mut heal_count = 0;
        for heal in pane.heals {
            let known = healed.iter().position(|layer| *layer == heal.layer);
            let Some(layer) = layers
                .get(heal.layer)
                .filter(|layer| layer.mark == Mark::Layer && layer.adjustment.is_none())
            else {
                continue;
            };
            if heal_count == MAX_HEALS
                || heal.mask >= pane.masks.len().min(MAX_MASKS)
                || (known.is_none() && healed.len() == MAX_HEALED_LAYERS)
            {
                continue;
            }
            let place = known.unwrap_or_else(|| {
                healed.push(heal.layer);
                healed.len() - 1
            });
            globals.layers[heal.layer][5] = place as f32;
            globals.healed[place] = [
                layer.band.min(self.bands - 1) as f32,
                layer
                    .blended
                    .map_or(-1.0, |(band, _)| band.min(self.bands - 1) as f32),
                layer
                    .blended
                    .map_or(0.0, |(_, amount)| amount.clamp(0.0, 1.0)),
                0.0,
            ];
            globals.heals[heal_count] = [
                heal.layer as f32,
                if heal.before_stretch { 0.0 } else { 1.0 },
                heal.mask as f32,
                heal.shift,
                // The offset in whole surface pixels.
                (heal.offset[0] as f64 * view.zoom).round() as f32,
                (heal.offset[1] as f64 * view.zoom).round() as f32,
                0.0,
                0.0,
            ];
            heal_count += 1;
        }
        globals.counts[2] = heal_count as f32;
        self.healed_count = healed.len();
        for (slot, step) in globals.finish.iter_mut().zip(finish) {
            // A mask the frame does not hold limits nothing.
            let mask = step
                .mask
                .filter(|mask| *mask < pane.masks.len().min(MAX_MASKS))
                .map_or(-1.0, |mask| mask as f32);
            let (kind, offset, shift) = match step.kind {
                FinishKind::Curves => (1.0, [0.0; 2], [0.0; 3]),
                FinishKind::Saturation => (2.0, [0.0; 2], [0.0; 3]),
                FinishKind::Heal { offset, shift } => (3.0, offset, shift),
            };
            *slot = [
                kind,
                step.amount,
                mask,
                step.opacity.clamp(0.0, 1.0),
                // The offset in whole surface pixels.
                (offset[0] as f64 * view.zoom).round() as f32,
                (offset[1] as f64 * view.zoom).round() as f32,
                0.0,
                0.0,
                shift[0],
                shift[1],
                shift[2],
                0.0,
            ];
        }
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
    }

    /// Uploads a pane's stretch tables. With a `version`, tables of a
    /// version already uploaded are left as they are.
    fn upload_stretches(&mut self, pane: &Pane, version: Option<u64>) {
        if version.is_some() && self.stretches_version == version {
            return;
        }
        let mut bits = vec![0u16; STRETCH_TABLE_WIDTH * MAX_LAYERS];
        for (row, table) in pane.stretches.iter().take(MAX_LAYERS).enumerate() {
            let target = &mut bits[row * STRETCH_TABLE_WIDTH..(row + 1) * STRETCH_TABLE_WIDTH];
            for (bits, value) in target.iter_mut().zip(table) {
                *bits = half::f16::from_f32(*value).to_bits();
            }
        }
        self.queue.write_texture(
            self.stretch_table.as_image_copy(),
            bytemuck::cast_slice(&bits),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(STRETCH_TABLE_WIDTH as u32 * 2),
                rows_per_image: None,
            },
            self.stretch_table.size(),
        );
        self.stretches_version = version;
    }

    fn upload_overlay(&mut self, frame: &Frame) {
        // The overlay only grows or resets, so upload its new tail.
        if frame.overlay.len() < self.overlay_uploaded
            || self.overlay.reserve(&self.device, frame.overlay.len())
        {
            self.overlay_uploaded = 0;
        }
        if frame.overlay.len() > self.overlay_uploaded {
            self.queue.write_buffer(
                &self.overlay.buffer,
                (self.overlay_uploaded * size_of::<Segment>()) as u64,
                bytemuck::cast_slice(&frame.overlay[self.overlay_uploaded..]),
            );
            self.overlay_uploaded = frame.overlay.len();
        }
    }

    /// Uploads a pane's masks. With a `version`, masks of a version already
    /// uploaded are left as they are.
    fn upload_masks(&mut self, pane: &Pane, version: Option<u64>) {
        if version.is_some() && self.masks_version == version {
            return;
        }
        let masks = &pane.masks[..pane.masks.len().min(MAX_MASKS)];
        let all: Vec<Segment> = masks.iter().flatten().copied().collect();
        self.masks.reserve(&self.device, all.len());
        self.queue
            .write_buffer(&self.masks.buffer, 0, bytemuck::cast_slice(&all));
        let mut first = 0;
        self.mask_ranges = masks
            .iter()
            .map(|mask| {
                let range = (first, mask.len() as u32);
                first += mask.len() as u32;
                range
            })
            .collect();
        self.masks_version = version;
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

    /// Every pass of one pane: the masks, the layers, then the finish steps
    /// into `rect` of `target`. `first` clears the target; later panes keep
    /// what the panes before them drew.
    #[allow(clippy::too_many_arguments)]
    fn passes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        sources: &[TileKey],
        overlay: u32,
        timed: bool,
        (rect, scissor): ([u32; 4], [u32; 4]),
        first: bool,
    ) {
        let within = |pass: &mut wgpu::RenderPass| {
            pass.set_viewport(
                rect[0] as f32,
                rect[1] as f32,
                rect[2] as f32,
                rect[3] as f32,
                0.0,
                1.0,
            );
            pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
        };
        let timestamps = |begin: Option<u32>, end: Option<u32>| {
            timed.then(|| wgpu::RenderPassTimestampWrites {
                query_set: &self.timing.as_ref().expect("timing").set,
                beginning_of_pass_write_index: begin,
                end_of_pass_write_index: end,
            })
        };

        // Each mask into its own layer, and the overlay into the last one.
        let strokes = self
            .mask_ranges
            .iter()
            .enumerate()
            .map(|(layer, range)| (layer, &self.masks.buffer, *range))
            .chain([(OVERLAY_LAYER as usize, &self.overlay.buffer, (0, overlay))]);
        for (index, (layer, buffer, (first, count))) in strokes.enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.mask_layers[layer],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                timestamp_writes: if index == 0 {
                    timestamps(Some(0), None)
                } else {
                    None
                },
                ..Default::default()
            });
            within(&mut pass);
            if count > 0 {
                pass.set_pipeline(&self.stroke_pipeline);
                pass.set_bind_group(0, &self.common, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..4, first..first + count);
            }
        }

        // The samples of each layer with heals of its own, so a heal can
        // copy them from another place.
        if self.healed_count > 0 {
            let attachments: Vec<_> = self
                .targets
                .healed_layers
                .iter()
                .map(|view| {
                    Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })
                })
                .collect();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("healed layers"),
                color_attachments: &attachments,
                ..Default::default()
            });
            within(&mut pass);
            pass.set_pipeline(&self.healed_pipeline);
            pass.set_bind_group(0, &self.common, &[]);
            pass.set_vertex_buffer(0, self.tile_instances.slice(..));
            for (index, source) in sources.iter().enumerate() {
                let tile = self.cache.get(*source).expect("a chosen tile");
                pass.set_bind_group(1, &tile.bind_group, &[]);
                pass.draw(0..4, index as u32..index as u32 + 1);
            }
        }

        // The computed masks that are smoothed: drawn, blurred across into
        // the second set, then blurred down back into the first, which the
        // layers read.
        if self.smooths {
            fn attachments(
                views: &[wgpu::TextureView],
            ) -> Vec<Option<wgpu::RenderPassColorAttachment<'_>>> {
                views
                    .iter()
                    .map(|view| {
                        Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })
                    })
                    .collect()
            }
            {
                let targets = attachments(&self.targets.smoothed[0]);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("computed masks"),
                    color_attachments: &targets,
                    ..Default::default()
                });
                within(&mut pass);
                pass.set_pipeline(&self.smoothed_pipeline);
                pass.set_bind_group(0, &self.common, &[]);
                pass.set_vertex_buffer(0, self.tile_instances.slice(..));
                for (index, source) in sources.iter().enumerate() {
                    let tile = self.cache.get(*source).expect("a chosen tile");
                    pass.set_bind_group(1, &tile.bind_group, &[]);
                    pass.draw(0..4, index as u32..index as u32 + 1);
                }
            }
            for (blur, pipeline) in self.blur_pipelines.iter().enumerate() {
                let targets = attachments(&self.targets.smoothed[1 - blur]);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("mask blur"),
                    color_attachments: &targets,
                    ..Default::default()
                });
                within(&mut pass);
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &self.common, &[]);
                pass.set_bind_group(1, &self.targets.smoothed_sources[blur], &[]);
                pass.draw(0..3, 0..1);
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("layers"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.combined,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.03,
                            g: 0.03,
                            b: 0.035,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            within(&mut pass);
            pass.set_pipeline(&self.tile_pipeline);
            pass.set_bind_group(0, &self.common, &[]);
            pass.set_bind_group(2, &self.targets.layer_masks, &[]);
            pass.set_vertex_buffer(0, self.tile_instances.slice(..));
            for (index, source) in sources.iter().enumerate() {
                let tile = self.cache.get(*source).expect("a chosen tile");
                pass.set_bind_group(1, &tile.bind_group, &[]);
                pass.draw(0..4, index as u32..index as u32 + 1);
            }
        }
        self.finish_pass(encoder, target, timed, (rect, scissor), first);
    }

    fn finish_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        timed: bool,
        (rect, scissor): ([u32; 4], [u32; 4]),
        first: bool,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("finish"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if first {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            timestamp_writes: timed.then(|| wgpu::RenderPassTimestampWrites {
                query_set: &self.timing.as_ref().expect("timing").set,
                beginning_of_pass_write_index: None,
                end_of_pass_write_index: Some(1),
            }),
            ..Default::default()
        });
        pass.set_viewport(
            rect[0] as f32,
            rect[1] as f32,
            rect[2] as f32,
            rect[3] as f32,
            0.0,
            1.0,
        );
        pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
        pass.set_pipeline(&self.finish_pipeline);
        pass.set_bind_group(0, &self.common, &[]);
        pass.set_bind_group(1, &self.targets.finish_inputs, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Draws the last frame again `frames` times into a texture of the
    /// surface's size and waits for the GPU to finish, five times over.
    /// Returns the time per frame of the fastest batch, which is the GPU's
    /// cost with no display to wait for.
    pub fn offscreen_frame_ms(&self, frames: u32) -> f32 {
        let texture = self.surface_sized("offscreen", wgpu::TextureUsages::empty());
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Other programs use the GPU too. The fastest of several batches is
        // the one they disturbed least.
        let mut best = f32::INFINITY;
        for _ in 0..5 {
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            let started = Instant::now();
            for _ in 0..frames {
                let mut encoder = self
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                self.passes(
                    &mut encoder,
                    &view,
                    &self.last_sources,
                    self.last_overlay,
                    false,
                    (
                        [0, 0, self.config.width, self.config.height],
                        [0, 0, self.config.width, self.config.height],
                    ),
                    true,
                );
                self.queue.submit([encoder.finish()]);
            }
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            best = best.min(started.elapsed().as_secs_f32() * 1000.0 / frames.max(1) as f32);
        }
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

fn create_targets(
    device: &wgpu::Device,
    finish_layout: &wgpu::BindGroupLayout,
    masks_layout: &wgpu::BindGroupLayout,
    tile_layout: &wgpu::BindGroupLayout,
    display_curves: &wgpu::TextureView,
    width: u32,
    height: u32,
) -> Targets {
    let texture = |label: &str, format: wgpu::TextureFormat, layers: u32| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    };
    let combined = texture("combined", COMBINED_FORMAT, 1)
        .create_view(&wgpu::TextureViewDescriptor::default());
    let masks = texture("masks", MASK_FORMAT, MAX_MASKS as u32 + 1);
    let mask_layers = (0..=MAX_MASKS as u32)
        .map(|layer| {
            masks.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            })
        })
        .collect();
    let all_masks = masks.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let finish_inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("finish inputs"),
        layout: finish_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&combined),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&all_masks),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(display_curves),
            },
        ],
    });
    let healed = texture("healed layers", HEALED_FORMAT, MAX_HEALED_LAYERS as u32);
    let healed_layers = (0..MAX_HEALED_LAYERS as u32)
        .map(|layer| {
            healed.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            })
        })
        .collect();
    let all_healed = healed.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    // Two sets of images for the computed masks: one to draw and to end
    // in, one for the pass between the two blurs.
    let smoothed_sets = ["smoothed masks", "masks half blurred"].map(|label| {
        let texture = texture(label, SMOOTHED_FORMAT, MAX_COMPUTED_MASKS as u32);
        let layers: Vec<wgpu::TextureView> = (0..MAX_COMPUTED_MASKS as u32)
            .map(|layer| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let all = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        (layers, all)
    });
    let smoothed_sources = [0, 1].map(|set: usize| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("masks to blur"),
            layout: tile_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&smoothed_sets[set].1),
            }],
        })
    });
    let layer_masks = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("layer masks"),
        layout: masks_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&all_masks),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&all_healed),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&smoothed_sets[0].1),
            },
        ],
    });
    let [(first, _), (second, _)] = smoothed_sets;
    Targets {
        combined,
        mask_layers,
        finish_inputs,
        layer_masks,
        healed_layers,
        smoothed: [first, second],
        smoothed_sources,
    }
}

const READBACK_FREE: u8 = 0;
const READBACK_PENDING: u8 = 1;
const READBACK_MAPPED: u8 = 2;

struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

/// GPU pass timing from timestamp queries. A result arrives a few frames
/// after its frame, so a small set of buffers takes turns.
struct GpuTiming {
    set: wgpu::QuerySet,
    resolved: wgpu::Buffer,
    readbacks: Vec<Readback>,
    period_ns: f32,
    latest_ms: Option<f32>,
}

impl GpuTiming {
    fn new(device: &wgpu::Device, period_ns: f32) -> Self {
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("frame timing"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolved = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame timing"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readbacks = (0..4)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("frame timing readback"),
                    size: 16,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(READBACK_FREE)),
            })
            .collect();
        Self {
            set,
            resolved,
            readbacks,
            period_ns,
            latest_ms: None,
        }
    }

    fn collect(&mut self) {
        for readback in &self.readbacks {
            if readback.state.load(Ordering::Acquire) != READBACK_MAPPED {
                continue;
            }
            {
                let bytes = readback
                    .buffer
                    .slice(..)
                    .get_mapped_range()
                    .expect("a mapped readback");
                let ticks: &[u64] = bytemuck::cast_slice(&bytes);
                let elapsed = ticks[1].saturating_sub(ticks[0]) as f32 * self.period_ns;
                self.latest_ms = Some(elapsed / 1.0e6);
            }
            readback.buffer.unmap();
            readback.state.store(READBACK_FREE, Ordering::Release);
        }
    }

    /// Claims a readback buffer for this frame, if one is free.
    fn begin(&mut self) -> Option<usize> {
        let slot = self
            .readbacks
            .iter()
            .position(|readback| readback.state.load(Ordering::Acquire) == READBACK_FREE)?;
        self.readbacks[slot]
            .state
            .store(READBACK_PENDING, Ordering::Release);
        Some(slot)
    }

    fn resolve(&self, encoder: &mut wgpu::CommandEncoder, slot: usize) {
        encoder.resolve_query_set(&self.set, 0..2, &self.resolved, 0);
        encoder.copy_buffer_to_buffer(&self.resolved, 0, &self.readbacks[slot].buffer, 0, 16);
    }

    fn read_back(&self, slot: usize) {
        let state = self.readbacks[slot].state.clone();
        self.readbacks[slot]
            .buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                state.store(
                    if result.is_ok() {
                        READBACK_MAPPED
                    } else {
                        READBACK_FREE
                    },
                    Ordering::Release,
                );
            });
    }
}
