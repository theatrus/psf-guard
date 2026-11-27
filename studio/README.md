# PSF-Studio

PSF-Studio is the native editor that starts from PSF Guard's channel stacks
and does the color processing as a document of layers. Its program is
`psf-studio`. The [design record](../docs/design/psf-studio.md) says what is
built and what is not.

PSF-Studio is its own Cargo workspace. It is not part of the PSF Guard
build: GPUI is a git dependency on Zed and needs Rust 1.100. The document
model and the tile store are compiled from PSF Guard's source, under
`src/studio` and `src/image_steps`, so both programs read and write the same
documents.

## Build and open a picture

```bash
cargo build --release -p psf-studio
```

On macOS, build the application, with `psf-guard` inside it:

```bash
./bundle-macos.sh ../target/release/psf-guard
```

The script prints where it wrote `PSF Studio.app`. The application answers
`psf-studio://` links, which is how **Open in PSF Studio** on PSF Guard's
color stacks reaches it. Move it to `/Applications` so macOS registers the
link; an application in a temporary folder is not registered.

The application's icon comes from `icons/psf-studio.svg`, PSF Guard's mark
redrawn for an editor: the same frame and star over three channels of
color. Regenerate the icons after changing it:

```bash
./generate-icons.sh
```

Open a picture in any of three ways:

- In PSF Guard, open a project's stacks and click **Open *target* in PSF
  Studio** over the color stacks.
- In PSF-Studio, click **Open** and choose one stack file per channel, FITS
  or XISF. PSF-Studio reads each file's channel from its `FILTER` header or
  its name.
- From a terminal, run `psf-guard studio`; see `psf-guard studio --help`.

Each way runs `psf-guard studio`, so the stacks are fetched, registered,
calibrated and stretched by the same code. PSF-Studio finds `psf-guard`
from `--psf-guard`, then the `PSF_GUARD` environment variable, then beside
its own program, then on the `PATH`. Pictures are kept in
`~/Library/Application Support/psf-studio/pictures`, never beside your
stacks, and opening the same target or files again keeps the document.
PSF Studio saves the document as you work, within a quarter of a second
of each change; there is no Save command.

## Layout

| Path | Contents |
|---|---|
| `render/` | The canvas: a GPU tile cache and three passes: masks from strokes, layers blended into one image, then the finish steps and the display transform. It depends on wgpu only. |
| `app/` | The `psf-studio` program: a GPUI window that hosts the canvas in a native view, with the tree, the inspector and the tools. macOS only. |
| `color/` | The display transform built from ICC profiles with Little CMS, and a check of the shader against Little CMS and ColorSync. macOS only. |
| `experiments/crop/` | Seiza deconvolution previewed on a crop, and `store_bench`, which measures PSF Guard's tile store. |
| `experiments/web/` | The canvas as it was when the web comparison was measured, in WebGPU. |
| `experiments/results/` | The measurements quoted in the design record. |

The experiments answer investigations 1, 3 and 9 of the design record.

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
cargo run --release -p psf-studio
```

Drag to pan. Scroll with Command held to zoom. Choose **Brush** and drag to
paint a mask. Drag a band's slider to change its midtone; click a band's name
to turn the band off.

## Open a tile store

The canvas draws the synthetic document unless it is given a tile store, the
file format of PSF Guard's `src/studio/store.rs`. Write part of the synthetic
document to a store, then open it. The default is 12,000 by 8,000 pixels in
eight bands, which takes 4.1 GB.

```bash
cargo run --release -p psf-studio -- --make-store /tmp/synthetic.pgtiles
```

```bash
cargo run --release -p psf-studio -- --open /tmp/synthetic.pgtiles
```

To open a real FITS or XISF frame, write a store from it with PSF Guard,
from the repository's root, and open that:

```bash
cargo run -- studio-store /path/to/master.fits --output /tmp/master.pgtiles
```

The canvas takes the document's size, pyramid levels and bands from the
store. One band is shown as a gray layer, three as red, green and blue, and
any other count as that many layers, up to eight. Each band's stretch is
Seiza's automatic midtones stretch, resolved from patches of the image at
full resolution; the band's slider sets where the background lands.

## Run the measurements

Keep the window visible and in front while a measurement runs. A covered
window gets no frames.

```bash
cargo run --release -p psf-studio -- --bench results/native.json
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
cargo run --release -p psf-studio-color -- results/color.json display-0.icc
```

Run the canvas on a wide-gamut surface. `--output` is `srgb8`, `p3-8`,
`p3-float` or `p3-extended`. Studio composes in Rec. 2020 on the sRGB
curve, and the canvas maps the picture into the surface's primaries: sRGB
for `srgb8`, Display P3 for the others. `p3-extended` also shows an HDR
picture's highlights brighter than white.

```bash
cargo run --release -p psf-studio -- --output p3-extended
```

The panel's **Layer** line shows the pixel format and color space that the
canvas's Metal layer ended up with.

## Measure the tile store

Give it a folder with room for the store: about 1.5 GB for the default
image, and 9.2 GB for the 600-megapixel one.

```bash
/usr/bin/time -l cargo run --release -p studio-crop-preview-spike --bin store_bench -- /tmp/store results/tile-store.json
```

```bash
/usr/bin/time -l cargo run --release -p studio-crop-preview-spike --bin store_bench -- /tmp/store results/tile-store.json 30000 20000 3
```

`--capture <prefix>` writes PNG captures of selected frames during a native
measurement.
