// Presents the finished picture: Seiza's compositor leaves it in the
// working space, and this pass applies the display transform on the way to
// the surface.

struct Present {
    // Linear working-space RGB to linear target RGB.
    to_target: mat3x3f,
    // x: samples per display curve, or 0 when there is no display transform.
    display: vec4f,
}

@group(0) @binding(0) var<uniform> present: Present;
@group(0) @binding(1) var linear_sampler: sampler;
@group(0) @binding(2) var finished: texture_2d<f32>;
@group(0) @binding(3) var display_curves_table: texture_2d<f32>;

@vertex
fn present_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let corner = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4f(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn present_fragment(@builtin(position) position: vec4f) -> @location(0) vec4f {
    var color = textureLoad(finished, vec2i(position.xy), 0).rgb;
    if present.display.x > 0.0 {
        color = display_curves(
            display_curves_table,
            linear_sampler,
            present.display.x,
            present.to_target,
            color,
        );
    }
    return vec4f(color, 1.0);
}
