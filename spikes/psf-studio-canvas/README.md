# PSF-Studio canvas spike

This holds investigations 1, 3 and 9 of the
[PSF-Studio design record](../../docs/design/psf-studio.md). Investigation 1:
can a GPUI window host a tiled wgpu canvas that meets the responsiveness
targets, and how does it compare with WebGPU in a browser? Investigation 3:
how fast is a heavy step previewed on a full-resolution crop? Investigation 9:
can the canvas present color-managed output? The results are
in the design record under “Investigation results”.

The spike is its own Cargo workspace. It is not part of the PSF Guard build,
and it needs Rust 1.100 for GPUI.

| Path | Contents |
|---|---|
| `render/` | The canvas: a synthetic 600-megapixel, eight-band document, a GPU tile cache, and three passes: masks from strokes, layers blended into one image, then the finish steps and the display transform. It depends on wgpu only. |
| `app/` | A GPUI window that hosts the canvas in a native view, with band sliders and a brush. macOS only. |
| `web/` | The canvas as it was when the web comparison was measured, in WebGPU, with its own copy of that shader, and the page for the crop preview. |
| `color/` | The display transform built from ICC profiles with Little CMS, and a check of the shader against Little CMS and ColorSync. macOS only. |
| `crop/` | Seiza deconvolution previewed on a crop: timings in process, and a server that answers the same previews over HTTP. |
| `results/` | The measurements quoted in the design record. |

## The staged panes

The app shows the staged workflow from the design record: a rail of six
stages, the showing stage's steps or layers on the left, the canvas, and the
stage's own controls on the right. The panes are written against the
document model in PSF Guard's `src/studio/`. The app compiles those source
files directly, so it needs no build of PSF Guard.

The canvas draws from the document:

- **Stretch:** each band's midtone.
- **Combine:** the layers in list order, bottom first, with each layer's
  color, opacity, visibility, blend mode and its own Curves.
- **Finish:** Curves, Saturation and Heal steps in order, each limited by its
  own painted mask.

Each step's line says whether it is drawn or only recorded. Linear-stage
steps are recorded only: they are heavy steps for the tile path. A heal
copies from three brush radii to the right and does not yet match the
brightness of its surroundings, because the canvas cannot read the image to
measure it. The external editor button is disabled.

## Run the native canvas

```bash
cargo run --release -p studio-canvas-spike
```

Drag to pan. Scroll with Command held to zoom. Choose **Brush** and drag to
paint a mask. Drag a band's slider to change its midtone; click a band's name
to turn the band off.

## Run the measurements

Keep the window visible and in front while a measurement runs. A covered
window gets no frames.

```bash
cargo run --release -p studio-canvas-spike -- --bench results/native.json
```

For the web baseline, start the server, then open the page in the browser to
measure. The page posts its results to `web/results/<name>.json`.

```bash
python3 web/serve.py
```

```bash
open -a Safari "http://localhost:8765/web/index.html?frames=600&name=safari"
```

Print a result file as a table:

```bash
python3 results/summarize.py results/native-render-thread.json results/web-chrome.json
```

## Run the crop preview measurements

```bash
cargo run --release -p studio-crop-preview-spike -- bench results/crop.json
```

For the browser side, start the server, then open the page. The page posts
its results to `web/results/<name>.json`.

```bash
cargo run --release -p studio-crop-preview-spike -- serve
```

```bash
open -a Safari "http://localhost:8766/web/crop.html?name=crop-safari"
```

## Run the color checks

Write the displays' ICC profiles to the current folder as `display-0.icc`,
`display-1.icc` and so on:

```bash
swift color/display-profiles.swift
```

Compare the shader's display transform with Little CMS and ColorSync:

```bash
cargo run --release -p studio-color-spike -- results/color.json display-0.icc
```

Run the canvas on a wide-gamut surface. `--output` is `srgb8`, `p3-8`,
`p3-float` or `p3-extended`. `--working` is the ICC profile of the document's
working space; the canvas converts from it to the surface's color space.

```bash
cargo run --release -p studio-canvas-spike -- --output p3-extended --working "/System/Library/ColorSync/Profiles/AdobeRGB1998.icc"
```

The panel's **Layer** line shows the pixel format and color space that the
canvas's Metal layer ended up with.

`--capture <prefix>` writes PNG captures of selected frames during a native
measurement.
