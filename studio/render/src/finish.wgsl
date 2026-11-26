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

// One row of the stretch table at a value from 0 to 1. A row is indexed by
// the square root of the value, as a layer's row is.
fn table_value(value: f32, row: f32) -> f32 {
    let size = vec2f(textureDimensions(stretch_table));
    let at = vec2f(
        (sqrt(clamp(value, 0.0, 1.0)) * (size.x - 1.0) + 0.5) / size.x,
        (row + 0.5) / size.y,
    );
    return textureSampleLevel(stretch_table, linear_sampler, at, 0.0).r;
}

// A stretch of the picture. Its three rows map red, green and blue. With
// luminance, the first row maps the Rec. 709 luminance and the pixel is
// scaled by one factor, kept in gamut, as Seiza's luminance-preserving
// strategy applies a curve.
fn stretch_picture(color: vec3f, step: FinishStep) -> vec3f {
    let row = step.heal.z;
    if step.heal.w < 0.5 {
        return vec3f(
            table_value(color.r, row),
            table_value(color.g, row + 1.0),
            table_value(color.b, row + 2.0),
        );
    }
    let luminance = dot(color, vec3f(0.2126, 0.7152, 0.0722));
    let mapped = table_value(luminance, row);
    if luminance <= 1.0e-8 {
        return vec3f(mapped);
    }
    var scale = mapped / luminance;
    let maximum = max(max(color.r, color.g), max(color.b, 0.0));
    if maximum * scale > 1.0 {
        scale = 1.0 / maximum;
    }
    return clamp(color * scale, vec3f(0.0), vec3f(1.0));
}

// Whether a step maps each pixel alone: everything but a heal.
fn maps_pixels(step: FinishStep) -> bool {
    return step.kind.x < 2.5 || step.kind.x > 3.5;
}

// Vibrance and saturation, each from -1 to 1. Vibrance changes a muted
// color more than a vivid one. The CPU render's `colorfulness` is the same.
fn colorfulness(color: vec3f, vibrance: f32, saturation: f32) -> vec3f {
    let luminance = dot(color, vec3f(0.2126, 0.7152, 0.0722));
    let high = max(max(color.r, color.g), color.b);
    let low = min(min(color.r, color.g), color.b);
    var vivid = 0.0;
    if high > 1.0e-6 {
        vivid = clamp((high - low) / high, 0.0, 1.0);
    }
    let gain = (1.0 + saturation) * (1.0 + vibrance * (1.0 - vivid));
    return max(vec3f(luminance) + (color - vec3f(luminance)) * gain, vec3f(0.0));
}

// A step that maps each pixel alone.
fn adjust(color: vec3f, step: FinishStep) -> vec3f {
    if step.kind.x > 4.5 {
        // Tone and color: the luminance curve, then the colors.
        return colorfulness(stretch_picture(color, step), step.shift.x, step.shift.y);
    }
    if step.kind.x > 3.5 {
        return stretch_picture(color, step);
    }
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
        if maps_pixels(step) {
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
        if maps_pixels(step) {
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
