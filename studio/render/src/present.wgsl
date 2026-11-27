// Presents the finished picture: Seiza's compositor leaves it in the
// working space, and this pass maps it into the surface's primaries with
// Seiza's display mapping, `to_display`.

struct Present {
    // The working space to the surface's primaries.
    to_display: DisplayMapping,
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

@vertex
fn present_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let corner = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4f(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn present_fragment(@builtin(position) position: vec4f) -> @location(0) vec4f {
    // Nothing goes past the pane's peak: SDR white unless HDR Tone lifts it.
    let peak = peak_at(position.xy);
    let color = min(textureLoad(finished, vec2i(position.xy), 0).rgb, vec3f(peak));
    var shown = to_display(color, present.to_display);
    // Only an EDR surface shows values above SDR white; the extended sRGB
    // curve the picture is on is the one it reads.
    if present.display.y < 0.5 {
        shown = min(shown, vec3f(1.0));
    }
    return vec4f(shown, 1.0);
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
