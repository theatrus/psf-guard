// The Combine stage: each layer stretches one band, colors it and blends it
// onto the layers beneath. Also the brush strokes that become masks.

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

// How a layer's color combines with what is beneath it, before opacity.
// The modes are in the order of the document's `BlendMode`.
fn blend(mode: f32, below: vec3f, layer: vec3f) -> vec3f {
    if mode < 0.5 {
        return layer;
    } else if mode < 1.5 {
        return 1.0 - (1.0 - clamp(below, vec3f(0.0), vec3f(1.0))) * (1.0 - clamp(layer, vec3f(0.0), vec3f(1.0)));
    } else if mode < 2.5 {
        return below + layer;
    } else if mode < 3.5 {
        return below * layer;
    } else if mode < 4.5 {
        return max(below, layer);
    }
    return min(below, layer);
}

@fragment
fn tile_fragment(in: TileVertex) -> @location(0) vec4f {
    var color = vec3f(0.0);
    let count = i32(globals.counts.x);
    for (var index = 0; index < count; index++) {
        let layer = globals.layers[index];
        let sample = textureSampleLevel(tile, linear_sampler, in.uv, i32(layer.select.x), 0.0).r;
        let x = clamp((sample - layer.stretch.x) * layer.stretch.y, 0.0, 1.0);
        let value = contrast_curve(midtone(layer.stretch.z, x), layer.stretch.w);
        let blended = blend(layer.select.y, color, layer.color.rgb * value);
        color = mix(color, blended, layer.color.a);
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
