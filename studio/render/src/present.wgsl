// Presents the finished picture: Seiza's compositor leaves it in the
// working space, and this pass applies the display transform on the way to
// the surface.

struct Present {
    // Linear working-space RGB to linear target RGB.
    to_target: mat3x3f,
    // x: samples per display curve, or 0 when there is no display transform.
    // y: 1 when the surface takes values above SDR white (EDR).
    // z: how many of `panes` are in use. w: the peak outside them.
    display: vec4f,
    // Each pane's scissor: x, y, width and height in surface pixels.
    panes: array<vec4f, 8>,
    // Each pane's peak, four to an entry.
    peaks: array<vec4f, 2>,
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
    // Nothing goes past the pane's peak: SDR white unless HDR Tone lifts it.
    let peak = peak_at(position.xy);
    var color = min(textureLoad(finished, vec2i(position.xy), 0).rgb, vec3f(peak));
    // On an EDR surface, an HDR picture's values above SDR white go on
    // past 1, on the extended sRGB curve the surface reads too. With no
    // display transform the working space is the surface's.
    if present.display.y > 0.5 && any(color > vec3f(1.0)) {
        if present.display.x > 0.0 {
            return vec4f(extended(color), 1.0);
        }
        return vec4f(color, 1.0);
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

// The peak of the pane at `position`.
fn peak_at(position: vec2f) -> f32 {
    let count = i32(present.display.z);
    for (var index = 0; index < count; index++) {
        let rect = present.panes[index];
        if all(position >= rect.xy) && all(position < rect.xy + rect.zw) {
            return present.peaks[index / 4][index % 4];
        }
    }
    return present.display.w;
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
// curves within 0 to 1; beyond it the extended sRGB curve, which Tone's HDR
// values continue on, decodes it and encodes it for the surface.
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
