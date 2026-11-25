// What every pass of the canvas shares.

// One layer of the Combine stage.
struct Layer {
    // rgb: the layer's color. a: its opacity, 0 when the layer is hidden.
    color: vec4f,
    // x: the row of the stretch table that holds this layer's stretch.
    // w: the layer's own curve amount.
    stretch: vec4f,
    // x: the band the layer shows, y: the blend mode. z: a second band
    // blended into the first before the stretch, or -1. w: how much of it.
    select: vec4f,
    // x: 0 for a layer that shows a band; 1 exposure, 2 curves, 3 saturation
    // for an adjustment of the layers beneath. y: the adjustment's amount.
    // z: the mask layer that limits the layer, or -1 for everywhere.
    // w: 1 for an adjustment of the band layer before it alone.
    adjust: vec4f,
}

// One step of the Finish stage.
struct FinishStep {
    // x: 1 curves, 2 saturation, 3 heal. y: amount. z: the mask layer that
    // limits the step, or -1 for everywhere.
    kind: vec4f,
    // xy: for a heal, where the replacement comes from, in surface pixels.
    heal: vec4f,
    // rgb: for a heal, the brightness added to the copied color.
    shift: vec4f,
}

struct Globals {
    // clip = doc * view.xy + view.zw
    view: vec4f,
    // x, y: surface size in pixels. z: strength of the stroke overlay.
    // w: the mask layer that holds the overlay.
    screen: vec4f,
    // x: number of layers. y: number of finish steps.
    counts: vec4f,
    layers: array<Layer, 12>,
    finish: array<FinishStep, 8>,
    // Linear working-space RGB to linear target RGB.
    display_matrix: mat3x3f,
    // x: samples per display curve, or 0 when there is no display transform.
    display: vec4f,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var linear_sampler: sampler;
// Each layer's stretch, sampled: one row per layer. A row is indexed by the
// square root of the band's sample, so the dark end, where a stretch is
// steepest, gets most of the entries.
@group(0) @binding(2) var stretch_table: texture_2d<f32>;

// An S-shaped contrast curve for a positive amount, and a flattening one for
// a negative amount. 0 leaves x unchanged.
fn contrast_curve(x: f32, amount: f32) -> f32 {
    let clamped = clamp(x, 0.0, 1.0);
    if amount >= 0.0 {
        return mix(clamped, clamped * clamped * (3.0 - 2.0 * clamped), min(amount, 1.0));
    }
    return mix(clamped, 0.5, min(-amount, 1.0) * 0.5);
}
