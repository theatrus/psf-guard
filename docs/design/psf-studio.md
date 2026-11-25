# PSF-Studio: an interactive, non-destructive image editor (proposed)

Status: proposed (2026-11-24). Nothing here is built. This record collects
the decisions made so far, the design they lead to, and the work to
investigate first. Current stack and color preview behavior lives in
[Project stack previews](../STACKING_PREVIEWS.md); the current color pipeline
is described in
[Color previews and channel processing](color-channel-processing.md).

## Goal

Turn the stack preview and its processing into a full interactive editor:

- pre-processing before the stack;
- layers that combine any set of bands, with masks and adjustments;
- painted masks and manual tweaks;
- a saved history of edits;
- every edit non-destructive, with results cached to files because memory is
  limited.

Performance and responsiveness are the main reason PSF-Studio exists. A
design choice that makes a slider or a brush lag is the wrong choice.

## Decisions made

These came from the project owner and are not open for the investigation to
change. Raise a conflict; do not work around one.

| Subject | Decision |
|---|---|
| Purpose | Responsiveness first. See [Responsiveness targets](#responsiveness-targets). |
| Shell | PSF-Studio is a native GPUI application. The web UI gets no layered editor. |
| Web UI | Keeps automatic previews, the processing forms and saved setups, on the shared engine. |
| Automatic previews | Move to the new engine, with good defaults. One pipeline, not two. |
| Catalogs | Studio is the next step of the stack and preview flow, but works without a PSF Guard catalog. It can save back to PSF Guard, and the same engine renders the result there. |
| Own masters | A person can bring their own masters into a document. |
| Bands | More than SHO and RGB: NII, He II and others, all handled the same way as layers. |
| Masks | Interactive manual masking, painted masks and manual tweaks, with undo, redo and saved history. |
| Step view | A view that moves forward and backward through the applied transforms, separate from history, to inspect what each step does. |
| Steps | A person can add a step where it matters. |
| Reuse | A layer from an earlier pass can be duplicated into the final assembly. |
| Remote use | Important, but only to serve images: sign in, fetch, work. It need not run in a browser. |
| Large mosaics | Must work. Loading a whole image at once is not always possible. |
| MCP | The editor must be drivable by an MCP client. |
| Launch | PSF Guard starts Studio. Studio is bundled with the PSF Guard desktop installer. |
| Documents | Several documents per project, one per processing attempt. |
| Export | Layered PSD/PSB export is wanted ("maybe"). |
| Color | The canvas is color managed on the display it is shown on, using that display's profile. |
| HDR | HDR is for high-brightness displays and is delivered as AVIF, most likely with Display P3 primaries. A document made for HDR is edited while shown in HDR, and has a fallback for displays that are not HDR. |

## What exists today

- **Step caching.** Each processing step (deconvolution, BlurXTerminator,
  NoiseXTerminator, star removal, prepared color inputs) is stored as its own
  file. Its key is built from its settings, its inputs and the algorithm
  version. Source stacks are never rewritten. See
  [Output, caching, and invalidation](../STACKING_PREVIEWS.md#output-caching-and-invalidation).
- **A fixed pipeline.** The order of steps is written in code.
  `src/server/stack_preview/color.rs` is about 10,500 lines of one color
  pipeline; `stretch.rs`, `rc_astro.rs`, `narrowband.rs`, `emission.rs` and
  `color_calibration.rs` hold the rest.
- **Fixed bands.** `StackColorRole` has seven values: Luminance, Red, Green,
  Blue, Ha, Oiii, Sii. `StackNarrowbandPalette` lists the palettes, including
  `ForaxxSho` and `ForaxxHoo`.
- **No live editing.** Every change is **Apply processing**, a queued job, and
  a PNG.
- **GPU use.** Seiza runs debayering, detection, resampling and rejection on
  the GPU through wgpu (`src/server/stack_preview/gpu.rs`). There is no
  display-side GPU rendering.
- **MCP.** `src/server/mcp/` has stack tools but cannot return image content.
- **Cache janitor and budgets.** `stack_preview/janitor.rs` and
  `cache_budget.rs` remove cached files within a disk budget.

## Architecture

Three parts are independent of any UI:

1. **Engine crate.** The document model, graph evaluation, cache keys, tiles
   and pyramids, and the heavy operations, which call Seiza. PSF Guard's
   server and Studio both link it.
2. **Command layer.** Every edit is a named command with typed parameters.
   Studio's controls and the MCP tools both call these commands and nothing
   else.
3. **Render crate.** The light operations as wgpu shaders. It does not depend
   on GPUI, so another native toolkit could use it.

New image algorithms (curves, masks, blends, tiled forms of existing steps)
belong in Seiza crates, following the rule in the contributor guide.

### The document

A document is a versioned graph of typed nodes.

- **Sources.** A stack from a PSF Guard catalog, or an imported master (FITS,
  XISF, TIFF, read through `src/image_io.rs`). Imported masters are
  registered onto one reference grid with Seiza.
- **Chains.** An ordered list of steps on a source. A person can insert,
  reorder, turn off or remove a step at any position.
- **Layers.** A layer has a source, a stretch, adjustments, a mask, a blend
  mode and an opacity. Layers and groups form the final assembly.
- **Masks.** Masks are nodes: luminance range, hue range, star mask (from a
  star removal output), gradient, and painted strokes.
- **Types.** Each connection has a type: linear mono, linear RGB, display RGB
  or mask. The engine refuses a step whose input type is wrong, such as a
  tool that needs linear data placed after a stretch, and the editor explains
  why.

Outside a catalog, a document is a project folder with the document, its
history, mask strokes and a cache. Sources are referenced by path and
fingerprint. In a catalog, PSF Guard stores the document in a PSF Guard-owned
table, never in a Target Scheduler table. A project can have several
documents; one is marked current and its render is the one the card shows.

### Bands

A band record replaces the fixed roles: a name, a center wavelength, a
bandwidth and a kind (line, continuum or broadband). NII, He II, H-beta and
S III are band records like any other.

Every band is the same kind of layer: a mono source, a stretch, a color
assignment and a blend. A palette (SHO, HOO, Foraxx, RGB, LRGB) is a preset
that creates a group of band layers with named slots. Continuum subtraction
pairs a line band with a continuum band.

### Heavy and light operations

| Class | Examples | Where it runs | Cached |
|---|---|---|---|
| Heavy | stacking, background extraction, deconvolution, noise reduction, RC-Astro tools | a tracked job, as today | a file on disk, keyed by settings, inputs and algorithm version |
| Light | stretch, curves, saturation, color assignment, blend modes, mask application | a GPU shader, on every frame | not cached |

Heavy operations follow the existing rules for
[background jobs](background-jobs.md): a tracked job, a priority, leased
workers, and no heavy work in a request handler.

A heavy step previews on a full-resolution crop that the person places. A
downscaled proxy would misrepresent these steps, because their result depends
on pixel scale. Preview results are cached by their settings, so moving a
slider back is immediate. Committing the step runs the whole image as a job.

### Tiles and pyramids

Tiles are part of the first engine milestone, because large mosaics cannot be
loaded whole. A 600-megapixel mosaic with three float channels is about 7 GB
for one layer state.

- Each heavy result is stored as tiles with a pyramid of reduced levels.
- The viewer loads the tiles for the visible area at the matching level. Its
  memory follows the screen size, not the image size.
- Heavy steps work out of core: in tiles with overlap, within the processing
  memory budget. A global model, such as a background surface, is fitted
  across the whole image and then applied per tile.
- Statistics for auto-stretch and histograms come from a reduced level.
- A neighborhood step uses padded tiles, and its radius scales with the
  pyramid level, so a blur looks the same at every zoom.
- Painted masks rasterize per tile from the stored strokes.
- Export streams tile by tile.

### Step view

The step view is separate from history. It changes no settings.

- Select a step in any chain, and the canvas shows that chain's output at
  that step. Later steps remain in the document.
- Inspection modes: before and after (split or toggle), the amplified
  difference, and the step's mask as an overlay.
- A step before the stretch is shown through a temporary display stretch that
  is not part of the document. The same stretch is used for before and after.
- A slider on the viewed step updates that view live.

For this, every step's output has a stable identity, and the pyramid is built
for each heavy step, not only for the end of a chain.

### Reusing earlier passes

Any step's output can be a layer source in the final assembly: the starless
image before noise reduction, the stars image from star removal, a band
before sharpening.

- **Linked:** the layer follows the step. A later change to that chain
  reaches the layer.
- **Frozen copy:** the layer keeps the pixels as they were, stored as a file
  with a record of its origin.

A linear output used in the display-space assembly gets a stretch on its
layer. A layer cannot use an output that depends on that same layer. The
cache janitor must keep every output that a document links to or has frozen.

### History

History is an append-only log of commands per document, with undo, redo and
named snapshots. It is saved with the document. Each brush stroke is one
entry. A batch of commands can be one entry. Each entry records who made it:
a person, or an MCP client.

Stepping back is immediate while the cached files exist, and costs a
recomputation after the janitor has removed them.

### Painted masks and manual tweaks

- A stroke is stored as a vector: points, radius, hardness, and whether it
  adds or subtracts. Coordinates are on the reference grid, so strokes stay
  aligned when a stack refreshes.
- The GPU rasterizes strokes into the mask.
- A manual tweak, such as a local brighten, is a painted mask with an
  adjustment.

### Pre-stack processing

Frame selection, calibration choice, rejection and drizzle are settings on a
catalog source node. Changing one makes a new stack revision through the
existing build line. The edits after it are kept and evaluated again. An
imported master has no pre-stack settings.

### MCP

The MCP tool list is generated from the command definitions, so a new step
type needs no extra MCP work. An MCP edit is a history entry and can be
undone like any other.

An MCP client also needs:

- **The document:** chains, layers, parameters, band records, and which steps
  are out of date or still running.
- **Pixels:** a rendered view of the final image or of any step, at a chosen
  size or crop; the before and after pair or the difference for a step.
- **Measurements:** per-channel statistics, a histogram, background level,
  noise, and star count and size.
- **Jobs:** a heavy command returns a job reference; the client can read its
  state or wait for the result.
- **Batches:** several commands applied as one history entry.

The tools exist in two places. PSF Guard's MCP server works on documents
saved to a catalog, rendered by the server's engine. Studio offers the same
tools for the open document; that local MCP is off until the person turns it
on. The MCP edits documents and starts renders. It does not delete source
files or masters, and it exports only to allowed locations.

### Launching Studio from PSF Guard

1. **Open in PSF-Studio** on a stack or color card creates a document for the
   project, or opens the most recent one. A new document starts from the
   card's current processing.
2. PSF Guard issues a one-time ticket that lasts about a minute. The
   `psf-studio://` link carries the server address, the document id and the
   ticket. No long-lived token appears in a URL.
3. Studio exchanges the ticket for its own session with that server.
4. Studio fetches proxies first, then full-resolution tiles as needed.
5. Saving writes the document and its render back to PSF Guard.

| Where PSF Guard runs | How Studio starts | How Studio gets images |
|---|---|---|
| Desktop app (Tauri) | the app starts the bundled Studio directly | reads the cache files from disk |
| Server on the same machine, in a browser | the link | reads from disk when paths and fingerprints match, otherwise downloads |
| Remote server, in a browser | the link | downloads over the signed-in session |

A browser cannot detect whether the link has a handler. The button therefore
opens a dialog with two actions: **Open in PSF-Studio**, and
**Download link file**, which saves a small file that Studio opens through
its file association. The dialog also links to the Studio download.

From Studio, a person can sign in to a PSF Guard server, browse its projects
and stacks, and open one without a link.

### Remote use

The server serves tiles; Studio computes locally. For a large remote mosaic,
downloading the full data before a heavy step is slow. Two options to
investigate: running a heavy step on the server or its workers at Studio's
request, and fetching only the tiles a crop preview needs.

### PSD/PSB export

No Rust crate was found that writes layered 16-bit or 32-bit files. The Rust
port of `ag-psd` writes 8-bit only; PhotoshopAPI writes all depths but is
C++. The proposal is a small PSB writer of our own: 16-bit raster layers,
layer masks, groups, and the blend modes Photoshop shares. Adjustments are
baked into each layer. PSB is required for mosaics, because PSD stops at
30,000 pixels a side.

## Responsiveness targets

These are proposed starting numbers. The investigation should confirm or
correct them with measurements.

| Action | Target |
|---|---|
| Light-step slider on a 4K view | every display frame, under 16 ms |
| Brush stroke to visible mask | under 10 ms |
| Pan and zoom on a 600-megapixel mosaic | no blank tiles at normal speed; sharp within 100 ms |
| Heavy-step slider on the preview crop | first result within 1 s where the tool allows |
| Step view change, cached | under 100 ms |
| Open a document, local | proxy visible within 2 s |

RC-Astro tools will not meet the 1 s target; the crop keeps their wait
short.

## What to investigate first

Each item should end with a written result in this record: what was tried,
what was measured, and what it changes.

1. **GPUI canvas: go or no-go.** A wgpu canvas in a GPUI window showing a
   tiled, synthetic 600-megapixel, eight-band document, with one live stretch
   slider and a brush. Measure against the targets on macOS, Windows and
   Linux. GPUI's Linux renderer appears to be wgpu and a `gpu_canvas` hook
   appears to exist, but this is known only from third-party crate
   documentation (`gpui_interop`); verify it against GPUI's own source. If
   GPUI falls short, name the native toolkit that would use the same render
   crate.
   See [the result](#1-gpui-canvas-2026-11-24-macos-only-so-far).
2. **Pipeline parity.** Rebuild one section of `color.rs` as engine nodes
   with identical output and identical cache keys. This is the largest cost
   and risk in the plan: the automatic previews must move without changing
   results. Report how the rest of `color.rs` would divide into nodes.
3. **Crop preview loop.** One heavy step (Seiza deconvolution or the noise
   model) previewed on a full-resolution crop. Measure slider latency.
   See [the result](#3-crop-preview-loop-2026-11-24).
4. **Step view and tile keys.** Before and after on one chain, to settle how
   tiles and pyramids are keyed for intermediate outputs.
5. **Tile format.** Sample depth (half or full float), tile size,
   compression, and whether tiles can be mapped from disk straight to the
   GPU.
   See [the first result](#5-tile-format-2026-11-25-first-result).
6. **Out-of-core heavy steps.** Which Seiza operations already work in tiles,
   which need a tiled form, and how RC-Astro tools behave on a mosaic-sized
   file.
7. **Command layer shape.** Define the commands before the first editor
   control exists, and show an MCP tool list generated from them.
8. **PSB writer scope.** Confirm the minimum the format needs for 16-bit
   layers with masks, and whether PhotoshopAPI is worth binding instead.
9. **Color-managed display.** Show one image with an embedded profile
   correctly on a wide-gamut display and on an sRGB display, on macOS,
   Windows and Linux. Settle where the display transform runs: the operating
   system, from a color space set on the canvas surface, or the render
   crate's last shader step, from the display's ICC profile. Include moving
   the window between two displays, a 10-bit or float surface, and soft
   proofing. See [what is known](#color-management-what-is-known-2026-11-24).
   See [the result](#9-color-managed-display-2026-11-24-macos-numbers-only).
10. **HDR editing and AVIF export.** Edit one document while it is shown in
    HDR on a high-brightness display, and export it as AVIF. Settle:
    - what the document stores: where standard white sits in the stretch,
      and how far highlights may go above it;
    - how the canvas follows the display's headroom, which changes with the
      brightness setting, and what it shows on a display with none;
    - the AVIF encoding: Display P3 primaries with the PQ or the HLG
      transfer, at 10 or 12 bits;
    - the form of the fallback: an HDR file that each viewer tone-maps
      itself, or a standard image with a gain map, which fixes both
      renditions in the file. A gain map makes the standard rendition
      something the person edits too;
    - which Rust AVIF encoder writes those, and which browsers and systems
      show the result as intended.

## Investigation results

### 1. GPUI canvas (2026-11-24, macOS only so far)

The code and the result files are in
[`spikes/psf-studio-canvas`](../../spikes/psf-studio-canvas/README.md).

**Result so far: go on macOS, not yet answered for Windows and Linux.** A
GPUI window can host the tiled canvas and hold the display's frame rate. The
same canvas in WebGPU holds it equally well, so rendering speed does not
separate a native shell from a web one. Upstream GPUI has no way to host the
canvas on Windows or Linux; see
[What GPUI offers for a custom GPU canvas](#what-gpui-offers-for-a-custom-gpu-canvas).

#### What was built

- A render crate that depends on wgpu only. It draws a synthetic
  30,000 × 20,000 pixel (600-megapixel) document with eight bands. Tiles are
  512 × 512 with eight half-float layers, 4 MB each, in a seven-level pyramid.
  Worker threads compute tiles on request; the work stands in for decoding a
  tile and does not measure disk reads. The GPU keeps at most 2 GB of tiles
  and drops the least recently drawn first.
- One shader that stretches each band (black point, white point, midtone),
  assigns its color, sums the eight bands, and applies a painted mask. Brush
  strokes are stored as segments and drawn into the mask again on every
  frame.
- A GPUI app that shows the canvas beside a panel with one midtone slider per
  band, a brush, and pan and zoom.
- A web page that draws the same document with the same shader file through
  WebGPU, with tiles computed in Web Workers.
- A script that both play: 600 frames each of moving every band's midtone,
  painting, painting on top of 20,000 stored segments, panning at 1500 and at
  6000 screen pixels per second, and zooming from the whole document to 1:1.

#### What GPUI offers for a custom GPU canvas

Checked against GPUI's source in the Zed repository at commit `f98c2c1`.

- GPUI draws with its own Metal renderer on macOS, a DirectX 11 renderer on
  Windows, and wgpu on Linux and the web.
- **There is no `gpu_canvas` hook in upstream GPUI.** That hook belongs to a
  fork, published as `bite-gpui`, and `gpui_interop` is written for the fork.
  The fork shares a GPU texture with GPUI on Linux and Windows.
  `gpui_interop` lists macOS as not yet implemented.
- Upstream GPUI has `NativeView`, which places a platform view inside a GPUI
  window. GPUI composites the view beneath its own content, so GPUI elements
  can cover it. `NativeView::new` fails on every platform except macOS.
- Upstream GPUI also has `Window::paint_surface`, which takes a
  `CVPixelBuffer`. It exists on macOS and iOS only.

GPUI Kit does not change this. GPUI Kit (`gpui-kit` 0.11.0, with
`gpui-component` and `gpui-base`) is a component library on top of GPUI. It
builds on `gpui-pre` 0.3.14, a snapshot of Zed's GPUI, or on the `gpui-fast`
fork. Its windows and components run on macOS, Windows and Linux. Neither
GPUI build has a `gpu_canvas` hook, `NativeView` is macOS only in both, and
GPUI Kit has no component that shows an application's own GPU texture. The
separate `gpuikit` 0.9.0 crate builds on `gpui-unofficial` 1.14.2, which has
neither hook.

So the interface is portable and the canvas is the part that is not. GPUI
draws everything in a window itself, and what it lacks outside macOS is a way
to place pixels that another renderer drew.

The spike uses `NativeView`: an empty `NSView` whose layer wgpu owns. A
transparent GPUI element in front of the view receives the mouse, so GPUI
handles all input. The canvas therefore has its own GPU device, separate from
GPUI's.

Two build facts: GPUI needs Rust 1.100, and PSF Guard pins 1.99.1. GPUI's
macOS build runs Xcode's `metal` compiler unless the `runtime_shaders`
feature is on.

#### Measurements

Machine: Apple M4 Max, 128 GB, macOS 27.2.1, one 60 Hz display at 2×. The
canvas surface is 3840 × 2160 pixels in every run. Browsers: Chrome 157 and
Safari 27.2. A frame is late when it follows the previous frame by more than
20 ms; on this display a frame is due every 16.7 ms.

Late frames out of 600:

| Phase | GPUI, canvas drawn on GPUI's thread | GPUI, canvas drawn on its own thread | Chrome | Safari |
|---|---|---|---|---|
| Midtone of all 8 bands, whole document | 2 | 2 | 0 | 0 |
| Midtone of all 8 bands, 1:1 | 2 | 5 | 0 | 0 |
| Brush | 1 | 5 | 1 | 1 |
| Brush over 20,000 stored segments | 3 | 5 | 0 | 0 |
| Pan at 1500 px/s | 35 | 8 | 3 | 1 |
| Pan at 6000 px/s | 96 | 16 | 12 | 0 |
| Zoom from whole document to 1:1 | 27 | 4 | 3 | 1 |

The native runs time each frame with a clock when the frame is handed to the
display. The browser runs use the frame timestamps the browser supplies,
which are rounded to the display's refresh, so a native frame that is a few
milliseconds late counts and the same frame in a browser may not. Read the
columns as “no worse than”, not as a ranking.

Cost of one native frame at 3840 × 2160:

| Measure | Value |
|---|---|
| CPU time to choose tiles, encode and submit | about 1 ms median, 2 to 4 ms at the 99th percentile |
| GPU time for the mask and the eight-band composite | 0.6 to 0.8 ms |
| GPU time with 20,600 brush segments drawn again | 0.75 ms |
| Frame interval, every measured phase | 16.7 ms median, at most 20.7 ms at the 99th percentile |
| Time to compute one tile on a worker | 14 to 16 ms native, 23 to 37 ms in a Web Worker |

Tiles, in every run on both sides:

- No frame had a blank tile. The coarsest level stays loaded, and a tile that
  is not loaded yet is drawn from a coarser level.
- Panning at 1500 px/s, every visible tile came from its own level in every
  frame. At 6000 px/s, at most 5 tiles came from a coarser level, for at most
  6 frames.
- After a pan or zoom stopped, the view was sharp within 35 ms.
- After a jump to a part of the document that was not loaded, the view was
  sharp after 140 to 340 ms natively and 145 to 825 ms in the browsers. The
  longest time on each side was the first view after launch. That time is 40
  to 45 tiles of synthetic work, so the tile-format investigation decides
  the real number.
- During the continuous zoom, some tiles came from a coarser level in about
  85 of 600 frames natively, 90 in Chrome and 145 in Safari.

#### Against the targets

| Target | Finding |
|---|---|
| Light-step slider on a 4K view: every display frame, under 16 ms | Met. A frame costs about 2 ms of CPU and GPU time. With the canvas on its own thread, 2 to 5 frames of 600 were more than 3 ms late. |
| Brush stroke to visible mask: under 10 ms | The work takes under 2 ms, so a stroke is in the next frame. On a 60 Hz display the next frame can be 16.7 ms away, so the target as written cannot be met by any toolkit. Time from pen to screen was not measured. Proposed wording: “in the next display frame”. |
| Pan and zoom on a 600-megapixel mosaic: no blank tiles; sharp within 100 ms | Met for panning and for the end of a zoom. Not met after a jump to an unloaded area (140 ms or more with synthetic tiles). |
| Heavy-step slider on the preview crop: first result within 1 s | See [investigation 3](#3-crop-preview-loop-2026-11-24). |
| Step view change, open a document | Not covered yet. |

#### What this changes

1. **Draw the canvas on its own thread.** Drawn on GPUI's thread, each frame
   waited up to 15 ms for the display inside GPUI's paint, and tile uploads
   ran there too. Moving the canvas to its own thread cut late frames during
   a fast pan from 96 to 16. It also lets the canvas keep drawing while GPUI
   lays out the panel.
2. **Rendering speed does not decide native against web.** On this machine
   the web baseline was as smooth as the native canvas. The spike did not
   measure the places where the two differ: time from input to screen,
   reading tiles from local files against fetching them, memory limits in a
   browser tab, a heavy step computed in the same process, and output beyond
   8-bit sRGB. The crop preview and tile format investigations cover most of
   these and should compare both.
3. **Conflict with the Shell decision.** The decision is a native GPUI
   application. Upstream GPUI can host the canvas on macOS only. On Windows
   and Linux the choices are:
   - the `bite-gpui` fork with `gpui_interop`, which does not cover macOS, so
     Studio would track a fork and still need `NativeView` on macOS;
   - a child window or surface of our own, created from the window handle
     GPUI exposes on every platform, written once per platform. This keeps
     upstream GPUI and GPUI Kit unchanged, and is the first path to try;
   - a hook contributed to GPUI. On Linux GPUI already draws with wgpu, so
     sharing its device is a small change there; Windows draws with
     DirectX 11 and needs a shared texture;
   - a toolkit that draws its interface with wgpu and lets an application
     add its own render pass on the same device, such as egui or iced. The
     render crate would be used unchanged. Neither was tried.
4. **A web shell has its own gap.** The PSF Guard desktop app shows the web
   UI in the system WebView. WebGPU ran in Safari 27.2 here; it was not
   tested in the app's WebView on macOS, in WebView2 on Windows, or in
   WebKitGTK on Linux.

#### Not done yet

- Windows and Linux, on any path.
- Time from input to screen, which needs a real pointer and a camera or a
  latency probe.
- A check by eye of the GPUI panel over the canvas. The measurements ran in
  the GPUI window and the canvas was captured from the GPU, but this session
  could not take a screenshot of the window.
- A slower GPU. An M4 Max leaves about ten times the headroom the targets
  need, so these numbers do not show where an integrated GPU falls short.

### 3. Crop preview loop (2026-11-24)

The code is the `crop` crate in
[`spikes/psf-studio-canvas`](../../spikes/psf-studio-canvas/README.md), with
`web/crop.html` for the browser side.

**Result: the 1 s target is met with a wide margin, in process and through a
local server. The preview does not yet show what the committed step will
produce; that needs a change in Seiza.**

#### What was measured

The step is `seiza_deconvolution::deconvolve`, the fixed-PSF Richardson-Lucy
update PSF Guard uses today, on a 6144 × 4096 linear mono image cut from the
synthetic document. Same machine as investigation 1. All times are medians.

Time for one preview in process, with 32 pixels of padding on each side:

| Crop | 4 iterations | 10 iterations | 20 iterations | 50 iterations |
|---|---|---|---|---|
| 512 × 512 | 5.4 ms | 10 ms | 19 ms | 44 ms |
| 1024 × 1024 | 11 ms | 18 ms | 30 ms | 65 ms |
| 2048 × 2048 | 32 ms | 47 ms | 70 ms | 139 ms |

- PSF width matters little: a 1024-pixel crop at 10 iterations takes 18 ms at
  a FWHM of 1.5 pixels and 24 ms at 12 pixels.
- The whole 25-megapixel image takes 253 ms at 10 iterations.
- A setting seen before is answered from a cache of results in 0.3 ms, which
  includes converting 1024 × 1024 samples to half floats for the GPU.
- A 512-pixel crop is faster on 4 threads than on 16 (3 ms against 5.4 ms at
  4 iterations). A 2048-pixel crop is faster on 16. The preview should lease
  workers by crop size.

The same previews computed by a local HTTP server and fetched by a page,
which uploads each result to a GPU texture. The server sends half floats,
uncompressed. Overhead is the whole trip less the server's compute time:

| Crop | Bytes per preview | Overhead in Chrome 157 | Overhead in Safari 27.2 |
|---|---|---|---|
| 512 × 512 | 0.5 MB | 3.9 ms | 3.4 ms |
| 1024 × 1024 | 2 MB | 8.3 ms | 8.3 ms |
| 2048 × 2048 | 8 MB | 13.7 ms | 16.3 ms |

On one machine a web editor's preview is therefore about half a display
frame slower than an in-process one at 1024 pixels, and one frame slower at
2048. Over a network the 2 to 8 MB per preview is the cost that matters, and
it was not measured. Studio avoids it by computing on the machine that shows
the result.

#### Two findings about the step

1. **`amount` is a blend.** The result at any `amount` equals the input plus
   `amount` times the difference to the result at `amount` 1, to within
   1.2 × 10⁻⁷. The canvas can apply `amount` as a light step from one cached
   result, so that slider never runs the step.
2. **A crop does not match the whole image.** The step takes its noise floor
   from the range of the samples it is given, and scales flux over those
   samples. A crop has a smaller range than the whole image, so the same
   settings damp it differently. On the test crop the preview differed from
   the same pixels of the whole-image result by 13% of what the step
   changed, and padding did not help. When the crop was given the whole
   image's noise floor, 8 pixels of padding brought the difference to 0.01%.

#### What this changes

- **Seiza:** `deconvolve` needs a way to take the sample range, and the flux
  scale, from the caller, so that a crop and a tile use the whole image's
  values. The out-of-core form in investigation 6 needs the same change:
  tiles processed separately would otherwise disagree at their edges. Check
  every heavy step for statistics computed from its input.
- **Engine:** a heavy step's preview cache key includes the crop rectangle
  and the whole-image statistics the step used. A step declares the padding
  it needs.
- **Document:** `amount`, and any other setting that only blends a step's
  output with its input, belongs to the light path.
- **Memory:** the step holds about eight copies of its input as 32-bit
  floats. A 600-megapixel channel would need about 19 GB in one piece, so
  the whole-image run must work in tiles.

#### Not done yet

- A step that is slow enough to need the crop: the noise model and the
  RC-Astro tools.
- A preview that cancels the run in progress when the slider moves again.
  Each measurement here ran to completion.
- The preview shown in the GPUI canvas. This harness has no window.

### 5. Tile format (2026-11-25, first result)

The code is `src/studio/store.rs`, with a measurement program in the spike
(`crop/src/bin/store_bench.rs`).

**Result so far: a plain, uncompressed tiled file with a pyramid is fast
enough and holds almost nothing in memory. Compression and mapping tiles
straight to the GPU are not yet tried.**

#### The format

One file per image.

- A short header in JSON: the image's shape and domain, the tile size, the
  sample format, a key for what the store was made from, and each level's
  size and offset.
- Then each level's samples, the image first. A level is stored tile by
  tile; a tile is 512 pixels a side, less at the right and bottom edges,
  with its samples row-major and channel-interleaved. A tile's offset
  follows from the level's size, so the file has no index.
- Each further level halves the one before it, leaving out samples that are
  not finite, so a registration border does not darken the pyramid. The last
  level fits in one tile.
- Samples are 32-bit floats, or 16-bit floats at half the size.

A store is written beside its final path and moved into place when it is
complete. A failed write removes the partial file and leaves an existing
store as it was.

#### Measurements

Same machine as investigation 1, internal SSD. The source is computed as it
is read, so the image is never in memory. Reads are from a file that was
just written, so they come from the system's file cache; a first read from
a cold disk was not measured.

| | 96 megapixels, 3 channels | 600 megapixels, 3 channels |
|---|---|---|
| Raw size as 32-bit floats | 1.1 GB | 6.9 GB |
| Store, 32-bit floats | 1.46 GB (1.33 times raw) | 9.2 GB |
| Store, 16-bit floats | 0.73 GB | 4.6 GB |
| Levels | 6 | 7 |
| Time to write, either format | 0.8 s | 5.0 s |
| Read a 1024 × 1024 crop, 32-bit | 1.4 ms | 1.6 ms |
| Read a 3840 × 2160 view at full resolution, 32-bit | 10.8 ms | 13.5 ms |
| Read the whole top level | 0.1 ms | 0.2 ms |
| Peak memory of the process | 247 MB | 242 MB |

- **Memory does not grow with the image.** Writing a 6.9 GB image and
  reading from it peaked at 242 MB, most of it the 95 MB views the
  measurement asked for.
- **Read time does not grow with the image.** A crop takes the same time
  from a 96-megapixel store as from a 600-megapixel one.
- **16-bit floats are slightly slower to read** (1.8 ms a crop), because
  each sample is converted. They halve the file.

#### What this settles, and what it leaves

- Tile size: 512 pixels. Not compared with other sizes.
- Sample format: 32-bit floats for linear results, where faint signal
  matters. 16-bit floats are an option for display-referred results, with
  about three significant digits.
- The pyramid costs one third more disk.
- Still open: compression (a noisy linear stack compresses poorly, and it
  would end offsets that follow from the level's size); mapping tiles from
  disk to the GPU; a cold disk and a network share; and how the janitor
  counts stores against the disk budget.

### 9. Color-managed display (2026-11-24, macOS, numbers only)

The code is the `color` crate and the `--output` and `--working` options of
the app in
[`spikes/psf-studio-canvas`](../../spikes/psf-studio-canvas/README.md).

**Result so far: the canvas can present a wide-gamut, floating-point,
color-managed surface on macOS at no cost in frame rate, and a shader step
can do the display conversion itself to within a quarter of an 8-bit level.
Nobody has yet looked at the screen with an instrument, and Windows and
Linux are untested.**

#### What the canvas surface offers on macOS

Read from wgpu 30 on the Pro Display XDR:

- Formats: 8-bit BGRA, 8-bit BGRA sRGB, 10-bit RGB with 2-bit alpha, and
  16-bit float RGBA.
- Color spaces for the 8-bit formats: sRGB and Display P3.
- Color spaces for 16-bit float: those two, plus extended sRGB (linear and
  encoded), extended Display P3, and BT.2100 PQ and HLG.

The app ran the full measurement script in four modes and read the Metal
layer's settings back from AppKit each time:

| Mode | Layer pixel format | Layer color space | Extended range |
|---|---|---|---|
| 8-bit sRGB | BGRA 8-bit | sRGB | no |
| 8-bit Display P3 | BGRA 8-bit | Display P3 | no |
| Float Display P3 | RGBA 16-bit float | Display P3 | no |
| Float extended Display P3 | RGBA 16-bit float | extended Display P3 | yes |

In each mode macOS converts from the layer's color space to the display's
profile, and does so again by itself when the window moves to another
display.

wgpu offers no setting for “the display's own profile, unconverted”. A
canvas that does the whole conversion itself on macOS would have to set the
layer's color space directly. That was not tried.

#### The conversion as a shader step

The render crate gained a display transform as its last light step. The host
builds it from two ICC profiles with a color engine; the render crate only
samples it. The check sends 65,536 colors through the shader on the GPU and
compares them with Little CMS converting the same colors directly, for four
working spaces (sRGB, Display P3, Adobe RGB, ProPhoto) and two display
profiles (Pro Display XDR and LG 27GN950). A quarter of the colors lie on
the faces of the color cube, where gamuts differ most. Differences are in
8-bit levels.

| Form of the transform | Mean | 99.9th percentile | Worst |
|---|---|---|---|
| 3D table, 65 nodes per axis, half float | 0.03 to 0.07 | 0.1 to 5.8 | 0.8 to 14 |
| Curve, matrix, curve; 32-bit float curves | under 0.004 | under 0.05 | 0.25, and 1.3 for one color |
| Curve, matrix, curve; half-float curves | 0.03 | under 0.19 | 1.7 |

- **A plain 3D table is not accurate enough.** It is right on average and
  wrong by up to 14 levels for saturated colors, because the target's curve
  is steep near black and a table of encoded values cannot follow it. Tables
  of 17 and 33 nodes are worse.
- **Curve, matrix, curve is exact enough,** and both display profiles here
  are of that kind. The curves need 32-bit float textures with linear
  filtering, which is an optional GPU feature; this Mac has it.
- A display profile built on a lookup table instead of curves, as some
  calibration tools write, still needs a 3D table, with shaping curves
  before and after it. That was not built.

Cost in the live canvas at 3840 × 2160: the transform adds 0.4 to 0.9 ms of
GPU time per frame (0.4 to 0.6 ms without, 0.8 to 1.6 ms with). A float
surface adds nothing measurable over an 8-bit one. The median frame interval
stayed at 16.7 ms in every mode. Late-frame counts varied more between
repeated runs than between modes, because two other programs were each using
a full CPU core during these runs; they do not show a cost of the transform.

#### Two findings that affect the design

1. **Color engines disagree near black.** Little CMS and macOS ColorSync
   agree to within rounding on the Pro Display XDR profile. On the 27GN950
   profile, whose curves are a plain power of 1.96, they differ by up to 4
   levels in dark channel values: for an sRGB value of 3, Little CMS gives 7
   and ColorSync gives 3. ColorSync appears to limit the curve's slope near
   black; that reading was not confirmed. Faint signal near black is what an
   astrophotographer stretches, so this is visible in exactly our images. If
   macOS converts for the surface and our own shader converts on other
   platforms, the same document will show different shadows on each.
2. **An 8-bit Display P3 surface clips.** Colors are cut to Display P3
   before the system converts them. For sRGB, Display P3 and Adobe RGB
   working spaces this changed no test color by more than one level on
   either display. For ProPhoto it changed 7% of the test colors on the Pro
   Display XDR (by up to 5 levels) and 25% on the 27GN950 (by up to 36),
   because that display shows colors outside Display P3. An extended-range
   float surface does not clip.

#### What this changes

- **macOS:** present to a 16-bit float surface in extended Display P3 and
  let the system convert. The canvas converts from the working space to
  Display P3 only. Moving between displays then needs no work.
- **Windows and Linux:** the render crate's curve, matrix, curve step, with
  the display's profile read per platform. Investigation 1's child surface
  must come first.
- **Engine:** the display transform is a light step with a cache key of the
  working profile, the target, and the engine's version. Decide whether one
  engine's handling of black is used everywhere, including on macOS.
- **Requirement on the GPU:** linear filtering of 32-bit float textures, or
  a fallback that reads the curves without filtering and interpolates in
  the shader.
- **Web:** a WebGPU canvas can be set to sRGB or Display P3, 8-bit or float,
  and the browser converts. A page cannot read the display's profile, so
  the second route is closed to it.

#### Not done yet

- **A measurement of the screen.** Every result here compares numbers before
  the display. Confirming the light needs a colorimeter, or a person
  comparing against a color-managed reference.
- Windows and Linux.
- HDR. The surface is set up for it, and investigation 10 covers the rest.
- Values above 1 on the extended surface. The shader still clamps to the
  0 to 1 range, so the extended surface is set up but not yet used for
  colors outside Display P3 or for highlights.
- The 10-bit format, soft proofing, and a display profile built on a lookup
  table.
- The window moved between the two kinds of display while showing a known
  image.

### Color management: what is known (2026-11-24)

Read from source, not yet tested. This is the starting point for
investigation 9.

- **GPUI does not manage color for an application.** On macOS its renderer
  can set its own layer to Display P3, as an option, and otherwise leaves
  the default. Its Windows and Linux renderers set no color space. Images that GPUI draws are 8-bit and carry no profile.
- **The canvas has its own surface, and that is what makes color management
  possible.** In the spike wgpu owns the canvas layer, separate from GPUI's.
  A hook that drew the canvas into GPUI's own target would take GPUI's color
  space and 8-bit depth, so a separate surface is the better design for an
  image editor and not only a way around a missing hook.
- **wgpu 30 can set a color space on a surface:** sRGB, Display P3, extended
  sRGB (linear or encoded), extended Display P3, and BT.2100 PQ and HLG,
  where the platform supports them. On macOS the system then converts from
  that space to the display's profile.
- **The spike's canvas today** is 8-bit and uses the layer's default, which
  macOS treats as sRGB. It is color matched, but limited to sRGB.
- **Windows and Linux are the open part.** A surface color space there covers
  the standard spaces at most, and whether the system converts to the
  display's ICC profile depends on the system's settings. The portable way
  is for the render crate to do the conversion itself as its last light
  step: build a 3D lookup table from the display's ICC profile and apply it
  in the shader. That needs the profile of the display the window is on,
  read through each platform's own API, and a new table when the window
  moves to another display.
- **A browser** lets a WebGPU canvas declare sRGB or Display P3 and converts
  to the display itself. A page cannot read the display's profile or supply
  its own transform, so soft proofing and a custom rendering intent would
  have to be computed without knowing the display.

## Building blocks: what Seiza needs first (2026-11-24)

The owner's direction: build the editor's core from primitives in Seiza
first. A mono chain and a one-shot-color (OSC) chain each run as steps, then
go through masked editing and layer recombination. This section lists what
is missing and the order to build it. It was written after reading Seiza at
release 0.56.0.

### What Seiza has

- **Linear steps:** stacking, with demosaicing for OSC; background
  extraction; deconvolution; color calibration from Gaia; continuum
  subtraction; learned noise and sharpening models, which already tile.
- **Linear to display:** `seiza-stretch`, which analyses, resolves and then
  applies, and keeps an ordered `StretchStack`.
- **Composition:** RGB, LRGB, SHO, HOO, Foraxx, star stretch and screen
  recombination, each as a fixed function.
- **A plan:** Seiza's `docs/design/editing-tools.md` proposes a `seiza-edit`
  crate with domain tags, masks, recipes, curves, saturation and multiscale
  tools. None of it is built.

### What is missing

1. **A step that runs on a region.** Every step takes a whole image.
   [Investigation 3](#3-crop-preview-loop-2026-11-24) measured the cost:
   deconvolution takes its noise floor from the pixels it is given, so a
   crop differs from the whole image by 13% of the step's change. A step
   must resolve a plan once from the whole image and then apply that plan to
   any region, with a stated padding. Crop previews, tiled mosaics and cache
   keys all depend on this.
2. **More than one or three channels.** Seiza's steps take a mono image or
   interleaved RGB. Mono work needs any number of named bands. OSC needs RGB
   that can later split into bands.
3. **A tag for linear or display-referred samples** on every image a step
   takes, and a refusal when a step is given the wrong one.
4. **Masks:** one shared type, its sources (a luminance range, a star layer,
   painted strokes) and ways to combine them.
5. **Layer recombination:** blend modes, opacity and masked blends as
   general operations. They exist today only inside functions such as the
   star screen.
6. **A record of steps** of mixed kinds, each with its resolved plan.
   `StretchStack` covers stretches only.
7. **One definition for each light step, in two forms.** The canvas runs
   stretch, curves, blends and masks as shaders. Each needs a CPU form and a
   shader form in Seiza, held together by a test, or the preview and the
   export differ.
8. **Display adjustments:** curves, saturation and SCNR, planned for
   `seiza-edit`.
9. **Color-managed and HDR output:** ICC profiles and AVIF in
   `seiza-image-io`.

### Order of work

Each item is built in PSF Guard first unless it says otherwise.

1. Keep Seiza's editing-tools design record in step with the findings
   above when the first piece moves there: region application with
   whole-image statistics, any number of bands, and the two-form rule for
   light steps.
2. **Foundations** (built, in `image_steps`): `Region`, the sample domain and its refusal, `Mask`, and
   the contract for applying a step to a region, with a check that every
   step must pass: a region's result equals the same pixels of the
   whole-image result. No new image algorithm.
3. **Deconvolution on the contract.** It is the step with the measured
   defect, and the check from step 2 is its test. This one needs a change
   inside `seiza-deconvolution`: the caller supplies the sample range, and
   the flux scale, which comes from the finished whole-image result and
   which a crop cannot know, is replaced or dropped. Then background and
   stretch, which already separate fitting from applying, then color
   calibration, continuum subtraction and the learned models.
4. **The step record,** shaped by the plans of the steps converted in step 3.
5. **Masks and blends,** CPU and shader together: luminance and star masks,
   painted strokes, blend modes, linear and screen recombination.
6. **Band records** in color composition, in place of the fixed roles.
7. **Light display steps:** SCNR, curves and saturation.
8. Later: tiled storage for mosaics, the multiscale tools, color-managed and
   AVIF export.

After step 3, a mono chain and an OSC chain run on a region and their
previews match the committed result. After step 5, masked editing and layer
recombination work. PSF Guard's engine crate and Studio build on those.

### Where the work happens

The owner's direction (2026-11-25): add no new Seiza crate now, and iterate
on larger additions in the PSF Guard repository first. A piece moves to Seiza
once its shape has settled against real steps.

- **The foundations** are the `image_steps` module in PSF Guard's main
  crate: `ImageShape`, `Region`, `Domain`, `Mask`, and the `RegionOperation`
  contract with `region_mismatch`, the check that a region's result matches
  the whole image's. They add no image type; a step takes a slice with its
  shape, as Seiza's steps do.
- **A Seiza step is wrapped here** in a `RegionOperation` and tested with
  `region_mismatch` before its crop preview is relied on. Where a step
  cannot keep the contract from outside, as with deconvolution's noise
  floor, the change to that step is a small PR to its existing Seiza crate,
  following the usual Seiza workflow.
- **A first version was opened as a new Seiza crate** (theatrus/seiza#504)
  and withdrawn on that direction. Its branch, `image-foundations`, remains
  as the starting point for the later move, together with the amendment it
  made to Seiza's `docs/design/editing-tools.md`.

A review of that first version changed the design, and the changes are in
the module: region arithmetic cannot overflow, a masked blend is exact where
the mask is 0 or 1, and a block carries the whole image's shape so a step
can tell which of its sides are image edges.

## Staged workflow and the scaffolding (2026-11-25)

The owner's direction: Studio is a staged workflow, with different panes for
each stage. Manual edits, the heal brush and masking are possible at
different steps. At each junction between stages a person can instead open
the intermediate image in an external editor, such as Photoshop, correct it
there, and continue from the corrected file. Memory use, work on the
viewport, and efficient previews of very large images shape the design from
the start.

The scaffolding is the `studio` module in PSF Guard's main crate. It has no
window and no GPU code. The canvas and the GPUI panes will call it.

### Stages

A document has six stages in a fixed order. Each stage holds an ordered list
of steps and ends at a junction.

| Stage | Works on | Typical steps | Panes |
|---|---|---|---|
| Sources | linear | choose stacks or masters, name bands, register, crop | source list with band records; coverage and registration check |
| Linear | linear | background, deconvolution, noise reduction, star separation, color calibration | step list; canvas with a temporary display stretch; full-resolution crop preview; measurements |
| Stretch | linear to display | one stretch per band or layer | histogram with the transfer curve; before and after |
| Combine | display | layers, each with its own color, mask, adjustments and blend; palettes; star recombination | layer list; the selected layer's adjustments and mask; band color assignment |
| Finish | display | adjustments to the combined image as a whole: curves, saturation, local contrast, retouching | adjustment list; mask list; before and after |
| Export | display | color space, standard and HDR renditions, file format | rendition previews side by side; file settings |

A mono document has one chain per band through Sources, Linear and Stretch,
and the bands meet in Combine. An OSC document has one RGB chain through
those stages and may split into bands by dual-band extraction in Linear.

Combine and Finish divide the work after stretching:

- **Combine is per layer.** Each layer has a source, a color, a blend mode,
  an opacity, a mask, and its own list of adjustments. Editing that is
  specific to one band, to the stars, or to one region of one layer happens
  here. A layer's source is a band's stretched result or the result of any
  earlier step, either following that step or frozen as it was.
- **Finish is global.** Its steps act on the combined image, every layer at
  once. A Finish step may still carry a mask, so "global" means across
  layers, not necessarily across the whole frame.

A step refuses a stage whose samples it does not work on. Deconvolution is
refused in Finish, and curves are refused in Linear.

### Manual edits

Painting a mask and healing are steps like any other, and are allowed in
every stage from Linear to Finish. Each is stored as strokes in document
coordinates, never as pixels, so the edit is small, can be undone, and is
drawn again at any zoom.

- **A painted mask** limits the step it is attached to. Strokes add to it or
  subtract from it, in order.
- **The heal brush** copies from a nearby source area into the stroke and
  matches the brightness around the two areas, so the patch does not show as
  a step in the background. The brightness difference is measured once, when
  the stroke is made, and stored with the stroke. A crop and the whole image
  then heal alike.

### Junctions and external editors

The junction at the end of a stage is where its result may be replaced.

1. **Export.** Studio writes the stage's result to a file in the document's
   folder and records the file's size, modification time and SHA-256 hash,
   and a key for everything upstream of the junction.
2. **Edit.** Studio opens the file in the editor the person chose. Studio
   does not wait on the editor's process.
3. **Return.** When the file's size or time changes, Studio hashes it. A
   different hash means the person saved an edit, and later stages read
   from the edited file. A file that was rewritten with the same content is
   not an edit.
4. **Upstream changes.** If a step before the junction changes afterwards,
   the upstream key no longer matches. Studio keeps the edited file and
   reports that it was made from an older result. The person chooses: keep
   it, or export again and redo the edit.

An external edit replaces pixels, so it is the one kind of edit that cannot
follow an upstream change. That is why it lives at a junction and is shown
as stale, and is not hidden inside a chain.

Still to settle: the file format for the round trip. Photoshop needs a
16-bit or 32-bit TIFF or PSB with an embedded profile; a linear stage needs
32-bit float to survive it.

### Memory and very large images

Nothing in the scaffolding takes a whole image. Three rules:

1. **Everything reads by region.** A source, a cached result, an edited file
   and a chain of steps all answer the same request: the samples of one
   region. A step applied to a source is itself such a reader. It asks its
   source for the region grown by its padding, applies, and cuts the padding
   off. Stages stack this way, and no stage holds more than the block it is
   working on.
2. **Every read has a price before it runs.** A reader reports the peak
   bytes a region will need: the block, the result, and the step's working
   copies. Seiza's deconvolution holds about eight copies of its input.
3. **A read over budget is cut into tiles.** Each tile is read with its own
   padding and written into the result, so the whole equals an uncut read.
   If one tile of the smallest size still exceeds the budget, the read is
   refused with both numbers. Nothing falls back to loading the image.

### Work on the viewport

The canvas asks for what is on screen, at the pyramid level that matches the
zoom. A document chain is built for that level:

- A step that maps each pixel alone runs unchanged at any level.
- A step with a radius runs with the radius scaled to the level, so a blur
  looks the same at every zoom.
- A step whose result depends on pixel scale, such as deconvolution or noise
  reduction, is left out of a reduced view, and the view reports which steps
  it left out. Those steps are shown on a full-resolution crop, and their
  committed result is read from the cache once it exists.

At 1:1 the view is exact: the same steps and settings as the committed
result, applied to the visible region plus padding.

### What is built

In `src/studio/`, with tests:

- the document, its stages and step records, saved as versioned JSON;
- region readers, steps as readers, the price of a read, tiling within a
  budget;
- chains for a pyramid level, with the list of steps a reduced view left
  out;
- strokes, painted masks and the heal brush;
- the junction's record and its states.
- the tile store, written from any region reader within the memory budget.

The document records layers, with their blend mode, opacity, color, mask and
steps.

The spike's canvas draws from the document in three passes, all on the GPU:

1. **Masks.** Each masked step's strokes are drawn into their own layer of
   one mask texture, at the surface's size. A stroke being drawn goes into a
   last layer and shows as an overlay.
2. **Layers.** Each tile is drawn once into a float image the size of the
   surface. The shader goes through the Combine stage's layers bottom
   first: stretch the layer's band, apply the layer's own curve, color it,
   and blend it onto the result so far by the layer's mode and opacity.
3. **Finish.** One full-surface pass applies the Finish steps in order,
   each limited by its mask, then the stroke overlay and the display
   transform. A heal reads the combined image at its source pixel and
   applies the earlier steps there.

With eight layers and two Finish steps at 3840 × 2160 the three passes take
0.8 to 1.1 ms of GPU time on the M4 Max, against 0.4 to 0.6 ms for the
earlier single pass. The limits are eight layers, eight Finish steps and
seven masks per frame.

Not drawn yet: Linear-stage steps, which are heavy steps for the tile path;
a layer whose source is an earlier step's result; a heal whose source is
off the surface; and the brightness match of a heal, which needs the engine
to read the image.

A tile store keeps an image on disk with its pyramid, and is a region reader
like any other; see
[investigation 5](#5-tile-format-2026-11-25-first-result).

The spike's canvas draws from a store. The renderer takes a tile source,
which gives the document's size, pyramid levels and band count, and any
tile of any level; the synthetic document and a store on disk are two
sources. Opened on a 12,000 × 8,000, eight-band store of 4.1 GB, the canvas
read a tile from disk in 6.6 ms, against 14 ms to compute a synthetic one.
Through the whole measurement script it had no late frame, no blank tile,
and no tile from a coarser level while panning at 1500 or 6000 pixels per
second. After a jump to an unloaded area the view was sharp within 81 ms.
That run was on a 1293 × 970 surface at 120 Hz, not the 3840 × 2160 surface
of investigation 1, so it shows the store keeping up, not a 4K frame cost.

FITS and XISF frames are sources too, read by region through Seiza:
`studio_frames::FrameSource`. The `studio-store` command writes a store
from a frame.

Not built: any Seiza step wrapped as a Studio step, the round-trip file
writer and reader, and undo history.

## Seiza does the work; Studio starts from PSF Guard's flow (2026-11-25)

The owner's direction, which governs everything above:

- **Seiza has the core operations, and Studio uses them.** Reading FITS and
  XISF, stretching, background, deconvolution, color calibration,
  composition and star handling are Seiza's. Studio does not copy or
  rewrite them.
- **Where a Seiza API does not fit, the API is fixed in Seiza,** on a
  branch, in the crate that owns the operation, by making it more flexible.
  Studio does not work around it with its own version.
- **A new document is PSF Guard's processing flow.** It starts as the steps
  PSF Guard's previews already apply, with the same results. What Studio
  adds is that every step is interactive, and that the final composition is
  far more adjustable.

### Where this stands

| Part | Whose code runs | State |
|---|---|---|
| Reading a FITS or XISF frame by region | Seiza's windowed reader, through `image_io::read_window` | Built: `studio_frames::FrameSource`, and the `studio-store` command |
| Stretch on the canvas | Seiza's sample domain and automatic midtones stretch; the canvas samples the resolved curve into a table and draws from the table | Built in the spike. The shader holds no stretch math |
| A layer's Curves, and Saturation, on the canvas | The spike's own shader code | Stand-ins. Seiza's planned curves and saturation replace them, as tables or as shared shader source |
| Heal, painted masks, blends | New in `studio`; Seiza has none | Candidates to move to Seiza once settled |
| Tile store and pyramid | New in `studio`; Seiza has no tiled store | PSF Guard's cache format, like its other caches |
| `image_steps` types | New here; drafted for Seiza and withdrawn | Moves to Seiza once settled |
| Display transform | Little CMS in the spike; Seiza has no color management | To decide: a Seiza crate, or PSF Guard's |
| Linear steps | None runs yet | Seiza's, wrapped as steps |
| The default document | The mapping in `studio::flow`, from a Color processing setup's settings | Built: see “The default document” |

### The default document (2026-11-25)

A new document is made from a Color processing setup, the settings PSF
Guard's color previews already use (`StackColorProcessing`).
`studio::document_from_color_setup` turns the settings into steps, and
`studio::color_setup_from_document` turns the steps back into settings.

| Setting | Stage | Step |
|---|---|---|
| A channel of the color card | Sources | Channel stack, one per channel |
| `background_extraction` | Linear | Background extraction |
| `input_processing` | Linear | Inputs follow their channel cards |
| `input_deconvolutions` | Linear | Deconvolution, one per channel |
| `input_rc_astro` | Linear | RC-Astro tools, one per channel |
| `emission` | Linear | Subtract continuum |
| `color_rc_astro` | Linear | RC-Astro tools on color |
| `color_calibration` | Linear | Color calibration |
| `input_stretches` | Stretch | Stretch, one per channel per stretch, in order |
| `narrowband` | Stretch | Narrowband balance and stars |
| `output_stretches` | Finish | Output stretch, in order |
| A channel of the color card | Combine | One layer per channel |

The rules the mapping keeps:

- **A step stores PSF Guard's settings unchanged.** The step's settings are
  the JSON PSF Guard saved. The settings read back from the document are
  equal to the settings it was made from, so the existing pipeline gives the
  same image and uses the same cache entries. Tests check this for the
  built-in default setup, in JSON and through `StackColorProcessing`.
- **A step that is off is left out of the settings,** which is how PSF Guard
  records a tool that is off.
- **A step Studio adds stays out of PSF Guard's settings.** A heal stroke or
  a Finish curve has no place there.
- **A setting this build does not know is kept,** in a step in the Export
  stage, and written back. A document opened and saved by an older Studio
  does not drop a newer PSF Guard's settings.
- **A channel the setup did not save takes the stretch the saved channels
  share,** when they all share one. NII gets the default stretch this way.
  A channel the setup saved and the card lacks keeps its stretch.
- **The order of steps does not depend on the order of keys in the file.**
  Channels sort by name. The spike caught this: its `serde_json` keeps key
  order and PSF Guard's sorts keys, and the two gave different step orders.

`studio_flow` is the typed side: it takes a `ProcessingSetupRecord` or a
`StackColorProcessing` and refuses a View setup and settings the color
pipeline would refuse. The spike's new document is this flow over the
default setup, with a Curves and a Saturation step added in Finish so that
stage has something to draw.

What this does not do yet:

- **No step runs.** The steps list PSF Guard's flow and hold its settings.
  The canvas still draws only the stretch, the layers and the Finish
  stand-ins. Running the flow means wrapping Seiza's operations as steps, or
  handing the settings to PSF Guard's color pipeline for a full render.
- **The order inside a stage is the pipeline's phase order, not a second
  source of truth.** The color pipeline runs its phases in a fixed order.
  Moving a step in Studio does not yet change what the pipeline does.
- **Narrowband is one step.** Its settings cover the palette, the balance,
  star separation and green reduction. Combine needs these as separate
  layers and mixes; see “Combine: what layering has to support.”
- **The layer colors are the spike's,** not the palette in the settings.
- **The spike's Add buttons make steps with no settings.** A Deconvolution
  step added that way has no channel, and reading the settings back refuses
  the document until the step has one.
- **Not tested against a setup a person saved.** This Mac has no
  `processing-setups.json`; the tests use the built-in default and settings
  written for the tests.

### Seiza changes this work has found so far

Each is a change to an existing crate.

1. `seiza-deconvolution`: take the sample range from the caller, and
   replace or drop the flux scale that needs the finished whole-image
   result, so a crop and a tile match the whole image.
2. `seiza-stretch`: give a resolved stretch as a sampled table, and map one
   value through a resolved sample domain. The spike gets both by calling
   `apply` on a ramp of values, which works and allocates for every table.
3. `seiza-stretch`: a stretch linked across the channels of a color image,
   resolved once and given per channel. The spike stretches each band on
   its own.

A real frame was the test: one 6248 × 4176 H-alpha sub of Sh2-157, read by
`FrameSource`, written to a store in 0.2 s, and shown on the canvas with
Seiza's stretch.

## Combine: what layering has to support (2026-11-25, for later)

The owner's direction for the Combine stage, recorded so the scaffolding
grows toward it. Nothing here is built. The spike's Combine is the simple
start: one layer per band, a fixed color each, six blend modes.

### What is wanted

- **Layers that compose as in Photoshop:** a stack with groups, where each
  layer has a mask, an opacity, a blend mode and its own adjustments.
- **Our own blend and mix modes** beside the general ones: the narrowband
  palettes, continuum subtraction, and luminance added to RGB by a variable
  amount, as in LRGB.
- **PSF Guard's star composition and Gaia correction passes** as part of the
  stack: a starless image and a stars image stretched separately and
  recombined, and star colors corrected from the catalog.
- **A stretch of its own for each layer.**
- **One band used more than once:** NII or He II as two layers, each mapped
  differently.
- **Bands mapped to color by rule:** Sloan filters by wavelength, as well as
  a color chosen by hand.
- **Masks on a layer,** for the imaging modes that need them.
- **Palettes that depend on the pixels,** as Foraxx does: the share of each
  band in a color changes with the bands' own values.

### What it changes in the model

1. **A layer is a chain, not a band with a color.** Source, linear steps, its
   own stretch, display adjustments, then how it enters the stack. The
   Stretch stage stops being one boundary for the whole document. Its pane
   edits each layer's stretch, with a default that new layers take.
2. **Layers and sources are separate.** A source is a band or the result of
   a step. Any number of layers may read one source, so NII can appear
   twice with two mappings. The document already lets a layer name a step's
   result; it needs the same for a band used twice.
3. **Some nodes take several inputs.** Continuum subtraction takes a line
   band and a continuum band. A dynamic palette takes every band it mixes.
   Star recombination takes a starless image and a stars image. A layer over
   the layer beneath cannot express these. The stack needs mix nodes whose
   inputs are named, as the design's graph of typed nodes already says; the
   fixed lists of steps in the scaffolding are a simplification of it.
4. **Combining is not only display-referred.** Continuum subtraction and
   star color correction work on linear samples. They belong to a linear
   part of Combine, before each layer's stretch, and the domain check must
   allow a layer's chain to cross from linear to display inside the stage.
5. **A color mapping is a rule with settings,** not three numbers: a fixed
   color, a wavelength, or a palette that reads other bands.
6. **Groups** carry a mask, an opacity and a blend mode for the layers in
   them, so a star group and a nebula group can be handled as two things.

### What it changes on the canvas

The spike's shader goes through at most eight layers, each one band with a
color, blended onto the layer beneath. That cannot draw a mix node or a
group. The likely form is a shader written from the document's graph: one
function per node, composed in order, compiled again only when the graph's
shape changes and not when a setting does. Settings stay in uniforms, so a
slider still costs nothing to move. Every band is already in each tile, so
a node may read any band at a pixel.

Heavy nodes stay off the canvas: star separation and catalog star colors
are tracked jobs whose results are stores, read as sources.

### Order to build, when the time comes

1. Per-layer stretch, and a band used by more than one layer.
2. Mix nodes on the CPU model, with continuum subtraction and LRGB with a
   luminance amount as the first two.
3. The shader written from the graph, with those two nodes and groups.
4. Dynamic palettes: Foraxx as the first, moved from the existing preview
   pipeline with its output held equal.
5. Star composition and the Gaia correction pass as sources and layers.
6. Wavelength mapping for Sloan and other filter sets.

Each step that moves an existing pass must keep its output and cache keys,
as [pipeline parity](#what-to-investigate-first) requires.

## Suggested order of work

1. The investigations listed in the previous section.
2. Engine crate with documents, tiles and the command layer, used by the
   existing web UI. Output parity tests; no visible change.
3. Automatic previews and saved setups on the engine.
4. Studio: canvas, layers, live light steps, step view, history.
5. Bands and palette presets; imported masters; documents without a catalog.
6. Masks: computed first, then painted masks and manual tweaks.
7. Launch from PSF Guard, sign-in, save back; bundling in the installer.
8. MCP tools in PSF Guard and in Studio.
9. Pre-stack settings on the source node.
10. PSB export.

## Open questions

- Which heavy steps may Studio ask a server to run, given that remote use is
  otherwise limited to serving images?
- How does a saved processing setup map to a document template, so setups
  from earlier releases keep their results and cache keys?
- How are Studio releases versioned and updated next to PSF Guard, when both
  ship in one installer?
- What is the smallest useful set of adjustments for the first Studio
  release?
- Are clone and heal tools wanted among the manual tweaks? They can be stored
  as strokes, but they add a second kind of painted edit.

## Sources checked

- [gpui_interop on docs.rs](https://docs.rs/gpui_interop/0.2.0)
- [longbridge/gpui-component](https://github.com/longbridge/gpui-component)
- [ag-psd (Rust port) on docs.rs](https://docs.rs/crate/ag-psd/0.1.0)
- [PhotoshopAPI](https://github.com/EmilDohne/PhotoshopAPI)
