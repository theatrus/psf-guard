// The Combine stage: each layer stretches one band, colors it and blends it
// onto the layers beneath. Also the brush strokes that become masks.

@group(1) @binding(0) var tile: texture_2d_array<f32>;
// The masks, one per layer of the array, in surface pixels.
@group(2) @binding(0) var layer_masks: texture_2d_array<f32>;
// The samples of each layer that has heals of its own, in surface pixels.
@group(2) @binding(1) var healed_layers: texture_2d_array<f32>;
// The computed masks that are smoothed, in surface pixels.
@group(2) @binding(2) var smoothed_masks: texture_2d_array<f32>;

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

// A layer's stretch of a band's sample. The stretch itself is resolved by
// the host and arrives as a table; nothing here knows its form.
fn stretched(sample: f32, row: f32) -> f32 {
    let size = vec2f(textureDimensions(stretch_table));
    let at = vec2f(
        (sqrt(clamp(sample, 0.0, 1.0)) * (size.x - 1.0) + 0.5) / size.x,
        (row + 0.5) / size.y,
    );
    return textureSampleLevel(stretch_table, linear_sampler, at, 0.0).r;
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
    } else if mode < 5.5 {
        return min(below, layer);
    }
    // Luminance: the layer's brightness with the color beneath it.
    let weights = vec3f(0.2126, 0.7152, 0.0722);
    return max(below + dot(layer, weights) - dot(below, weights), vec3f(0.0));
}

// What an adjustment layer makes of the color beneath it.
fn adjusted(color: vec3f, adjust: vec4f) -> vec3f {
    if adjust.x < 1.5 {
        return color * exp2(adjust.y);
    } else if adjust.x < 2.5 {
        return vec3f(
            contrast_curve(color.r, adjust.y),
            contrast_curve(color.g, adjust.y),
            contrast_curve(color.b, adjust.y),
        );
    }
    let luminance = dot(color, vec3f(0.2126, 0.7152, 0.0722));
    return max(mix(vec3f(luminance), color, adjust.y), vec3f(0.0));
}

// A band's sample in a tile, blended with its second band.
fn band_sample(uv: vec2f, band: f32, second: f32, amount: f32) -> f32 {
    var sample = textureSampleLevel(tile, linear_sampler, uv, i32(band), 0.0).r;
    if second >= 0.0 {
        let other = textureSampleLevel(tile, linear_sampler, uv, i32(second), 0.0).r;
        sample = mix(sample, other, amount);
    }
    return sample;
}

// A computed mask as its bands give it at a place in a tile: the product of
// their stretched samples, shaped and raised to the mask's reach.
fn computed_shape(index: i32, uv: vec2f) -> f32 {
    let mask = globals.computed[index];
    var product = 1.0;
    if mask.first.x >= 0.0 {
        product *= stretched(band_sample(uv, mask.first.x, mask.first.y, mask.first.z), mask.first.w);
    }
    if mask.second.x >= 0.0 {
        product *= stretched(band_sample(uv, mask.second.x, mask.second.y, mask.second.z), mask.second.w);
    }
    product = clamp(product, 0.0, 1.0);
    if mask.shape.x > 0.5 {
        product = pow(product, 1.0 - product);
    }
    return pow(product, mask.shape.z);
}

// A computed mask as a layer uses it: smoothed when it asks for that, so
// the bands' noise does not become speckle, then touched up by strokes that
// set it to 1 or to 0, then inverted or not.
fn computed_mask(index: i32, uv: vec2f, pixel: vec2i) -> f32 {
    let mask = globals.computed[index];
    var value = 0.0;
    if mask.touch.z > 0.0 {
        value = textureLoad(smoothed_masks, pixel, index, 0).r;
    } else {
        value = computed_shape(index, uv);
    }
    if mask.touch.x >= 0.0 {
        value = max(value, textureLoad(layer_masks, pixel, i32(mask.touch.x), 0).r);
    }
    if mask.touch.y >= 0.0 {
        value *= 1.0 - textureLoad(layer_masks, pixel, i32(mask.touch.y), 0).r;
    }
    if mask.shape.y > 0.5 {
        value = 1.0 - value;
    }
    return value;
}

struct EveryMask {
    @location(0) first: vec4f,
    @location(1) second: vec4f,
    @location(2) third: vec4f,
    @location(3) fourth: vec4f,
    @location(4) fifth: vec4f,
    @location(5) sixth: vec4f,
    @location(6) seventh: vec4f,
    @location(7) eighth: vec4f,
}

fn every_mask(values: array<f32, 8>) -> EveryMask {
    var out: EveryMask;
    out.first = vec4f(values[0]);
    out.second = vec4f(values[1]);
    out.third = vec4f(values[2]);
    out.fourth = vec4f(values[3]);
    out.fifth = vec4f(values[4]);
    out.sixth = vec4f(values[5]);
    out.seventh = vec4f(values[6]);
    out.eighth = vec4f(values[7]);
    return out;
}

// The computed masks that are smoothed, one per target, before the blurs.
@fragment
fn computed_fragment(in: TileVertex) -> EveryMask {
    var values = array<f32, 8>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for (var index = 0; index < 8; index++) {
        if globals.computed[index].touch.z > 0.0 {
            values[index] = computed_shape(index, in.uv);
        }
    }
    return every_mask(values);
}

// One triangle over the whole target.
@vertex
fn whole_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let corner = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4f(corner * 2.0 - 1.0, 0.0, 1.0);
}

// One direction of a Gaussian blur of each smoothed mask. A blur reads the
// masks through the tile binding. Thirty-three samples cover three standard
// deviations to each side; a wide blur spaces them further apart.
fn blurred(position: vec2f, direction: vec2f) -> EveryMask {
    let size = vec2f(textureDimensions(tile).xy);
    var values = array<f32, 8>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for (var index = 0; index < 8; index++) {
        let sigma = globals.computed[index].touch.z;
        if sigma <= 0.0 {
            continue;
        }
        let spacing = sigma * 3.0 / 16.0;
        var sum = 0.0;
        var weights = 0.0;
        for (var tap = -16; tap <= 16; tap++) {
            let offset = f32(tap) * spacing;
            let weight = exp(-offset * offset / (2.0 * sigma * sigma));
            let at = (position + direction * offset) / size;
            sum += weight * textureSampleLevel(tile, linear_sampler, at, index, 0.0).r;
            weights += weight;
        }
        values[index] = sum / weights;
    }
    return every_mask(values);
}

@fragment
fn blur_across_fragment(@builtin(position) position: vec4f) -> EveryMask {
    return blurred(position.xy, vec2f(1.0, 0.0));
}

@fragment
fn blur_down_fragment(@builtin(position) position: vec4f) -> EveryMask {
    return blurred(position.xy, vec2f(0.0, 1.0));
}

struct HealedLayers {
    @location(0) first: vec4f,
    @location(1) second: vec4f,
    @location(2) third: vec4f,
    @location(3) fourth: vec4f,
}

// The samples of each layer that has heals of its own, one per target.
@fragment
fn healed_fragment(in: TileVertex) -> HealedLayers {
    var samples = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    for (var place = 0; place < 4; place++) {
        let healed = globals.healed[place];
        if healed.x >= 0.0 {
            samples[place] = band_sample(in.uv, healed.x, healed.y, healed.z);
        }
    }
    var out: HealedLayers;
    out.first = vec4f(samples[0]);
    out.second = vec4f(samples[1]);
    out.third = vec4f(samples[2]);
    out.fourth = vec4f(samples[3]);
    return out;
}

// A healed layer's sample at a surface pixel, with the heals of its band
// applied. A heal copies the sample as it was before any heal.
fn healed_sample(index: i32, place: i32, pixel: vec2i) -> f32 {
    let size = vec2i(globals.screen.xy);
    var sample = textureLoad(healed_layers, pixel, place, 0).r;
    let count = i32(globals.counts.z);
    for (var at = 0; at < count; at++) {
        let heal = globals.heals[at];
        if i32(heal.what.x) != index || heal.what.y > 0.5 {
            continue;
        }
        let source = pixel + vec2i(heal.offset.xy);
        if all(source >= vec2i(0)) && all(source < size) {
            let copy = textureLoad(healed_layers, source, place, 0).r + heal.what.w;
            sample = mix(sample, copy, textureLoad(layer_masks, pixel, i32(heal.what.z), 0).r);
        }
    }
    return sample;
}

// A healed layer's stretched value at a surface pixel, with the heals of
// the layer applied. A heal copies the value as it was before those heals.
fn healed_value(index: i32, pixel: vec2i) -> f32 {
    let layer = globals.layers[index];
    let place = i32(layer.stretch.y);
    let size = vec2i(globals.screen.xy);
    var value = contrast_curve(
        stretched(healed_sample(index, place, pixel), layer.stretch.x),
        layer.stretch.w,
    );
    let count = i32(globals.counts.z);
    for (var at = 0; at < count; at++) {
        let heal = globals.heals[at];
        if i32(heal.what.x) != index || heal.what.y < 0.5 {
            continue;
        }
        let source = pixel + vec2i(heal.offset.xy);
        if all(source >= vec2i(0)) && all(source < size) {
            let copy = contrast_curve(
                stretched(healed_sample(index, place, source), layer.stretch.x),
                layer.stretch.w,
            ) + heal.what.w;
            value = mix(value, copy, textureLoad(layer_masks, pixel, i32(heal.what.z), 0).r);
        }
    }
    return value;
}

@fragment
fn tile_fragment(in: TileVertex) -> @location(0) vec4f {
    // The layers blended so far.
    var color = vec3f(0.0);
    // The layers beneath a group, kept while the group's members blend on
    // their own.
    var beneath = vec3f(0.0);
    // The last band layer or group, which waits to be blended until its own
    // adjustments have been applied to it.
    var waiting = false;
    var own = vec3f(0.0);
    var own_cover = 0.0;
    var own_mode = 0.0;
    let pixel = vec2i(in.position.xy);
    let count = i32(globals.counts.x);
    for (var index = 0; index < count; index++) {
        let layer = globals.layers[index];
        // How much of the layer applies here: its opacity, within its mask
        // and within the mask of the layer it is clipped to.
        var cover = layer.color.a;
        if layer.adjust.z >= 0.0 {
            cover *= textureLoad(layer_masks, pixel, i32(layer.adjust.z), 0).r;
        }
        if layer.stretch.z >= 0.0 {
            cover *= textureLoad(layer_masks, pixel, i32(layer.stretch.z), 0).r;
        }
        if layer.more.x >= 0.0 {
            cover *= computed_mask(i32(layer.more.x), in.uv, pixel);
        }
        let kind = layer.adjust.x;
        if kind > 0.5 && kind < 9.5 && layer.adjust.w > 0.5 {
            // An adjustment of the waiting layer or group alone.
            if waiting {
                own = mix(own, adjusted(own, layer.adjust), cover);
            }
            continue;
        }
        if waiting {
            color = mix(color, blend(own_mode, color, own), own_cover);
            waiting = false;
        }
        if kind > 11.5 {
            // A layer of its color everywhere.
            own = layer.color.rgb;
            own_cover = cover;
            own_mode = layer.select.y;
            waiting = true;
            continue;
        }
        if kind > 10.5 {
            // The end of a group: its members' blend is one layer now.
            own = color;
            own_cover = cover;
            own_mode = layer.select.y;
            waiting = true;
            color = beneath;
            continue;
        }
        if kind > 9.5 {
            // The start of a group: its members blend from nothing.
            beneath = color;
            color = vec3f(0.0);
            continue;
        }
        if kind > 0.5 {
            color = mix(color, adjusted(color, layer.adjust), cover);
            continue;
        }
        var value = 0.0;
        if layer.stretch.y >= 0.0 {
            value = healed_value(index, pixel);
        } else {
            let sample = band_sample(in.uv, layer.select.x, layer.select.z, layer.select.w);
            value = contrast_curve(stretched(sample, layer.stretch.x), layer.stretch.w);
        }
        own = layer.color.rgb * value;
        own_cover = cover;
        own_mode = layer.select.y;
        waiting = true;
    }
    if waiting {
        color = mix(color, blend(own_mode, color, own), own_cover);
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
