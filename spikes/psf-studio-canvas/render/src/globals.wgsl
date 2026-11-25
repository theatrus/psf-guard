// What every pass of the canvas shares.

// One layer of the Combine stage.
struct Layer {
    // rgb: the layer's color. a: its opacity, 0 when the layer is hidden.
    color: vec4f,
    // x: the row of the stretch table that holds this layer's stretch.
    // y: the layer's place among the healed layers, or -1 when it has no
    // heals of its own. z: the mask layer of the layer it is clipped to, or
    // -1. w: the layer's own curve amount.
    stretch: vec4f,
    // x: the band the layer shows, y: the blend mode. z: a second band
    // blended into the first before the stretch, or -1. w: how much of it.
    select: vec4f,
    // x: 0 for a layer that shows a band; 1 exposure, 2 curves, 3 saturation
    // for an adjustment of the layers beneath; 10 for the start of a group
    // and 11 for its end; 12 for a layer of its color everywhere. y: the
    // adjustment's amount.
    // z: the mask layer that limits the layer, or -1 for everywhere.
    // w: 1 for an adjustment of the band layer before it alone.
    adjust: vec4f,
    // x: the computed mask that limits the layer, or -1 for none.
    more: vec4f,
}

// One step of the Finish stage.
struct FinishStep {
    // x: 1 curves, 2 saturation, 3 heal. y: amount. z: the mask layer that
    // limits the step, or -1 for everywhere. w: the step's opacity.
    kind: vec4f,
    // xy: for a heal, where the replacement comes from, in surface pixels.
    heal: vec4f,
    // rgb: for a heal, the brightness added to the copied color.
    shift: vec4f,
}

// A heal of one layer.
struct LayerHeal {
    // x: the layer healed. y: 0 for a heal of the band's samples before the
    // stretch, 1 for a heal of the stretched layer. z: the mask layer that
    // holds the heal's stroke. w: the brightness added to the copy.
    what: vec4f,
    // xy: where the copy comes from, in surface pixels.
    offset: vec4f,
}

// A mask computed at each pixel from one or two bands.
struct ComputedMask {
    // x: the band, or -1 for none. y: a second band blended into it before
    // the stretch, or -1. z: how much of the second. w: the row of the
    // stretch table that holds the band's stretch.
    first: vec4f,
    second: vec4f,
    // x: 1 to raise the product to the power of 1 minus itself. y: 1 for
    // 1 minus the result. z: the power the shaped product is raised to.
    shape: vec4f,
    // x: the mask layer of strokes that set the mask to 1, or -1. y: the
    // mask layer of strokes that set it to 0, or -1. z: how far the mask is
    // smoothed before the strokes, as the standard deviation of a blur in
    // surface pixels; 0 is not at all.
    touch: vec4f,
}

struct Globals {
    // clip = doc * view.xy + view.zw
    view: vec4f,
    // x, y: surface size in pixels. z: strength of the stroke overlay.
    // w: the mask layer that holds the overlay.
    screen: vec4f,
    // x: number of layers. y: number of finish steps. z: number of heals
    // of single layers.
    counts: vec4f,
    layers: array<Layer, 24>,
    finish: array<FinishStep, 8>,
    // Per healed layer: x: its band, y: a second band blended into the
    // first, or -1, z: how much of the second. A place not in use has -1.
    healed: array<vec4f, 4>,
    heals: array<LayerHeal, 16>,
    computed: array<ComputedMask, 8>,
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
