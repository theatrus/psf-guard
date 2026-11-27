// Presents the finished picture: Seiza's compositor leaves it in the
// working space, and this pass applies the display transform on the way to
// the surface.

struct Present {
    // Linear working-space RGB to linear target RGB.
    to_target: mat3x3f,
    // x: samples per display curve, or 0 when there is no display transform.
    // y: 1 when the surface takes values above SDR white (EDR).
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
    // On an EDR surface, an HDR picture's values above SDR white go on
    // past 1, on the extended sRGB curve the surface reads.
    if present.display.x > 0.0 && present.display.y > 0.5 && any(color > vec3f(1.0)) {
        return vec4f(extended(color), 1.0);
    }
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

// One display curve's value: row 0 decodes, indexed by the encoded value;
// row 1 encodes, indexed by the square root of the linear value.
fn curve_at(row: f32, at: f32, channel: i32) -> f32 {
    let width = present.display.x;
    let place = (clamp(at, 0.0, 1.0) * (width - 1.0) + 0.5) / width;
    let value = textureSampleLevel(
        display_curves_table,
        linear_sampler,
        vec2f(place, row * 0.5 + 0.25),
        0.0,
    );
    return value[channel];
}

// The display transform for a color with a channel above SDR white: the
// curves within 0 to 1, and the extended sRGB curve beyond it, both ways.
fn extended(color: vec3f) -> vec3f {
    var linear = vec3f(0.0);
    for (var channel = 0; channel < 3; channel++) {
        let value = max(color[channel], 0.0);
        if value > 1.0 {
            linear[channel] = pow((value + 0.055) / 1.055, 2.4);
        } else {
            linear[channel] = curve_at(0.0, value, channel);
        }
    }
    let target_linear = max(present.to_target * linear, vec3f(0.0));
    var out = vec3f(0.0);
    for (var channel = 0; channel < 3; channel++) {
        let value = target_linear[channel];
        if value > 1.0 {
            out[channel] = 1.055 * pow(value, 1.0 / 2.4) - 0.055;
        } else {
            out[channel] = curve_at(1.0, sqrt(value), channel);
        }
    }
    return out;
}
