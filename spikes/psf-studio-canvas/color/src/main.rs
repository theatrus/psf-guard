//! Investigation 9 of docs/design/psf-studio.md: is a 3D table sampled in the
//! shader an accurate display transform?
//!
//! The check has no window. It pushes 65,536 colors through the render
//! crate's `display_lookup` on the GPU and compares the result with Little
//! CMS converting the same colors directly, and compares Little CMS with
//! macOS ColorSync (through `sips`) so the reference is not one engine alone.
//!
//! usage: studio-color-spike <results.json> <display.icc>...

use lcms2::Profile;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use studio_color_spike::{convert, curves, SYSTEM_PROFILES};
use studio_render_spike::display::{DisplayCurves, DisplayTable, TableFormat, SHADER};

const SIDE: u32 = 256;
const SOURCES: [(&str, &str); 4] = [
    ("sRGB", "sRGB Profile.icc"),
    ("Display P3", "Display P3.icc"),
    ("Adobe RGB (1998)", "AdobeRGB1998.icc"),
    ("ProPhoto (ROMM RGB)", "ROMM RGB.icc"),
];

/// 65,536 colors on the 16-bit grid: half spread through the cube, a quarter
/// near neutral, a quarter on the cube's faces, where gamuts differ most.
fn test_colors() -> Vec<[f32; 3]> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        ((state >> 48) as f32) / 65535.0
    };
    (0..SIDE * SIDE)
        .map(|index| {
            let mut color = [next(), next(), next()];
            match index % 4 {
                2 => {
                    let gray = color[0];
                    color = color.map(|channel| (gray + (channel - 0.5) * 0.04).clamp(0.0, 1.0));
                }
                3 => color[(index as usize / 4) % 3] = if index % 8 == 3 { 0.0 } else { 1.0 },
                _ => {}
            }
            color.map(|channel| (channel * 65535.0).round() / 65535.0)
        })
        .collect()
}

fn table(source: &Profile, target: &Profile, size: u32) -> DisplayTable {
    let step = (size - 1) as f32;
    let mut inputs = Vec::with_capacity((size * size * size) as usize);
    for blue in 0..size {
        for green in 0..size {
            for red in 0..size {
                inputs.push([red as f32 / step, green as f32 / step, blue as f32 / step]);
            }
        }
    }
    DisplayTable {
        size,
        nodes: convert(source, target, &inputs),
    }
}

/// Differences between two sets of colors, in 8-bit levels, after both are
/// clamped to what a display can show.
fn differences(a: &[[f32; 3]], b: &[[f32; 3]]) -> Value {
    let mut all: Vec<f32> = a
        .iter()
        .zip(b)
        .flat_map(|(a, b)| {
            [0, 1, 2].map(|channel| {
                (a[channel].clamp(0.0, 1.0) - b[channel].clamp(0.0, 1.0)).abs() * 255.0
            })
        })
        .collect();
    all.sort_by(f32::total_cmp);
    let at = |fraction: f64| all[((all.len() - 1) as f64 * fraction) as usize];
    let round = |value: f32| (value as f64 * 1000.0).round() / 1000.0;
    json!({
        "mean": round(all.iter().sum::<f32>() / all.len() as f32),
        "p99": round(at(0.99)),
        "p999": round(at(0.999)),
        "max": round(at(1.0)),
    })
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    curves_pipeline: wgpu::RenderPipeline,
    input: wgpu::TextureView,
    sampler: wgpu::Sampler,
    size: wgpu::Buffer,
    full_tables: bool,
}

impl Gpu {
    fn new(colors: &[[f32; 3]]) -> Self {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("a GPU adapter");
        let full_tables = adapter
            .features()
            .contains(wgpu::Features::FLOAT32_FILTERABLE);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: if full_tables {
                wgpu::Features::FLOAT32_FILTERABLE
            } else {
                wgpu::Features::empty()
            },
            ..Default::default()
        }))
        .expect("a GPU device");

        let source = format!(
            "{SHADER}
@group(0) @binding(0) var input: texture_2d<f32>;
@group(0) @binding(1) var table: texture_3d<f32>;
@group(0) @binding(2) var table_sampler: sampler;
@group(0) @binding(3) var<uniform> size: vec4f;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {{
    let corner = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4f(corner * 2.0 - 1.0, 0.0, 1.0);
}}

@fragment
fn fragment(@builtin(position) position: vec4f) -> @location(0) vec4f {{
    let rgb = textureLoad(input, vec2i(position.xy), 0).rgb;
    return vec4f(display_lookup(table, table_sampler, size.x, rgb), 1.0);
}}
"
        );
        let curves_source = format!(
            "{SHADER}
@group(0) @binding(0) var input: texture_2d<f32>;
@group(0) @binding(1) var curves: texture_2d<f32>;
@group(0) @binding(2) var curves_sampler: sampler;
struct Settings {{ to_target: mat3x3f, width: vec4f }}
@group(0) @binding(3) var<uniform> settings: Settings;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {{
    let corner = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4f(corner * 2.0 - 1.0, 0.0, 1.0);
}}

@fragment
fn fragment(@builtin(position) position: vec4f) -> @location(0) vec4f {{
    let rgb = textureLoad(input, vec2i(position.xy), 0).rgb;
    return vec4f(display_curves(curves, curves_sampler, settings.width.x, settings.to_target, rgb), 1.0);
}}
"
        );
        let build = |label: &str, source: String| {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fragment"),
                    targets: &[Some(wgpu::TextureFormat::Rgba32Float.into())],
                    compilation_options: Default::default(),
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = build("table check", source);
        let curves_pipeline = build("curves check", curves_source);

        let extent = wgpu::Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        };
        let input = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test colors"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texels: Vec<f32> = colors
            .iter()
            .flat_map(|color| [color[0], color[1], color[2], 1.0])
            .collect();
        queue.write_texture(
            input.as_image_copy(),
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIDE * 16),
                rows_per_image: None,
            },
            extent,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let size = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("table size"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            input: input.create_view(&Default::default()),
            device,
            queue,
            pipeline,
            curves_pipeline,
            sampler,
            size,
            full_tables,
        }
    }

    /// Sends the test colors through the table and reads the result back.
    fn run(&self, table: &DisplayTable, format: TableFormat) -> Vec<[f32; 3]> {
        let view = table.upload(&self.device, &self.queue, format);
        let uniform = [table.size as f32, 0.0, 0.0, 0.0];
        self.draw(&self.pipeline, &view, bytemuck::cast_slice(&uniform))
    }

    /// The same through the curve, matrix, curve form.
    fn run_curves(&self, curves: &DisplayCurves, format: TableFormat) -> Vec<[f32; 3]> {
        let view = curves.upload(&self.device, &self.queue, format);
        let mut uniform = [0f32; 16];
        uniform[..12].copy_from_slice(&curves.matrix_uniform());
        uniform[12] = curves.width as f32;
        self.draw(&self.curves_pipeline, &view, bytemuck::cast_slice(&uniform))
    }

    fn draw(
        &self,
        pipeline: &wgpu::RenderPipeline,
        table_view: &wgpu::TextureView,
        uniform: &[u8],
    ) -> Vec<[f32; 3]> {
        self.queue.write_buffer(&self.size, 0, uniform);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.input),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(table_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.size.as_entire_binding(),
                },
            ],
        });
        let extent = wgpu::Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        };
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("converted"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("converted"),
            size: (SIDE * SIDE * 16) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Default::default(),
                })],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIDE * 16),
                    rows_per_image: None,
                },
            },
            extent,
        );
        self.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .expect("a mapped buffer");
        let texels: &[f32] = bytemuck::cast_slice(&mapped);
        texels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|texel| [texel[0], texel[1], texel[2]])
            .collect()
    }
}

fn write_png(path: &Path, colors: &[[f32; 3]]) {
    let file = std::fs::File::create(path).expect("a scratch file");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), SIDE, SIDE);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Sixteen);
    let bytes: Vec<u8> = colors
        .iter()
        .flatten()
        .flat_map(|channel| ((channel * 65535.0).round() as u16).to_be_bytes())
        .collect();
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&bytes))
        .expect("writing a PNG");
}

fn read_png(path: &Path) -> Option<Vec<[f32; 3]>> {
    let decoder = png::Decoder::new(std::fs::File::open(path).ok()?);
    let mut reader = decoder.read_info().ok()?;
    let mut bytes = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut bytes).ok()?;
    let pixels = &bytes[..info.buffer_size()];
    match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgb, png::BitDepth::Sixteen) => Some(
            pixels
                .as_chunks::<6>()
                .0
                .iter()
                .map(|pixel| {
                    [0, 2, 4]
                        .map(|at| u16::from_be_bytes([pixel[at], pixel[at + 1]]) as f32 / 65535.0)
                })
                .collect(),
        ),
        // `sips --matchTo` writes 8 bits per channel.
        (png::ColorType::Rgb, png::BitDepth::Eight) => Some(
            pixels
                .as_chunks::<3>()
                .0
                .iter()
                .map(|pixel| [0, 1, 2].map(|at| pixel[at] as f32 / 255.0))
                .collect(),
        ),
        other => {
            eprintln!("sips wrote {other:?}");
            None
        }
    }
}

/// Converts the test colors with macOS ColorSync, through `sips`.
fn colorsync(colors: &[[f32; 3]], source: &Path, target: &Path) -> Option<Vec<[f32; 3]>> {
    let scratch = std::env::temp_dir().join(format!("studio-color-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).ok()?;
    let (plain, tagged, matched) = (
        scratch.join("plain.png"),
        scratch.join("tagged.png"),
        scratch.join("matched.png"),
    );
    write_png(&plain, colors);
    let sips = |arguments: &[&std::ffi::OsStr]| {
        Command::new("sips")
            .args(arguments)
            .output()
            .is_ok_and(|output| output.status.success())
    };
    let done = sips(&[
        "--embedProfile".as_ref(),
        source.as_os_str(),
        plain.as_os_str(),
        "--out".as_ref(),
        tagged.as_os_str(),
    ]) && sips(&[
        "--matchTo".as_ref(),
        target.as_os_str(),
        tagged.as_os_str(),
        "--out".as_ref(),
        matched.as_os_str(),
    ]);
    let result = done.then(|| read_png(&matched)).flatten();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [output, displays @ ..] = arguments.as_slice() else {
        eprintln!("usage: studio-color-spike <results.json> <display.icc>...");
        std::process::exit(2);
    };
    let colors = test_colors();
    let gpu = Gpu::new(&colors);
    let system = Path::new(SYSTEM_PROFILES);
    let display_p3 = Profile::new_file(system.join("Display P3.icc")).expect("Display P3");
    let mut rows = Vec::new();

    for display_path in displays {
        let display = Profile::new_file(display_path).expect("a display profile");
        let display_name = display
            .info(lcms2::InfoType::Description, lcms2::Locale::none())
            .unwrap_or_else(|| display_path.clone());
        for (source_name, file) in SOURCES {
            let source_path = system.join(file);
            let source = Profile::new_file(&source_path).expect("a system profile");
            let direct = convert(&source, &display, &colors);

            // The shader against the engine that built its table.
            let mut tables = Vec::new();
            for size in [17u32, 33, 65] {
                let table = table(&source, &display, size);
                let mut entry = json!({
                    "size": size,
                    "half_float": differences(&gpu.run(&table, TableFormat::Half), &direct),
                });
                if gpu.full_tables {
                    entry["full_float"] = differences(&gpu.run(&table, TableFormat::Full), &direct);
                }
                tables.push(entry);
            }

            let model = curves(&source, &display);
            let mut curve_form = json!({
                "half_float": differences(&gpu.run_curves(&model, TableFormat::Half), &direct),
            });
            if gpu.full_tables {
                curve_form["full_float"] =
                    differences(&gpu.run_curves(&model, TableFormat::Full), &direct);
            }

            // The engine against ColorSync, which converts when the system
            // manages a surface.
            let matched = colorsync(&colors, &source_path, Path::new(display_path));
            if let (Some(matched), true) = (&matched, std::env::var_os("WORST").is_some()) {
                let mut order: Vec<usize> = (0..colors.len()).collect();
                let gap = |i: usize| {
                    (0..3)
                        .map(|c| (matched[i][c] - direct[i][c].clamp(0.0, 1.0)).abs())
                        .fold(0.0, f32::max)
                };
                order.sort_by(|a, b| gap(*b).total_cmp(&gap(*a)));
                for &i in order.iter().take(4) {
                    eprintln!(
                        "  in {:?} colorsync {:?} lcms {:?}",
                        colors[i].map(|v| (v * 255.0).round()),
                        matched[i].map(|v| (v * 255.0).round()),
                        direct[i].map(|v| (v.clamp(0.0, 1.0) * 2550.0).round() / 10.0)
                    );
                }
            }
            let system_result = matched.map(|matched| differences(&matched, &direct));

            // What an 8-bit Display P3 surface costs: colors go to Display P3
            // and are clipped there before the system converts them.
            let through_p3 = {
                let clipped: Vec<[f32; 3]> = convert(&source, &display_p3, &colors)
                    .into_iter()
                    .map(|color| color.map(|channel| channel.clamp(0.0, 1.0)))
                    .collect();
                let shown = convert(&display_p3, &display, &clipped);
                let changed = shown
                    .iter()
                    .zip(&direct)
                    .filter(|(a, b)| {
                        (0..3).any(|channel| {
                            (a[channel].clamp(0.0, 1.0) - b[channel].clamp(0.0, 1.0)).abs()
                                > 1.0 / 255.0
                        })
                    })
                    .count();
                json!({
                    "differences": differences(&shown, &direct),
                    "share_of_colors_off_by_more_than_one_level":
                        changed as f64 / colors.len() as f64,
                })
            };

            eprintln!("{display_name} <- {source_name}: done");
            rows.push(json!({
                "display": display_name,
                "source": source_name,
                "shader_table_against_little_cms": tables,
                "shader_curves_against_little_cms": curve_form,
                "colorsync_against_little_cms": system_result,
                "through_a_clipped_display_p3_surface": through_p3,
            }));
        }
    }

    let report = json!({
        "colors": colors.len(),
        "unit": "8-bit levels (1/255)",
        "full_float_tables_filterable": gpu.full_tables,
        "results": rows,
    });
    let text = serde_json::to_string_pretty(&report).expect("JSON");
    std::fs::write(output, &text).expect("writing the report");
}
