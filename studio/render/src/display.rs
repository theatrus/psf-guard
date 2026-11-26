//! The display transform's lookup table, as the render crate sees it.
//!
//! The host builds the table with a color engine, from the document's
//! working profile and either the display's profile or the color space set on
//! the surface. The render crate only uploads and samples it, so it depends
//! on no color engine.

/// The shader function that samples a table. Prepend it to a shader that
/// calls `display_lookup`.
pub const SHADER: &str = include_str!("display.wgsl");

/// How the table is stored on the GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableFormat {
    /// Half floats. Filterable everywhere.
    Half,
    /// 32-bit floats. Linear filtering needs `Features::FLOAT32_FILTERABLE`.
    Full,
}

/// A cube of `size` nodes per axis. Red varies fastest, then green, then
/// blue. Each node is the target's encoded RGB.
pub struct DisplayTable {
    pub size: u32,
    pub nodes: Vec<[f32; 3]>,
}

impl DisplayTable {
    pub fn upload(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: TableFormat,
    ) -> wgpu::TextureView {
        let extent = wgpu::Extent3d {
            width: self.size,
            height: self.size,
            depth_or_array_layers: self.size,
        };
        let (texture_format, bytes): (_, Vec<u8>) = match format {
            TableFormat::Half => (
                wgpu::TextureFormat::Rgba16Float,
                self.nodes
                    .iter()
                    .flat_map(|node| [node[0], node[1], node[2], 1.0])
                    .flat_map(|value| half::f16::from_f32(value).to_bits().to_le_bytes())
                    .collect(),
            ),
            TableFormat::Full => (
                wgpu::TextureFormat::Rgba32Float,
                self.nodes
                    .iter()
                    .flat_map(|node| [node[0], node[1], node[2], 1.0])
                    .flat_map(f32::to_le_bytes)
                    .collect(),
            ),
        };
        let texel = bytes.len() as u32 / (self.size * self.size * self.size);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("display table"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: texture_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.size * texel),
                rows_per_image: Some(self.size),
            },
            extent,
        );
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }
}

/// The display transform as curves and a matrix: exact for a profile built
/// that way. See `display_curves` in the shader.
pub struct DisplayCurves {
    /// Samples per curve.
    pub width: u32,
    /// Working-space decode: encoded value to linear light, per channel.
    pub decode: Vec<[f32; 3]>,
    /// Target encode: linear light to encoded value, per channel, indexed by
    /// the square root of the linear value.
    pub encode: Vec<[f32; 3]>,
    /// Linear working-space RGB to linear target RGB, in column-major order.
    pub to_target: [[f32; 3]; 3],
}

impl DisplayCurves {
    pub fn upload(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: TableFormat,
    ) -> wgpu::TextureView {
        let extent = wgpu::Extent3d {
            width: self.width,
            height: 2,
            depth_or_array_layers: 1,
        };
        let values = self
            .decode
            .iter()
            .chain(&self.encode)
            .flat_map(|sample| [sample[0], sample[1], sample[2], 1.0]);
        let (texture_format, bytes, texel): (_, Vec<u8>, u32) = match format {
            TableFormat::Half => (
                wgpu::TextureFormat::Rgba16Float,
                values
                    .flat_map(|value| half::f16::from_f32(value).to_bits().to_le_bytes())
                    .collect(),
                8,
            ),
            TableFormat::Full => (
                wgpu::TextureFormat::Rgba32Float,
                values.flat_map(f32::to_le_bytes).collect(),
                16,
            ),
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("display curves"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * texel),
                rows_per_image: None,
            },
            extent,
        );
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// The matrix as a std140 `mat3x3f`: three padded columns.
    pub fn matrix_uniform(&self) -> [f32; 12] {
        let m = self.to_target;
        [
            m[0][0], m[0][1], m[0][2], 0.0, m[1][0], m[1][1], m[1][2], 0.0, m[2][0], m[2][1],
            m[2][2], 0.0,
        ]
    }
}
