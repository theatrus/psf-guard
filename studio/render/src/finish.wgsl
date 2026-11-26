// The Finish stage: steps applied to the combined image in order, each
// limited by its own mask, then the stroke overlay and the display transform.

@group(1) @binding(0) var combined: texture_2d<f32>;
@group(1) @binding(1) var masks: texture_2d_array<f32>;
@group(1) @binding(2) var display_curves_table: texture_2d<f32>;

@vertex
fn finish_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let corner = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4f(corner * 2.0 - 1.0, 0.0, 1.0);
}

// How much of a step applies at a pixel.
fn coverage(pixel: vec2i, mask_layer: f32) -> f32 {
    if mask_layer < 0.0 {
        return 1.0;
    }
    return textureLoad(masks, pixel, i32(mask_layer), 0).r;
}

// How much of a step applies at a pixel: its opacity, within its mask.
fn applies(pixel: vec2i, step: FinishStep) -> f32 {
    return coverage(pixel, step.kind.z) * step.kind.w;
}

// A step that maps each pixel alone.
fn adjust(color: vec3f, step: FinishStep) -> vec3f {
    if step.kind.x < 1.5 {
        return vec3f(
            contrast_curve(color.r, step.kind.y),
            contrast_curve(color.g, step.kind.y),
            contrast_curve(color.b, step.kind.y),
        );
    }
    let luminance = dot(color, vec3f(0.2126, 0.7152, 0.0722));
    return max(mix(vec3f(luminance), color, step.kind.y), vec3f(0.0));
}

// The combined color at a pixel after the steps before `upto`, leaving heals
// out. A heal copies this from its source pixel.
fn before(pixel: vec2i, upto: i32) -> vec3f {
    var color = textureLoad(combined, pixel, 0).rgb;
    for (var index = 0; index < upto; index++) {
        let step = globals.finish[index];
        if step.kind.x < 2.5 {
            color = mix(color, adjust(color, step), applies(pixel, step));
        }
    }
    return color;
}

@fragment
fn finish_fragment(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let pixel = vec2i(position.xy);
    let size = vec2i(globals.screen.xy);
    var color = textureLoad(combined, pixel, 0).rgb;
    let count = i32(globals.counts.y);
    for (var index = 0; index < count; index++) {
        let step = globals.finish[index];
        let covered = applies(pixel, step);
        if step.kind.x < 2.5 {
            color = mix(color, adjust(color, step), covered);
        } else {
            // A heal. Its source must be on the surface to be copied.
            let source = pixel + vec2i(step.heal.xy);
            if all(source >= vec2i(0)) && all(source < size) {
                color = mix(color, before(source, index) + step.shift.rgb, covered);
            }
        }
    }
    let overlay = textureLoad(masks, pixel, i32(globals.screen.w), 0).r * globals.screen.z;
    color = mix(color, vec3f(1.0, 0.25, 0.2), overlay * 0.45);
    if globals.display.x > 0.0 {
        color = display_curves(
            display_curves_table,
            linear_sampler,
            globals.display.x,
            globals.display_matrix,
            color,
        );
    }
    return vec4f(color, 1.0);
}
