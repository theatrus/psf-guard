// The display transform: the last light step. A 3D table maps encoded RGB in
// the document's working space to encoded RGB for the target, which is either
// the display's own profile or the color space set on the surface.

// Looks `rgb` up in a table with `size` nodes per axis. The sampler must
// filter linearly and clamp to the edge.
fn display_lookup(table: texture_3d<f32>, table_sampler: sampler, size: f32, rgb: vec3f) -> vec3f {
    // Node k sits at the center of texel k, so 0 and 1 land on the outer nodes.
    let position = (clamp(rgb, vec3f(0.0), vec3f(1.0)) * (size - 1.0) + 0.5) / size;
    return textureSampleLevel(table, table_sampler, position, 0.0).rgb;
}

// The exact form for a display profile made of three curves and a matrix,
// which most are. `curves` has two rows. Row 0 decodes the working space to
// linear light, indexed by the encoded value. Row 1 encodes linear light for
// the target, indexed by the square root of the linear value so that the
// steep part of the curve near black gets most of the samples.
fn display_curves(curves: texture_2d<f32>, curves_sampler: sampler, width: f32, to_target: mat3x3f, rgb: vec3f) -> vec3f {
    let decode_at = (clamp(rgb, vec3f(0.0), vec3f(1.0)) * (width - 1.0) + 0.5) / width;
    let linear = vec3f(
        textureSampleLevel(curves, curves_sampler, vec2f(decode_at.r, 0.25), 0.0).r,
        textureSampleLevel(curves, curves_sampler, vec2f(decode_at.g, 0.25), 0.0).g,
        textureSampleLevel(curves, curves_sampler, vec2f(decode_at.b, 0.25), 0.0).b,
    );
    let target_linear = clamp(to_target * linear, vec3f(0.0), vec3f(1.0));
    let encode_at = (sqrt(target_linear) * (width - 1.0) + 0.5) / width;
    return vec3f(
        textureSampleLevel(curves, curves_sampler, vec2f(encode_at.r, 0.75), 0.0).r,
        textureSampleLevel(curves, curves_sampler, vec2f(encode_at.g, 0.75), 0.0).g,
        textureSampleLevel(curves, curves_sampler, vec2f(encode_at.b, 0.75), 0.0).b,
    );
}
