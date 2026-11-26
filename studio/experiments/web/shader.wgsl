// The display transform: the last light step. A 3D table maps encoded RGB in
// the document's working space to encoded RGB for the target, which is either
// the display's own profile or the color space set on the surface.

// Looks `rgb` up in a table with `size` nodes per axis. The sampler must
// filter linearly and clamp to the edge.
fn display_lookup(table: texture_3d<f32>, table_sampler: sampler, size: f32, rgb: vec3f) -> vec3f {
    // Node k sits at the center of texel k, so 0 and 1 land on the outer nodes.
    let position = (clamp(rgb, vec3f(0.0), vec3f(1.0)) * (size - 1.0) + 0.5) / size;
    return textureSampleLevel(table, table_sampler, position, 0.0).rgb;
}

// The exact form for a display profile made of three curves and a matrix,
// which most are. `curves` has two rows. Row 0 decodes the working space to
// linear light, indexed by the encoded value. Row 1 encodes linear light for
// the target, indexed by the square root of the linear value so that the
// steep part of the curve near black gets most of the samples.
fn display_curves(curves: texture_2d<f32>, curves_sampler: sampler, width: f32, to_target: mat3x3f, rgb: vec3f) -> vec3f {
    let decode_at = (clamp(rgb, vec3f(0.0), vec3f(1.0)) * (width - 1.0) + 0.5) / width;
    let linear = vec3f(
        textureSampleLevel(curves, curves_sampler, vec2f(decode_at.r, 0.25), 0.0).r,
        textureSampleLevel(curves, curves_sampler, vec2f(decode_at.g, 0.25), 0.0).g,
        textureSampleLevel(curves, curves_sampler, vec2f(decode_at.b, 0.25), 0.0).b,
    );
    let target_linear = clamp(to_target * linear, vec3f(0.0), vec3f(1.0));
    let encode_at = (sqrt(target_linear) * (width - 1.0) + 0.5) / width;
    return vec3f(
        textureSampleLevel(curves, curves_sampler, vec2f(encode_at.r, 0.75), 0.0).r,
        textureSampleLevel(curves, curves_sampler, vec2f(encode_at.g, 0.75), 0.0).g,
        textureSampleLevel(curves, curves_sampler, vec2f(encode_at.b, 0.75), 0.0).b,
    );
}
// The light operations of the spike: per-band stretch and color assignment,
// summed across eight bands, with a painted mask applied on top, then the
// display transform. `display.wgsl` is prepended to this file.

struct Band {
    // rgb: the band's color. A band that is off has a zero color.
    color: vec4f,
    // x: black point, y: 1 / (white - black), z: midtone balance.
    stretch: vec4f,
}

struct Globals {
    // clip = doc * view.xy + view.zw
    view: vec4f,
    // x, y: surface size in pixels. z: mask brighten gain. w: mask overlay.
    screen: vec4f,
    bands: array<Band, 8>,
    // Linear working-space RGB to linear target RGB.
    display_matrix: mat3x3f,
    // x: samples per display curve, or 0 when there is no display transform.
    display: vec4f,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var tile_sampler: sampler;
@group(0) @binding(2) var mask: texture_2d<f32>;
@group(0) @binding(3) var display_curves_table: texture_2d<f32>;
@group(1) @binding(0) var tile: texture_2d_array<f32>;

struct TileVertex {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn tile_vertex(
    @builtin(vertex_index) index: u32,
    @location(0) rect: vec4f,
    @location(1) uv_rect: vec4f,
) -> TileVertex {
    let corner = vec2f(f32(index & 1u), f32((index >> 1u) & 1u));
    let doc = mix(rect.xy, rect.zw, corner);
    var out: TileVertex;
    out.position = vec4f(doc * globals.view.xy + globals.view.zw, 0.0, 1.0);
    out.uv = mix(uv_rect.xy, uv_rect.zw, corner);
    return out;
}

// The midtone transfer function. A balance of 0.5 leaves x unchanged.
fn midtone(balance: f32, x: f32) -> f32 {
    return (balance - 1.0) * x / ((2.0 * balance - 1.0) * x - balance);
}

@fragment
fn tile_fragment(in: TileVertex) -> @location(0) vec4f {
    var color = vec3f(0.0);
    for (var index = 0; index < 8; index++) {
        let band = globals.bands[index];
        let sample = textureSampleLevel(tile, tile_sampler, in.uv, index, 0.0).r;
        let x = clamp((sample - band.stretch.x) * band.stretch.y, 0.0, 1.0);
        color += band.color.rgb * midtone(band.stretch.z, x);
    }
    let coverage = textureLoad(mask, vec2i(in.position.xy), 0).r;
    color *= 1.0 + globals.screen.z * coverage;
    color = mix(color, vec3f(1.0, 0.25, 0.2), globals.screen.w * coverage * 0.45);
    if globals.display.x > 0.0 {
        color = display_curves(
            display_curves_table,
            tile_sampler,
            globals.display.x,
            globals.display_matrix,
            color,
        );
    }
    return vec4f(color, 1.0);
}

struct StrokeVertex {
    @builtin(position) position: vec4f,
    @location(0) doc: vec2f,
    @location(1) @interpolate(flat) ends: vec4f,
    @location(2) @interpolate(flat) brush: vec4f,
}

// One brush segment, drawn as a quad around a capsule.
@vertex
fn stroke_vertex(
    @builtin(vertex_index) index: u32,
    @location(0) ends: vec4f,
    @location(1) brush: vec4f,
) -> StrokeVertex {
    let corner = vec2f(f32(index & 1u), f32((index >> 1u) & 1u)) * 2.0 - 1.0;
    let delta = ends.zw - ends.xy;
    let length = length(delta);
    var along = vec2f(1.0, 0.0);
    if length > 0.0001 {
        along = delta / length;
    }
    let across = vec2f(-along.y, along.x);
    let radius = brush.x;
    let doc = mix(ends.xy, ends.zw, corner.x * 0.5 + 0.5)
        + along * corner.x * radius
        + across * corner.y * radius;
    var out: StrokeVertex;
    out.position = vec4f(doc * globals.view.xy + globals.view.zw, 0.0, 1.0);
    out.doc = doc;
    out.ends = ends;
    out.brush = brush;
    return out;
}

@fragment
fn stroke_fragment(in: StrokeVertex) -> @location(0) vec4f {
    let delta = in.ends.zw - in.ends.xy;
    let along = clamp(
        dot(in.doc - in.ends.xy, delta) / max(dot(delta, delta), 0.0001),
        0.0,
        1.0,
    );
    let distance = distance(in.doc, in.ends.xy + delta * along);
    let radius = in.brush.x;
    let coverage = 1.0 - smoothstep(radius * in.brush.y, radius, distance);
    // brush.z is 1 to add to the mask and 0 to subtract from it.
    return vec4f(in.brush.z, 0.0, 0.0, coverage);
}
