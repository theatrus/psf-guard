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

#### Follow-up: PSF Guard records Studio documents (asked for 2026-11-25)

The owner asked for this as its own piece of work: PSF Guard records the
documents Studio makes for a project, and a person opens Studio from PSF
Guard with one click. What that needs, on top of the steps listed in
“Launching Studio from PSF Guard”:

- **A record per document in PSF Guard,** below the database slug like
  other derived data: the document, the color card it started from
  (project, target, kind, palette, crop and star settings), and the build
  it last ran. `studio-run` takes the card as options today because the
  document has no place for it.
- **A list of a project's Studio documents** on the project's stack page,
  each with **Open in PSF-Studio**.
- **Save from Studio** writes the document back to that record. A document
  whose steps are all PSF Guard's can also be saved as a Color processing
  setup, since it reads back as one.
- **One click on the desktop app** starts the bundled Studio with the
  record. In a browser the link and the link file do the same.

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
[`studio`](../../studio/README.md).

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
[`studio`](../../studio/README.md), with
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
[`studio`](../../studio/README.md). Studio has since fixed its working space
to Rec. 2020 and dropped `--working`; see **Working space** under the
Finish stage.

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
| `input_rc_astro` | Linear | One tool step per tool of each channel's chain: Sharpening, Noise reduction, Star removal |
| `emission` | Linear | Subtract continuum |
| `color_rc_astro` | Linear | One tool step per tool, with the band of the color group it runs on (`rgb`, or `lines` without red, green and blue) |
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

- **Only the first steps run.** See “Studio runs the flow; PSF Guard
  shares the steps.” The canvas still draws only the stretch, the layers
  and the Finish stand-ins.
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

### Studio runs the flow; PSF Guard shares the steps (2026-11-25)

Two directions from the owner settle how the flow runs:

- **Studio does all the color processing. It starts from PSF Guard's
  channel stacks.** PSF Guard stacks each channel; Studio fetches the
  stacks and does everything after. A first version sent the document's
  settings to the server as a color preview request and downloaded the
  result. That was dropped: it left Studio showing a finished picture with
  nothing to adjust.
- **The steps are one piece of code that PSF Guard's color previews also
  call.** A step holds the order of the Seiza calls and the rules around
  them, and knows nothing of jobs, caches or progress bars. The caller
  passes a function that hears what the step is doing and may stop it.
  For now only Studio gives interactive feedback; PSF Guard runs the same
  steps inside its jobs.

- **The existing PSF Guard flow does not change.** A routine is
  generalized so Studio can call it interactively; what PSF Guard's color
  previews do with it stays the same: the same results, progress lines and
  cache keys. Each move is checked with the existing tests and against a
  job PSF Guard recorded before the move.
- **Later, PSF Guard renders a Studio document with the same engine,**
  with no interface: the document's steps run headless in a job, so what a
  person adjusted in Studio can be reprocessed when the stacks change.

What is built:

| Piece | Where | State |
|---|---|---|
| List and download a target's channel stacks | `studio_sources::Server` | Built. One current stack per channel; a channel with several is refused until a person can choose |
| Background step: fit each channel in its own frame, choose one model for all, refit, correct | `color_steps::correct_channel_backgrounds` | Built, and `compose_color` now calls it. PSF Guard's progress lines, stop check and recorded fits come from the events |
| Register channels and place them on the reference grid | `color_steps::register_channels`, `place_on_reference` | Built, and `compose_color` calls `register_channels`. Its progress lines, stop check, log line and recorded transforms come from the events. `compose_color` still resamples with its own call |
| Prepare a document's channels | `studio_sources::RegisteredChannels::prepare` | Calls the shared steps in PSF Guard's order |
| The channels as one linear image, read by region | `RegisteredChannels` as a `RegionReader` | Built. Channels stay as planes and a region is interleaved when read |
| `studio-sources` command | `commands::studio_sources` | Fetches, prepares, writes a tile store and the document |

The document decides what runs: `studio-sources` reads the background
settings from the document's Background extraction step, and a step turned
off does not run.

Checked on real data, 45 local Sh2-157 frames (15 each of R, G and B),
stacked by a test server on a scratch copy of the database:

- Registration gave the numbers the server's own color job logged: green
  0.054 px RMS over 1766 stars, blue 0.080 px over 1656.
- The three background fits are equal, field for field, to the fits the
  server's color job recorded: model, coefficients, reference level and
  every sample.
- The whole run took 1.5 s and peaked at 574 MB for a 6248 × 4176 image
  of three channels.
- The store opens on the canvas with the default flow: stars are white and
  the background is neutral.

#### Live background strength

A person moves the strength of Background extraction and the canvas
follows at once, with no refit:

- Preparation keeps two planes per channel on the reference grid: the
  channel before the step, and corrected in full. The tile store names the
  corrected plane after the step, such as `red@step-<id>` (see “Each tool
  is a step”).
- Seiza's correction changes each sample in proportion to the strength,
  in Subtract and in Divide, and resampling keeps proportions. So the
  channel at any strength is a blend of its two planes. A test compares
  the blend with correcting at that strength and placing the result:
  they agree to 2e-6 on samples near 0.2.
- The canvas blends the two planes in the layer shader, before the
  stretch. The slider writes `strength` into the step's settings, which
  are PSF Guard's, so the document still reads back as a setup.
- The stretch is analysed again from the blended samples when the
  strength changes, because the stretch is applied to corrected data.

On the Sh2-157 stacks the gradient is plain at strength 0, weaker at 0.5
and gone at 1. Moving the slider reached the GPU in about 1 ms. The store
is twice the size, 6 channels for 3, and preparation peaked at 811 MB.

The cost is memory and disk, and it covers one setting. A step whose
result is not in proportion to its setting cannot be made live this way.

#### What the view shows, and how a person picks it (2026-11-25)

This part is written so the interface can move to another toolkit, a web
canvas included, without being designed again. The rules live in
`studio::view`, which knows of no toolkit and no GPU; the GPUI panes only
draw buttons and hand the result to the canvas.

A person picks three things. Each is a row of buttons under the stages,
and a sentence under them says what is on screen, such as “green on its
own, after the Linear stage, drawn with the view stretch, not saved,
because the view stops before the stretch.”

| Choice | Options | What it means |
|---|---|---|
| Show | One tab per channel, and Combined | A channel is drawn on its own, in gray. Combined blends the layers as the Combine stage does |
| Up to | Start of stage, Selected step, End of stage, Final | The last step the view applies. This is how a person sees an intermediate result. Changing stage moves it to the end of that stage |
| Stretch | Document's, Preview | The document's stretch is each channel's own Stretch steps. The view stretch is for looking only and is never saved or exported. A channel the view has not stretched yet is drawn with the view stretch, and the sentence says so |

`studio::plan(document, view)` returns what a canvas draws: for each
channel the background step that applies and the stretch to use, whether
the layers are blended, which Finish steps apply, and the sentence. A
channel view applies no Finish step, because those act on the combined
image.

Channels are not assumed to share settings:

- **A step with a channel applies to that channel. A step without one
  applies to every channel that has no step of its own of that kind.**
- **A channel's own step decides for that channel, on or off.** Turning
  it off turns the step off for that channel; the channel does not fall
  back to the shared step.
- **Give a channel its own settings** copies a shared step for the viewed
  channel, right after the shared one. The shared step's row then says
  “every channel, except this one: it has its own.”
- **Stretch is per channel from the start.** Each channel's slider writes
  that channel's own Stretch step.
- **The step list follows the view.** A channel tab lists that channel's
  steps and the shared ones; Combined lists all.
- **A channel's own Background step is Studio's.** PSF Guard's setup holds
  one background for every channel, so the settings read back from the
  document keep the shared step and leave the channel's own out.

Tried on the Sh2-157 stacks: green on its own in the Linear stage, given
its own Background step and moved to strength 0. The combined view then
showed green's gradient with red and blue still corrected.

What the canvas needs from a toolkit, for whoever ports it:

1. A surface the canvas renders into, behind the interface, with pointer
   events reaching the interface first.
2. Per frame: the view transform, a list of layers (tile channel, color,
   opacity, blend mode, curve, an optional second tile channel with a
   blend amount), one stretch table per layer, the Finish steps and the
   mask strokes. `Frame` in the spike's render crate is that list.
3. Tiles as array textures with one layer per stored channel, fetched by
   level and position.

Everything above the frame is `studio::plan` and the document, which are
plain Rust and compile to WebAssembly.

Not done in the view:

- **A step's result in the middle of a channel's chain** is shown only for
  the steps the canvas can apply: the background strength, the stretch
  and the Finish steps. Other steps are recorded and not drawn.
- **The view stretch** is set with **View stretch** in the View toolbar:
  any of Seiza's stretches but the layered ones, Automatic midtones to
  start. Studio keeps it while it runs; the document does not, and an
  export leaves a channel nothing stretches linear. It is used only where
  nothing stretches a channel, never in place of a document's stretch. It
  cannot be linked across channels.
- **Brushes are offered in the Finish stage only,** where the canvas draws
  masks and heals. They were shown in every stage and did nothing there.
- **A channel's own Background step changes its strength only.** The fit
  is still made with the shared settings.

#### PSF Guard's flow after the moves

The existing flow must not change. After both moves, a color preview
built fresh by the refactored server gave the same registration
transforms and the same background fits, field for field, as the job the
server recorded before any code moved. The 159 color and Studio tests
pass.

The order to move the rest of `compose_color` into `color_steps`, each
with the same check against a recorded job: resampling; deconvolution and RC-Astro tools on the channels; continuum subtraction;
color calibration; stretch; narrowband balance and stars; composition;
output stretch. `color_steps` still takes its settings types from
`server::stack_preview::color`; they move with the steps.

Not done:

- **Catalog protection.** PSF Guard keeps solved catalog emission out of
  the background samples. Studio reads no plate solve yet, so it protects
  nothing. The test stacks had no solve, so the fits still matched.
- **Interactive background beyond strength.** The strength is live; see
  “Live background strength.” A change to the model, the samples or the
  mode still reruns the preparation, which takes 0.3 s for these frames.
  Correcting only the tiles in view needs theatrus/seiza#509, which is
  open and not used here yet. It applies when the fit and the view share a
  grid: the reference channel, a one-shot color image, a mosaic.
- **Memory for a mosaic.** Every stack is held while the channels are
  prepared. Seiza registers and resamples whole images.
- **The stretch on the canvas** is still the spike's Auto-MTF at its own
  target, not the stretch in the document's steps.

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
4. `seiza-background`: correct one rectangle of a fitted image, with the
   values the whole-image correction gives. Open as theatrus/seiza#509,
   reviewed, with the C export and the Python method; PSF Guard does not
   depend on it yet.

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

### Inspecting layers and adjusting them in place (asked for 2026-11-25)

In the Combine view a person must be able to:

- **Inspect a layer:** see it alone, see it with the layers under it, and
  see its mask.
- **Edit a layer:** its color mapping, opacity, blend mode, stretch and
  mask, as now, with the result on the canvas at once.
- **Add an adjustment layer at any place in the stack:** exposure,
  clarity, curves, saturation and the like. It changes what is under it
  and carries its own mask, painted or computed, so an adjustment applies
  to one region.

In the model an adjustment layer is a layer with no pixels of its own: an
operation, its settings, a mask, an opacity and a blend. The view model
gains a subject for one layer and for the stack up to a layer, beside one
channel and Combined. Finish steps become adjustment layers at the top of
the stack, so there is one way to adjust and not two.

On the canvas the layer pass becomes a loop over pixel layers and
adjustment layers in order, each adjustment reading the image so far and
a mask layer. The spike's Finish pass already does this for curves,
saturation and heal with painted masks; it moves into the layer loop.
Clarity needs a blurred copy of the image so far, which is one more
target at a reduced size.

#### What is built of this (2026-11-25)

- **Model.** `LayerSource::Adjustment { operation, settings }` is a layer
  with no pixels. Its mask and opacity limit where and how strongly it
  applies. `Document::paint_layer_mask` adds a stroke to a layer's mask.
  Adjustment layers are Studio's and stay out of PSF Guard's settings.
- **View.** Two more subjects beside a channel and Combined: one layer on
  its own, and the layers up to a layer. An adjustment layer alone has
  nothing to show, so it is shown over the layers beneath it.
  `ViewPlan::stack` lists the layers a blended view draws, bottom first.
- **Canvas.** The layer pass is one loop over the stack, per pixel. A band
  layer is stretched, colored and blended; an adjustment layer changes the
  color so far. Both are limited by opacity and by a mask read at the
  pixel. Exposure, curves and saturation are drawn. The masks are bound to
  the layer pass as a third group, and the mask pass runs first.
- **Panes.** In Combine: add Exposure, Curves or Saturation above the
  selected layer; an amount slider; **This layer**, **Up to this layer**
  and **All layers** to pick what the canvas shows; the mask brush paints
  the selected layer's mask; **Remove mask** and **Remove this
  adjustment**.

Tried on the Sh2-157 stacks: an Exposure layer above the three channels,
a painted stroke as its mask, and the amount at +1.96 stops. Only the
stroke brightened.

Not built: clarity, which needs a blurred copy of the image so far;
computed masks on layers; moving a layer by dragging; Finish steps as
adjustment layers, so there are still two ways to adjust; heal inside the
stack. The exposure, curves and saturation math is the spike's own shader
code, standing in for Seiza's as before. A frame draws at most 12 layers
and 7 masks.

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

## One tree, palettes and split panes (2026-11-25)

The first panes had five separate choices that each changed the canvas: a
stage rail, a selected step, and three button groups for what to show, up
to where and with which stretch. Layers were the inputs themselves, named
by how many bands a store had, so a narrowband store was labelled R, G, B.
This section records what replaced that. The rules live in
`src/studio/place.rs` and `src/studio/palette.rs`, which know of no
toolkit, so a web canvas would lay out the same tree and draw the same
panes.

Photoshop's and Affinity's layer panels are the guide where they agree:
the top layer is listed first, each row has its own switch, an edit that
belongs to one layer is indented under it, and rows move by dragging.

### Inputs and layers are different things

- An **input** is a channel stack, named as the store names it: `ha`,
  `oiii`, `sii`, `luminance`, `red`, `green`, `blue`. A person reads them
  as Ha, OIII, SII, L, R, G, B. A store that records no names gets them by
  its band count.
- A **layer** shows one input in one color with a blend mode. Several
  layers may show the same input: Ha as red and Ha as luminance, or RGB
  stars over a narrowband picture.
- A **palette** is a starting set of layers for the inputs a document has:
  RGB, LRGB, SHO, HSO, HOO, and two tonal variants (SHO in gold and teal,
  HOO in red and teal). A new document gets the palette that uses the most
  of its inputs. Choosing another palette replaces the layers that show an
  input, keeps each one's mask and own edits where the input matches, and
  keeps every other layer above them.
- A layer's color is **any hue and saturation**, not only pure red, green
  or blue. The color's brightness is the layer's share of the sum: a
  palette scales its layers so their sum is full scale in its brightest
  component, and “Balance the layers' colors” does that again after a
  person changes hues.
- `BlendMode::Luminance` takes the layer's brightness and keeps the color
  beneath it, which is how the L of LRGB goes on.

### Palettes that follow the signal

Some narrowband palettes give a color to an input only where another
input is strong. Foraxx is the common one. Its SHO form is:

- red is SII where OIII is strong, and Ha elsewhere;
- green is Ha where both Ha and OIII are strong, and OIII elsewhere;
- blue is OIII.

"Strong" is the power of inverted pixels of the stretched samples: a
value `x` from 0 to 1 becomes `x^(1 - x)`, which lifts faint signal.

Studio builds this from layers and **shared masks**. A shared mask
(`SharedMask`) belongs to the document, not to one layer. At each pixel
it is:

1. the product of one or two inputs' stretched samples;
2. shaped by `MaskShape`, and raised to the mask's **reach**;
3. **smoothed** by a Gaussian blur, so the inputs' noise does not become
   speckle in the layers behind the mask;
4. **touched up** by strokes that set it to 1 or to 0.

A layer is behind a shared mask or its inverse (`MaskRecord::Shared`). A
Foraxx palette is then five ordinary layers and two masks: SII as red
behind the OIII mask, Ha as red behind its inverse, and so on. Because the
two layers use one mask, a change to the mask keeps them consistent: their
shares of red still sum to the whole. Two layers behind a mask and its
inverse keep their full color; “Balance the layers' colors” leaves them
out.

- **SHO, Foraxx** and **HOO, Foraxx** are offered with the other palettes.
  A plain palette stays the default.
- **Masks are remade from the palette's inputs.** A palette has slots,
  named after the inputs they show to start with. The document records
  which input each slot shows (`Document::palette`). Giving a slot another
  input (`Document::set_palette_input`) remakes the layers and the masks
  that use the slot. Each layer keeps its color, opacity, own edits and
  place; each mask keeps its reach, smoothing and touch-ups.
- **A mask follows its inputs without being remade.** It reads each input
  with the stretch the view gives that input, at every frame.
- **Tweaking a mask by hand:** a mask has its own row in the tree, under
  Masks. Its controls set the inputs, the shape, the reach and the
  smoothing. The mask brush adds to the mask or takes away from it. A mask
  a person gives other inputs is theirs: a palette no longer remakes it.
- **Seeing the result:** with a mask selected the canvas shows the mask
  in gray, or the final picture. The Input and picture split shows both.
- Any layer, group or adjustment layer can be put behind a shared mask in
  its controls, under **Shared mask**.
- Choosing another palette removes the masks the old palette made and
  keeps painted masks.

**Smoothing is a difference from Seiza as released.** Seiza's Foraxx
composition, which PSF Guard's Foraxx palettes use, computes the mix at
each pixel with no smoothing, so OIII noise shows as speckle in blue and
green. A new shared mask is smoothed by 4 pixels.

Seiza gets the same step in theatrus/seiza#519, not yet merged:
`ForaxxOptions::mask_smoothing`, a Gaussian blur of the two blend factors
in pixels, 0 by default so existing output is unchanged. Two things differ
from the canvas and must be settled before PSF Guard renders a Studio
document with it: Seiza's kernel reaches four standard deviations and the
canvas samples three; and Seiza blurs at full resolution, where the canvas
blurs at the zoom on screen.

These palettes are meant for stretched, starless inputs. With stars in
the inputs, the stars take part in the masks.

### Color tones

A palette is a starting point. The Combined row's controls list every
layer that shows an input with its hue and saturation, so the tone of each
can be set in one place. The same two sliders are in each layer's own
controls.

### What is being edited is always shown

A line under the top bar states what is selected, what it applies to, and
what the tool in hand will change: “the mask OIII signal, shared by 2
layers · The mask brush takes away from this mask.” While a mask is
selected, or the mask brush is in hand, the line is marked in amber and
reads “Editing a mask”, because a stroke then changes where layers show
and not the picture itself.

### One tree

The left pane is one tree, in the order the picture is made:

1. **Inputs.** Each input, with the steps that apply to it indented under
   it: its own steps, and the shared steps it has no own step in place of.
   A shared step's row reads “every input”; an own step's reads “Ha only.”
   An input's steps are folded away until a person opens them or selects
   one.
2. **Combine.** The layers, top first. An edit that belongs to one layer
   (an adjustment or a heal) is indented under that layer and reads
   “Ha as gold only.” An adjustment layer sits between layers and reads
   “every layer beneath.” Then a row for the combined picture.
3. **Finish.** Steps on the whole picture, then the final picture.

A person selects a `Row`. The right pane shows that row's controls, and
starts with the row's name and one line that says what the row applies to.

### An adjustment applies to one layer or to the stack

- **One layer only:** the adjustment is a step of that layer
  (`LayerRecord::steps`). It changes the layer before the layer is blended,
  and moves with the layer.
- **Every layer beneath:** the adjustment is a layer of the stack
  (`LayerSource::Adjustment`). It changes the blend of the layers under it.

`Document::attach_adjustment` and `detach_adjustment` turn one into the
other, keeping the settings, the mask and the opacity. A step has an
opacity of its own for this. Dragging does the same: an
adjustment dropped on a layer becomes that layer's own; dropped between
layers, it changes every layer beneath. Any layer dropped between layers
moves there.

A heal belongs to where the selection is: the selected input, the selected
layer, or the whole picture. A heal of an input changes the input's samples
before its stretch, wherever that input is shown. A heal of a layer changes
the stretched layer before the layer's adjustments. On the canvas a layer
with heals of its own is first drawn into a surface-sized image of its
samples, so a heal can copy them from another place.

### Groups and clipping

A **group** is a layer with no pixels of its own (`LayerSource::Group`).
Its members are the layers directly beneath it that name it in
`LayerRecord::group`. The members blend together on their own, starting
from nothing; the result is then treated as one layer, with the group's
opacity, blend mode, mask and own adjustments. An adjustment layer inside a
group changes the members beneath it and nothing outside the group.

- **Put in a new group** wraps one layer. The new group takes that layer's
  blend mode, so grouping one layer does not change the picture.
- A layer joins a group when it is dragged onto the group's row, or into a
  gap inside the group. It leaves when it is dragged to a gap outside.
  `Document::place_layer` does both, and moves a group with its members.
- **Ungroup** keeps the members where they are. The group's own mask and
  adjustments go with the group.
- The document keeps a group's members together after every change to the
  stack. A group holds no group.

The stack stays one flat list, bottom first, so finding, masking and
adjusting a layer work the same inside a group as outside. The view plan
marks where a group's members start and end (`StackEntry::GroupBegin`,
`GroupEnd`). A view that stops inside a group shows the group as far as it
goes; one member alone is shown as it is, not as its group blends it.

A palette's layers read top down in the palette's order, as the rest of
the flow does: R, G, B and Ha, OIII from the top. The palette lists them
bottom first, so each run of added layers is stacked in reverse; adding is
the same in any order, and a layer of another blend, such as luminance,
keeps its place on top. A document saved before stacks them the other
way; Studio turns such a run over as it opens the document
(`Document::list_palette_layers_top_down`) when the run is still in the
palette's order, all added, none clipped and with no clipped layer above.

A layer may be **clipped to the layer beneath it** (`LayerRecord::clipped`).
It then shows only inside that layer's mask, and not at all while that
layer is hidden. The layer beneath is the nearest one in the same group
that is not itself clipped and is not an adjustment; for a layer above a
group, that is the group. Every layer here covers the whole frame, so
clipping to a layer with no mask changes nothing.

### A pane shows a place

A canvas pane shows a `Place`: the result up to and including a row. A
pane follows the selected row unless it is pinned, so there is one choice
to make, not one for what to edit and another for what to look at.

- An input row shows the input as stacked; a step row shows the input
  after that step.
- A layer row shows the stack up to that layer, or that layer alone.
  **Show: Stack | Only**, first in the layer's inspector, chooses which.
  A layer of one input shown alone is drawn in gray at full strength,
  without its color, opacity or blend mode, so its tones can be judged as
  the input's own. A group shown alone keeps its colors, as its members
  blend together (`ViewPlan::gray`).
- A row for one layer's own edit shows that layer alone, with its own
  edits up to and including that one (`Place::LayerEdit`). The row before
  it is the same layer without the edit, so a before and after of one edit
  compares like with like.
- A place before an input's stretch is drawn with the view stretch, and
  the pane's label says so: “Ha · as stacked · view stretch, not saved.”
  There is no separate stretch choice.

`Place::view` gives the `View` that `plan` already turned into what a
canvas draws, so the earlier
[view rules](#what-the-view-shows-and-how-a-person-picks-it-2026-11-25)
still decide which steps and stretches apply. Their three button groups
are gone.

### Split panes

`Split` divides the canvas, and `panes` chooses each pane's place from the
selection:

| Split | Panes |
|---|---|
| With final (the default) | The selected row, and the final picture. |
| Before, after, final | The row before the selected one, the selected row, and the final picture. |
| Wipe | Before and after in one pane, with a divider a person drags: before on its left, after on its right. |
| Single | The selected row. |

Every split but Single ends with the final picture, so an edit's effect on
the result is always in view; the panes before it follow the click. A pane
is not repeated: with the final picture selected, With final is one pane.
Pinning a pane keeps it on a place of the person's choice, such as a stage
to compare against. “Input and picture” and “All inputs” are gone: the
first is With final with an input selected, and the second showed panes
that did not follow the selection.

The panes are one size and share one pan and zoom: up to three in a row,
more in two rows. Each pane has a label in its corner and a Pin button that
keeps its place while the selection moves. The label also lists what the
pane leaves out when one frame has no room for more.

On the canvas, the panes share the uniform buffer, the stretch table and
the mask textures, so the renderer writes and submits each pane in turn
with a viewport and a scissor. The two are the same rectangle for a pane of
its own; for a wipe both scenes are laid out over the whole canvas and each
is cut at the divider. Each pane keeps its own stretch
analyses, because a pane before the background step and a pane after it
stretch different samples.

### The mask overlay

The selected row's mask, or a heal's stroke, can be drawn over the image
in red. **Mask overlay** in the Tools block switches it on and off;
picking the mask brush switches it on. Hiding the overlay does not change
the mask. A stroke shows while it is being drawn either way.

### What is built, and what is not

Built in the spike and tried on the Sh2-157 stacks, as RGB and with the
same stacks renamed SII, Ha and OIII: named inputs, the palettes, hue and
saturation per layer, layers that share an input, adjustments for one
layer and for the stack with painted masks and opacity, dragging between
the two, heals of one input, of one layer and of the whole picture, groups
with their own adjustments, clipping, the tree, the row controls, the five
splits, pinning, the mask overlay, shared masks with smoothing and
touch-ups, the two Foraxx palettes, palette input mapping and the list of
color tones. The palettes and masks were tried on real Ha, OIII and SII
stacks of Sh2-157.

One frame draws at most 24 layers, counting each adjustment and both ends
of each group, 15 masks,
16 heals of single layers on at most 4 layers, 8 shared masks, and 8
Finish steps. What
does not fit is listed in the pane's label and is not drawn; nothing is
drawn without its mask. The engine has no such limits, because it renders
a layer at a time.

Not built:

- Groups inside groups, and a group that passes its members through to
  the layers beneath one by one, as Photoshop's default group does.
- Clipping a group to the layer beneath it, and a heal of a group.
- A shared mask on one layer's own adjustment; a layer with both a
  painted and a shared mask; clipping to a layer's shared mask.
- A wide smoothing is drawn from 33 samples in each direction, so at the
  widest settings it is an approximation of a Gaussian.
- A heal copies samples as they were before any heal of the same layer, so
  one heal cannot copy what another healed.
- The adjustment math and the heal are still the spike's shader code,
  standing in for Seiza's. Seiza has no exposure, curves or saturation
  step for display samples yet.

## Stretches in Studio (2026-11-25)

An input's stretch is its Stretch steps, in order. Each step holds one of
Seiza's stretches with its settings, in the JSON Seiza reads
(`seiza_stretch::StretchModel`): automatic midtones, median anchor,
generalized hyperbolic (GHS), linear, asinh, asinh by percentiles, midtones
and none. An input may have several, such as an automatic stretch followed
by two GHS steps.

- **The canvas resolves the whole chain through Seiza.** Each stage is
  resolved from the input's samples as the stages before it left them,
  which is what Seiza's stretch stack does, and the chain becomes one
  lookup table per input. Nothing here reimplements a stretch.
- **A Stretch step's controls** choose the stretch and set its settings
  with sliders. “Add another stretch after this one” adds a GHS step for
  the same input.
- **Views.** A row for a Stretch step shows the input after that step and
  the stretches before it, so Before and after compares one stage with
  the next. When an input has several, a pane's label numbers them:
  “Ha · after Stretch 2.”
- A shared mask reads each input through its whole chain.

Not built: the layered stretch, which needs the image and not samples of
it, so only the engine can draw it; a row says so. A GHS slider covers
stretch factors up to 10, where Seiza allows 20.

### Output stretches (2026-11-26)

PSF Guard's output stretches stretch the combined picture, after the
layers are blended. Studio draws them as Finish steps, in their place among
the other Finish steps. **Add step › Output stretch** adds one, a
generalized hyperbolic stretch to start with, and its controls are those of
an input's Stretch step with one more choice, **Channels**:

- **Linked:** one curve for red, green and blue.
- **Each channel:** a curve resolved for each channel, which neutralizes a
  colored background.
- **Luminance:** one curve for the Rec. 709 luminance; each pixel is scaled
  by one factor, kept in gamut, so its color holds.

These are Seiza's three color strategies. Seiza resolves each stretch; the
canvas draws the result as three rows of the stretch table, and the Finish
pass applies them. The luminance strategy's per-pixel scaling is the only
part the shader does itself, as Seiza's `map_luminance_pixel` does.

A stretch with explicit settings (generalized hyperbolic, linear, asinh,
midtones, PixInsight's histogram transformation, or none) is resolved
without the picture. One that Seiza resolves from the picture's statistics
(automatic midtones, median anchor, asinh by percentiles) needs samples of
the combined picture. The render thread draws an eight-by-eight grid of
128-pixel patches of the combined picture at full resolution into
offscreen targets, up to eight patches per frame as their tiles load, and
reads each frame's patches back with one wait for the GPU. Full resolution matters for the same reason as for an input: a
reduced level's samples are means, and a stretch resolved from them would
clip the shadows in the wrong place. Each output stretch after the first is
resolved from the samples as the stretches before it left them, as Seiza's
stretch stack does.

The picture is sampled again when anything that changes it does: the
store, a layer, a stretch, a mask, a heal, an input's state or the crop.
A refresh that changes no output stretch and no samples reuses the
stretches already resolved, and a sampling no longer needed stops. Until the
new samples are read, the old ones stand in, so the stretch keeps its place
while the picture changes. The first time, the picture shows without the
stretch until the samples arrive.

Limits:

- The samples are of the combined picture before the Finish steps. Curves,
  Saturation and heals placed before an output stretch are left out of
  what that stretch is resolved from.
- A smoothed mask in a patch is blurred within the patch, so near a
  patch's edge it can differ slightly from the canvas. A heal whose source
  lies outside its patch copies nothing there.
- The layered stretch needs the whole image, so the canvas does not draw it
  as an output stretch either.

### Final adjustments, histogram and 16-bit export (2026-11-26)

Asked for: final adjustments of the picture such as a stretch, the tone
and color controls Adobe Camera Raw has around its histogram, and saving
the final picture as FITS or TIFF without leaving 16-bit precision.

**Tone.** A Finish step, **Tone and color**, has Camera Raw's Basic
controls, in its order:

| Control | Range | What it does |
|---|---|---|
| Exposure | −3 to +3 stops | Multiplies the picture by 2^exposure. |
| Contrast | −1 to +1 | An S-curve around the middle, or a flattening one. |
| Highlights | −1 to +1 | Raises or lowers the bright areas' tones, most at two thirds of the way up, keeping the detail in them. |
| Shadows | −1 to +1 | Raises or lowers the dark areas' tones, most at a third of the way up, keeping the detail in them. |
| Whites | −1 to +1 | Moves the white point: up clips more of the brightest tones to white. |
| Blacks | −1 to +1 | Moves the black point: down clips more of the darkest tones to black. |
| Texture | −1 to +1 | Raises or lowers fine detail, over a few pixels. |
| Clarity | −1 to +1 | Raises or lowers the contrast of larger structures, most in the midtones. |
| Dehaze | −1 to +1 | Takes away a gray veil such as sky glow, keeping colored nebulae; negative adds one. |
| Vibrance | −1 to +1 | Saturation that changes muted colors more than vivid ones. |
| Saturation | −1 to +1 | Saturation of every color alike. |

Seiza's `seiza_compose::tone::Tone` does the work, on the canvas and in an
export. Each pixel's luminance changes with its color kept, scaled by one
factor and kept in gamut. Highlights, Shadows, Texture, Clarity and Dehaze
read the pixel's neighborhood, 97 pixels across for the area and 7 for
fine detail, through an edge-preserving base, so a star gets no halo; see
Seiza's compositor design record. On the canvas the windows are measured in
screen pixels, so zoomed out they cover the same part of the picture at the
canvas's resolution.

**HDR.** HDR belongs to the whole picture: the View toolbar and the Final
picture's inspector have an HDR switch and a Headroom, 0 to 4 stops (2 to
start), and the inspector a Knee, which the document keeps. With HDR on, every stage may be brighter than
SDR white up to the headroom: the stretches, the combined picture and every
Finish step, and every view of the picture shows it so (Seiza's
`Picture::headroom`). HDR is a mode of the picture, not of one stretch:
every input's stretch follows it. Layered, HDR puts the brightest structure
there with its core compression; plain Layered becomes Layered, HDR
without core compression; any other stretch continues above the picture's
knee, three quarters of its output unless the Knee moves it, along Seiza's
`HdrTail`, up to the peak at the band's brightest sample. That brightest
sample is the scale the canvas draws the band with (bands stretched
together share theirs). It is usually far above the 1 of the stretch's
range, which a percentile sets: a tail that ended at that 1 put every
brighter pixel at the peak, flat. On the Iris with automatic midtones,
55 to 86% of a 256-pixel square of the core, by channel, sat at the peak
that way, with no spread between its median and its 99th percentile; with
the tail reaching to the brightest sample, none does. (An earlier note here
gave a quarter to two fifths: that run's switch to automatic midtones had
not taken, and the canvas drew the inputs unstretched.) A lower knee gives the core
more of the headroom: from a knee of 0.75 to 0.35, the core's spread from
its median to its 99th percentile grew from 0.57 to 0.66. While HDR is on,
each Stretch step but Layered, HDR shows the Knee too. A stretch after an HDR one shapes the range up to white and
keeps what is above it (`TransferStage::BelowWhite`). Tone and color keeps
the headroom, and Exposure and Whites put more there; the **Clipping**
toggle in the View toolbar marks what reaches the peak. A document from when HDR was a switch of the Tone and color step
opens with that step's headroom as the picture's.

Above SDR white a value continues on the extended sRGB curve that macOS's
extended color spaces use, so two stops of headroom peaks at the value
whose light is four times SDR white's, about 1.82. Seiza's `Picture::peak`
gives the picture's peak. The canvas, the histogram and every export clip
values at the peak, so an SDR picture never shows above white, whatever
its blends or Saturation give. On an EDR
surface (`--output p3-extended`, as on a Pro Display XDR) the canvas shows
values above 1 brighter than white.
Export to a name ending `.avif` writes an HDR AVIF: the picture in linear
Rec. 2020 through the working space's ICC profile, PQ, 10 bits, with its
content light levels. A 32-bit float TIFF keeps the values above 1 as
they are. The histogram of an HDR picture counts clipping at the peak and
gives the share above SDR white apart. Tried on the Cat's Eye (NGC 6543)
from the C925: with Exposure +1.5 and a 2-stop headroom, the AVIF's
brightest light is 808 cd/m², within the 812 cd/m² the headroom allows,
and the float TIFF's brightest value is 1.825.

**Working space.** Studio composes every picture in Rec. 2020's primaries
on the sRGB curve (Seiza's `WorkingSpace::Rec2020`), so no color a display
can show is lost before the picture reaches it: Saturation, Vibrance and
narrowband hues may go past sRGB and Display P3. A layer's color is given
in Display P3, the gamut of the displays Studio is used on, and converted
into the working space by the matrix alone: a layer's color multiplies its
band's stretched values and the layers add, so layers whose colors add up
to white still do, and a calibrated picture keeps its balance. The canvas maps the picture into the surface's
primaries with Seiza's `DISPLAY_SHADER`: sRGB for `--output srgb8`, Display
P3 for the others. A color the display cannot show is desaturated toward
the gray of its own luminance until it fits, so it keeps its luminance and
hue. A TIFF keeps the working space and embeds its ICC profile, which
Studio makes with Little CMS; a PNG is mapped to Display P3 the same way
and embeds that profile; an AVIF is Rec. 2020 already.

**Texture and noise.** Studio measures the picture's noise from its
samples, as the Finish steps before Tone leave them, and gives it to Tone,
so Texture leaves noise alone and raises detail above it. On the Sh2-157
stack, Texture +0.4 raises the sky's noise by 7%. A Finish step with a
mask shows the histogram of its area.

**Histogram.** The inspector of the Final picture, of a Tone step and of an
output stretch shows the histogram of the final picture: red, green, blue
and luminance, with the share of pixels clipped to black and to white.
The render thread samples the final picture on the GPU, every Finish step
drawn as the canvas draws it, at the same full-resolution patches the
combined picture is sampled at, and Studio only counts the samples. It
used to work the Finish steps out again on the CPU, which took about a
second on the main thread for each slider move; counting now takes a few
milliseconds. The render thread draws up to 8 patches between frames and
copies them to a buffer it maps without waiting; it takes the samples on a
later frame, once the GPU has finished. All patches of one sampling share
one upload of the scene's tables and masks. On the Cat's Eye each call
takes about 8 ms on the render thread, down from about 64 ms when every
patch uploaded them again and the thread waited for the copy. Set
`PSF_STUDIO_TIMING` to print where an edit's time goes.

**Scenes off the UI thread.** An edit hands a copy of the document to the
scene thread and returns at once. That thread makes each band's stretch
analysis, a layered stretch's maps, Texture's noise measure and the output
stretches resolved from the picture, then writes the scenes for the render
thread to draw. A newer edit replaces one the thread has not started, and
scenes a newer edit replaces before they are written are dropped. On the
Cat's Eye, an analysis takes 60–100 ms on that thread; the UI thread spends
under 0.1 ms per slider move. Export and the parity check still build their
scene on the UI thread, once per click. Each scene records the place it was
built for, and a pane's label names that place, so the label matches the
picture while a newer scene is built; a line under it names the place on
its way (“Drawing OIII · after Processing tools on color…”).

A store a tool run prepared opens on a thread of its own as well, with
**Opening the store** in the status line. The canvas shows the picture it
had until the store is open, then the new document and store together. A
store that cannot be opened leaves the picture as it was. The store given
when Studio starts opens before the window appears, in about 260 ms for the
Cat's Eye.

**Export.** **Export** in the Final picture's inspector writes the whole
picture, cropped, at full resolution: TIFF at 16 bits per channel or as
32-bit floats in the working space, PNG at 16 bits in Display P3, AVIF as
HDR, or FITS or XISF as 32-bit floats in the working space, through Seiza's
`seiza_image_io`, which replaces the destination only once the file is
complete.

The canvas cannot make that file. Its tiles hold half floats, about three
significant digits, and so do its stretch tables. An export is rendered by
Seiza's compositor on the CPU, `seiza_compose::render`, from the store's
32-bit samples. That render is the reference the canvas's GPU render
follows; see “Where the code lives”. It works in tiles with a margin wide
enough for mask smoothing and heals, so unlike the canvas's patches it has
no seams. A cropped picture ends at the crop: smoothing and heals near
its edge see the picture reflected there, not the document beyond it,
which the automatic crop leaves out because some input does not cover it.

The two renders of one picture must agree. Seiza's tests hold its GPU
render to its CPU render. `psf-studio --parity-report <file>` also checks
them on a real store: once the canvas has sampled the picture, it writes
how far Seiza's CPU render of the combined picture is from the canvas's
samples of the same 64 patches. On the Sh2-157 store (six inputs, three
groups, screened stars, a layered stretch) the 3.1 million samples
differed by 9e-5 on average, 4e-4 at the 99.9th percentile and 7e-4 at
most: half-float rounding, since a half float's step at 0.5 is 4.9e-4.

Built and tried on that store: Tone and color with every control, the
histogram following each change, and an export of the whole 6135 × 4038
picture to a 16-bit TIFF of 148.6 MB. With no control that reads a
neighborhood, that export differs from Studio's earlier CPU render by at
most 6 16-bit steps, and by at most one in 99.9% of samples. On that
noisy stack, Texture also raises the sky's noise: the noise is stronger
than the detail Texture leaves alone. Not built: a histogram limited to a
masked step's area; choosing an ICC profile for an export, which holds the
canvas's working-space values without one.

**Where the code lives.** Seiza draws the picture. `seiza-compose` holds
the description of a picture (`Picture`: layers reading inputs through
transfers, masks, heals and Finish steps), its tiled CPU render, and its
GPU render with the shaders. PSF Studio keeps its own view of a document
(`studio/app/src/canvas.rs`): layers that name a band and a stretch the
document resolves later. Once each band's stretch is known, Studio turns
that view into a Seiza `Picture`, with each stretch as a Seiza transfer.
The canvas (`studio/render`) chooses and uploads tiles, draws each pane
with Seiza's `GpuCompositor`, and applies the display transform as it
presents. An export and the parity report render the same `Picture` with
Seiza's CPU render. Studio has no shaders or CPU render of its own. Until
a Seiza release has `seiza-compose`, Studio's workspace pins Seiza to the
pull request's commit.

**One renderer, not two.** PSF Guard's color previews and PSF Studio must
end up drawing a picture the same way. Seiza's `combine_*` functions now
build their pictures with the same compositor, with their output
unchanged. The next step is for PSF Guard, once it moves to a Seiza
release with `seiza-compose`, to render a color preview from its document
(the setup maps to one; see “The default document”) through that
compositor, keyed on `seiza_compose::OUTPUT_VERSION`.

## Processing tools in Studio (2026-11-25, first part)

The tools are the ones PSF Guard's color processing runs: sharpening,
noise reduction and star removal, each by a provider. The providers today
are RC-Astro's BlurXTerminator, NoiseXTerminator and StarXTerminator, and
Seiza's built-in noise model. Each tool is a step of its own in the Linear
stage (see “Each tool is a step”); this section first described one chain
per input in one “Processing tools” step, which documents opened now are
split from.

A tool takes seconds to minutes on a whole stack, so it is not live.
Studio runs the tools when a document's store is prepared, and keeps each
result.

- **Shared step.** `color_steps::run_tool_chain` runs one chain on one
  linear image, and `color_steps::ToolChain::resolve` fills in a chain's
  settings and cache ids. PSF Guard's color previews call the same
  function; their behavior is unchanged. A provider is chosen by its key,
  so a Seiza model and an RC-Astro tool go the same way.
- **Preparing a store.** `psf-guard studio-sources --apply <document>`
  runs the Linear steps that change the inputs in the document's order.
  PSF Guard's own color setups put the tools before the background, as
  the color pipeline runs them.
- **What the store keeps.** Per input: the stack as it came; the input
  after each step that changed it (`<input>@step-<id>`); and the stars a
  star removal kept (`<input>@stars`).
- **Cache.** Each step's result is kept beside the downloaded stacks, so
  a chain that grew by one tool runs only that tool.

Tried on the Sh2-157 stacks on this Mac: BlurXTerminator and
NoiseXTerminator on Ha, OIII and SII, and StarXTerminator on Ha, took 68
seconds in all. StarXTerminator returned no stars image with its default
settings, so the store has no `ha@stars`.

Seiza's noise model, installed with `psf-guard noise-model download`, ran
on the same three stacks through the same step in 87 seconds
(noise-v1-d33 on the GPU through Core ML). Nothing in Studio is particular
to a provider: the chain names `noise-model` where it named `nxt`.

With a store prepared for the document, the canvas shows an input after
each of its steps: selecting a tool step shows the input after that tool,
so Before and after and Wipe compare the input before and after it.

### Choosing and running tools from Studio

The app does not run a tool itself, and does not link PSF Guard's server
code. It works through two commands of the `psf-guard` program, so a web
canvas could do the same through the server:

- `psf-guard studio-tools` prints the tools this computer can run, as
  JSON: each provider with what it does, and its settings with their
  kinds, defaults and ranges, as the tool itself reports them.
- `psf-guard studio-prepare --sources <folder> --apply <document>
  --output <store>` prepares the store again from the channel stacks
  `studio-sources` downloaded, with no server. `studio-sources` now writes
  a `sources.json` beside the stacks for this.

In the app, started with `--psf-guard <path>`:

- An input's controls list one choice per kind of tool: Sharpening, Noise
  reduction and Star removal, each **None** or one of the providers the
  computer has. A tool this computer cannot run is listed with the reason.
- Each chosen tool's settings follow, as sliders and switches from the
  tool's own list.
- Each choice is a tool step of the input, which PSF Guard's settings
  read back as the input's chain (`input_rc_astro`).
- **Run the tools** saves the document, runs `studio-prepare`, shows the
  tool and its progress as it goes, and has the canvas draw the new store
  when it ends, without a restart. A note says when the tools have changed
  since the store was prepared.

A tool that ran with the same settings before is not run again: each
step's result is kept beside the stacks.

#### Each tool is a step (2026-11-27)

Every processing tool is a step of its own in the Linear stage, as every
other operation is: Sharpening, Noise reduction and Star removal, each
with a provider and its settings, and the band of an input or of a color
group. A step can be turned off, moved and removed like any other, and an
input can have a tool before the background extraction and another after
it. Until this change an input's tools were one “Processing tools” step
holding a chain, and preparation ran a fixed order (every tool, then
every background), because the store was built from PSF Guard's color
setup, which has that shape. That fixed order is gone.

- **Opening older documents.** A “Processing tools” or “Processing tools
  on color” step is split into one tool step per tool, in its place and in
  the chain's order. The first keeps the old step's id. PSF Guard's color
  tools, which had no band, take the color group they run on.
- **Order.** **Move earlier** and **Move later** on a tool, a
  deconvolution or a background extraction move it past the step before or
  after it in the input's list. A step shared by every input, such as
  Background extraction, moves for every input. A new tool goes after the
  input's tools of an earlier kind, and else before Background extraction.
- **Preparing.** `studio-prepare` reads the document's Linear steps and
  runs each one that is on and changes an input
  (`studio::changes_input`), in order: a tool on its input in the input's
  own frame, a color group's tool on the group's inputs registered and
  combined, a deconvolution, and each Background extraction, fitted with
  one model across the inputs it is for. A step after a background
  extraction takes its input at the step's strength.
- **What the store keeps.** For each input, its plane after each step
  (`<input>@step-<id>`, a background extraction corrected in full), and
  its record lists them in order (`"states"`) with the steps it ran
  (`"program"`, `studio::input_program`).
- **Canvas.** An input is drawn after the last step the view reached that
  the store holds. A background extraction there is blended into the
  state before it by its strength, at once. A step the store was prepared
  without is listed as left out, and the toolbar lists the inputs Run
  tools would update. A store prepared before this change keeps its
  `@tools` and `@background` planes, which stand for every tool step and
  for the background extraction.
- **Cache.** Tool steps that follow one another in a chain's order on one
  input (sharpening, then noise reduction, then star removal) run as one
  chain, so each tool's result is kept by the chain so far, as PSF Guard
  keeps a chain's. A tool after another kind of step is kept by that
  step's result. So a store prepared again from an older document finds
  every tool's result it ran before.
- **PSF Guard's settings.** An input's tool steps read back as its chain
  and a color group's as the color tools, one of each kind, in the
  document's order; PSF Guard runs them before the background. A color
  group's star removal stays Studio's.

### Stars

When a star removal keeps the stars it took out (StarXTerminator's
“Generate Star Image”), the store holds them as `<input>@stars`. Studio
lists them as an input of their own, “Ha stars”, and adds a layer for
them on top with the screen blend, which puts stars back onto a starless
picture.

- The stars are stretched as their input is, from the input's own
  analysis, so the two go back together. Stretched on their own, a stars
  image is almost all background and the stretch blows it up.
- A palette leaves a stars layer where it is.
- PSF Guard's settings have no place for a stars input, so it stays out
  of them.

#### A stretch of their own, and star reduction (2026-11-27)

A stars input can have a stretch of its own: in the stars input's
inspector, **Stretch on their own** copies its input's Stretch step for
`<input>@stars`, and **Follow the input's stretch** removes it again.

**Star reduction** on a stars layer, or on a group of stars layers such as
RGB stars, makes the stars smaller. Each star is blended toward a one-pixel
cross erosion of itself, Seiza's `cross_erosion`, which its RGB star
reduction also uses. A group's stars are reduced together: the erosion is
measured on the brightest of its stars inputs at each pixel, and every
input is scaled by the same amount, so the stars keep their colors. A
layer's own amount, above 0%, is used in place of its group's.

The reduced stars are planes worked out from the whole image, as the
luminance of a layered stretch is: the first change works them out, one
plane per stars input, and the canvas draws the stars as they are until
then. The amount only blends between the two, so moving the slider draws
at once. The planes are made before the stretch; for stars the store
keeps stretched for display, that matches Seiza's reduction, and for
linear stars it matches it at 0% and 100%. A stars layer whose background
correction is blended in cannot be reduced too, and its pane says so.

On the Cat's Eye, full reduction left 85 of 148 star pixels above a
display value of 96 in a 400-pixel square of sky, and took a quarter of
the square's star light.

#### Star removal on a color group (2026-11-25)

Star removal runs on each input alone, or on a color group together:
red, green and blue, or the narrowband lines SII, Ha and OIII. The
person chooses. In the Tools block of an input with a star removal tool,
**Run on** offers **This input** or the group's inputs together. The
choice moves the tool and its settings between each input's Processing
tools step and one step for the group.

- The group's step is a Star removal step with the group's band, `rgb` or
  `lines`.
- When the store is prepared, the group's step runs where it is in the
  document: the group's registered inputs, as the steps before it left
  them, are combined into one color image, the tool runs on it, and the
  result is split back into each input's plane after the step and its
  `@stars`. A group of two inputs repeats the last one to make three
  channels. The steps after it on those inputs run on the reference
  grid.
- PSF Guard refuses star removal among its color tools (“Star removal runs
  on channel stacks, not on the color channels together”), so the group
  steps stay out of the exported color setup. A setup exported from such a
  document has no star removal.
- The cache key of a group's result includes each input's stack and the
  last tool that ran on it, so a change to an input's own tools runs the
  group's tools again.

### Tried

On the Sh2-157 stacks on this Mac:

- BlurXTerminator and NoiseXTerminator on Ha, OIII and SII, and
  StarXTerminator on Ha, from the command: 68 seconds in all.
- Seiza's noise model, installed with `psf-guard noise-model download`,
  on the same three stacks through the same step: 87 seconds
  (noise-v1-d33 on the GPU through Core ML). Nothing in Studio is
  particular to a provider: the chain names `noise-model` where it named
  `nxt`.
- From the app: StarXTerminator chosen for Ha with its star image
  switched on, then **Run the tools**. The store was prepared again in 15
  seconds, and the canvas took it and listed “Ha stars” as an input and a
  layer.

### Not built yet

- The narrowband star steps, which run after the channels are resampled
  and normalized together, which Studio's preparation does not do yet.
  PSF Guard's color tools (`color_rc_astro`) now run as color group tool
  steps.
- A run cannot be stopped from the app, and a second cannot be queued.
- The app saves the document only when the tools are run.

## Setup and workspace: one flow for every kind of picture (2026-11-25)

Partly built in the spike; “What is built” lists which parts. The tree, palettes, groups, masks and tools above
serve one narrowband picture well. They do not yet make these easy to set
up or to change:

- narrowband alone;
- narrowband blended into RGB, such as Ha into red;
- narrowband with its continuum subtracted;
- a narrowband nebula with RGB stars;
- RGB, LRGB, and HaLRGB.

All five answer the same two questions: what is each input for, and how do
the results combine. The flow asks those two questions directly.

### Parts

A picture is made of a few **parts**. A part is a group in the Combine
stage, with a name, the inputs that feed it, a palette for them, and a
blend onto the parts beneath:

| Part | Fed by | Blends |
|---|---|---|
| Nebula | starless narrowband inputs, by a narrowband palette | first |
| Color | R, G, B, by the RGB palette | first |
| Luminance | L, or L with some Ha | luminance |
| Stars | the stars of R, G, B (or of the narrowband inputs) | screen |
| Line into color | Ha into red, OIII into green and blue | lighten or add, by an amount |

A part is nothing new in the document: it is a group, its layers and its
shared masks. A person who never opens Setup can build the same thing by
hand.

### Nothing locks

A recipe and a part are ways to start, not a flow a document must stay in.
The rules that keep it so:

- **Setup is part of the workspace, not a page.** A separate Setup page
  was built first and dropped the same day: it hid the tree, so a person
  lost sight of what they were building. What goes where is now shown in
  the tree and the inspector.
- **Routes are read from the layers.** Where an input goes is read from
  the layers as they are (`Document::routes`), however they were made.
  No plan is stored beside them, so there is no state in which a document
  has “left” its recipe.
- **Any input goes into any part.** “Add to” in an input's controls, or
  “Add layer” in a part's, adds one layer that shows the input, in the input's
  usual hue, on top of that part's layers. Nothing else changes. Ha joins
  the Color part of an LRGB picture this way, and NII joins a narrowband
  nebula.
- **A palette replaces only the layers it made.** Each of those layers
  records its slot. A layer a person added beside them stays when the
  palette, or the input of one of its slots, changes.
- **Each part has its own palette and masks.** Two parts may use the same
  palette with different inputs; a Foraxx mask belongs to the part whose
  palette made it.
- **A recipe replaces only when asked.** “Recipe” over the layers replaces the
  layers that show inputs, and the groups; adjustment layers stay. It is
  never applied again on its own.
- **No kind is checked later.** A part's kind decides what it starts
  with. After that it is a group: its blend, layers, palette and order
  are free, and a part may hold layers no recipe would put there.

### Setup: what goes where

As first proposed, Setup was a page of its own with three things. It is
now part of the workspace (see “Nothing locks”); the three things remain:

1. **Recipes.** The stock setups that fit the inputs the document has,
   each with a small picture of its parts: Narrowband; Narrowband with RGB
   stars; Narrowband into RGB; RGB; LRGB; HaLRGB. One click fills in the
   rest of the page. “Subtract continuum” is a switch on any recipe that
   has both a line and a broadband input, not a recipe of its own.
2. **The routing table.** One row per input. Each row says which tools
   run on the input and what the input feeds: “Ha → Nebula, as green”,
   “R → Stars, as red; continuum for Ha and SII”, “L → Luminance”. A
   destination is added, changed or removed in place. This table is the
   answer to “what goes where”, and it is the same table whichever recipe
   filled it.
3. **The parts.** One card per part: its palette, its tones, its blend and
   amount, and its order. Reordering the cards reorders the groups.

Changing Setup remakes the layers and masks it made, and keeps a person's
tweaks to them, as giving a palette slot another input does today.

### Inputs made from inputs

Two things a recipe needs are inputs computed from other inputs:

- **Stars and starless**, from a star removal. Built: the stars of an
  input are an input of their own.
- **A line with its continuum subtracted**, such as Ha minus a share of
  R. PSF Guard has this as its emission step.

Both are listed under the input they come from, and are routed like any
input.

### Workspace: breaking the steps apart

The workspace is the tree as it is, with the Combine stage grouped by
part. Everything Setup made is ordinary layers, masks and steps there, so
“manually breaking apart the steps” needs no conversion: a person opens
the part and edits it.

What changes is who keeps a part up to date:

- A part Setup made **follows Setup**. Tones, opacity, adjustments, mask
  reach and touch-ups are a person's tweaks and are kept when Setup
  remakes the part.
- Changing a part's structure in the workspace (adding, removing or
  reordering its layers, or giving a mask other inputs) makes the part
  **edited by hand**. Setup then shows it as such, stops remaking it, and
  offers **Reset to the recipe**.

So Setup is the fast way in and stays useful after tweaks, and nothing a
person builds by hand is overwritten.

### Seeing how it combines

- The Setup page's routing table and part cards are the summary.
- In the workspace, selecting a part shows it alone or the picture up to
  it, as a group does today. The Input and picture split shows an input
  beside the part it feeds.

### Layout of the controls

The spike's right pane grew one row of text buttons at a time, and reads
as a scatter of buttons. The controls follow these rules from here on:

- **Sections that fold.** A row's controls are a few named sections
  (for a layer: Source, Color, Blend, Mask, Adjustments). Each folds, and
  the pane remembers which are open per kind of row. The section a person
  most likely came for is open; the rest are closed.
- **One control per decision, of the right kind.** A choice between two
  or three things is one segmented control, not separate buttons. A
  choice from a longer list (blend mode, stretch, palette, provider) is
  one menu that shows its current value. An on or off is one switch. A
  number is a slider with its value beside it, on one line with its
  label.
- **Label on the left, control on the right,** in one column, so the eye
  runs down the labels.
- **Actions apart from settings.** Things that change the document's
  structure (Add, Duplicate, Remove, Ungroup, Run the tools) sit in one
  row at the foot of the section they act on, and are not mixed in with
  settings.
- **Help on demand.** A sentence of explanation belongs to a setting's
  tooltip or a folded “About” line, not under every control. What stays
  on screen is state a person needs: what a row applies to, and what the
  canvas leaves out.
- **Nothing a row cannot use.** A control that does not apply is left
  out, not shown disabled with a reason, except where a person would
  look for it and needs the reason.

Setup's routing table and part cards follow the same rules.

### The layered stretch

Seiza's layered stretch treats large and fine structure apart, so it
needs more than each pixel. The canvas draws it live, through Seiza's
compositor: when Studio opens a store it reads every band at a quarter of
the resolution from the store's pyramid, and a layered stretch measures
its maps from that copy (the HDR core compression's base and the contrast
recovery's large-scale layer), again only when a setting that moves them
changes. The curve, the core compression, the HDR tail and the detail gain
are worked out per pixel, on the GPU for the canvas and on the CPU for an
export, so moving Black point, Background or Core compression redraws at
once. The HDR layered stretch reaches as far as the picture's headroom;
in an SDR picture it rolls off to white. Each band is stretched on its
own, so calibrated red, green and blue keep their balance through their
shared statistics. Two things a layered stretch reads need the whole
image at full resolution, so Studio works them out as tasks while a
person edits, with their progress in the status line: the fine blur that
star-core compression measures detail against, and, for red, green and
blue calibrated together, their luminance, which the stretch reads and
each channel then follows. Until a plane is done the canvas draws without
it; when it is, the canvas draws from a source that adds it to the
store's channels, its tiles load again, and exports read it too. On the
Cat's Eye store a band's fine blur takes under half a second.

A band's stretch is resolved from samples of the band. When Studio opens a
store it reads patches spread across the image, which are fast but can
miss a small bright object, and then samples every band evenly over the
whole region every input covers as a task (0.2 s on the Cat's Eye store).
When that is done the stretches are resolved again from it and the canvas
redraws.

### What is built

In `src/studio/recipe.rs` and `src/studio/palette.rs`, with tests in
`src/studio/recipe_tests.rs`:

- parts of seven kinds (Nebula, Color, Luminance, Stars, RGB stars,
  Emission lines, Empty) and seven recipes made of them;
- palettes per part, slots recorded on layers, masks per part;
- `routes`, `add_layer_showing`, `add_part`, `use_recipe`.

In the spike app (`parts.rs`, `widgets.rs`), all in the workspace:

- the tree shows, on each input's row, the parts the input goes to, or
  “not used”;
- over the layers, the Recipe and Add part menus;
- an input's controls show “Goes to” (one chip per layer that shows it; a
  click selects the layer) and “Add to” (any part, or Top);
- a part's controls hold Blend, Palette, the slots' inputs, Add layer,
  Ungroup and Delete;
- the layout rules below: fields with the label on the left, menus in
  place of rows of buttons, segmented controls for two or three choices,
  folding sections, and help text hidden until “?” is on. The processing
  tools use the same fields and menus.

Tried on the Sh2-157 Ha, OIII and SII stacks (recipe “Narrowband with its
own stars”, then Ha added to the Nebula part) and on the RGB stacks.

Also tried on a store of Ha, OIII, SII, R, G and B with star removal on
all six: the recipe “Narrowband with RGB stars” gives a starless SHO
nebula with the stars of R, G and B screened over it.

A recipe or part that shows stars apart (RGB stars, Stars) needs them
separated, so choosing one sees to it: every input the picture shows gets
the first star removal tool this computer has, a color group's inputs
together so the stars keep their colors, and `psf-guard` prepares the
store again, with its progress in the status line. Before, the recipe only
made the layers; the nebula kept its stars and the stars layers read
channels the store did not have. A document whose stars layers show no
stars says so under the Recipe menu. Without a star removal tool the
status line says what is missing.

**Gaia calibration of R, G and B** runs in the store preparation
(`calibrate_colors` in `src/commands/studio_sources.rs`, on the shared step
`color_steps::measure_color_gains`):

- `studio-sources` fetches the plate solve of the first stack's reference
  frame, asking the server to solve it when it has none, and keeps it in
  the sources folder as `reference-solve.json`.
- The preparation measures the gains on red, green and blue as stacked
  and registered, where they still have their stars, through the solve
  carried to the stacks' grid and the installed Gaia colors.
- The gains are applied to every state of a channel: as stacked, after
  its tools, its stars, and with its background corrected. Then the sky
  of red and blue is levelled to green's in every state that has a sky,
  because a gain scales the sky too.
- `<store>.prepared.json` records what happened. The spike reads it, and
  resolves the stretches of calibrated red, green and blue from their
  samples together, so one stretch serves the three and the balance is
  kept. A document whose Color calibration step is off is left alone.

On the Sh2-157 stacks: the reference frame solved with 278 stars at
0.31 arcsec RMS, and 523 Gaia stars gave red x1.027 and blue x1.303.

**The star stretch** is the step “Star stretch” in the Stretch stage, with
an amount from 0 to 8. The RGB stars part adds it. The preparation runs
Seiza's `stretch_rgb_stars`, the call PSF Guard's RGB stars use, on the
calibrated channels with and without their stars, and writes the result
as the three stars planes: one linked display layer. The record lists
those planes as stretched for display, and the spike draws them as they
are. A change of the amount is applied by preparing the store again
(“Run tools”). PSF Guard's star reduction and its saving of the amount in
the `narrowband` settings are not carried over yet.

**Tools not run yet.** Studio compares each input's tools, deconvolution,
color group tools and continuum subtraction with the ones the store was
prepared with. When they differ, the toolbar shows “Tools changed for Ha,
OIII, SII” and a **Run tools** button, and each affected input's row says
“tools not run”. A color group's tools count for each of its inputs, and
continuum subtraction for each line. The tools a store was prepared with
are kept beside it, in `<store>.tools.json`, with the store's size and
modification time, so the flag is still there after Studio restarts. A
record of another version of the store, such as one prepared outside
Studio, is not read; the document's tools are then taken as prepared.

A star removal in Studio always keeps the stars it removes, linear, with
Unscreen Stars off: `psf-guard` sets those settings itself
(`rc_astro::keep_linear_stars`), and Studio does not show them. A store's
new stars become inputs; they get a layer of their own only when the
picture has no stars layer yet, so the stars are not added twice.

**The layered stretch** is Studio's default in place of the automatic
midtones stretch: a new document's stretches are changed by
`Document::use_layered_stretch`, and PSF Guard's own default setup is not.
The layered stretch blurs the image, so it cannot be a table resolved from
samples. The preparation runs each such channel's whole stretch chain with
Seiza's stretch stack (`color_steps::stretch_for_display`) on the
channel's last linear state and adds the result as `<channel>@stretched`.
Calibrated red, green and blue with the same stretches are stretched
together as one color image. The spike draws a stretched plane as it is
wherever the view applies the channel's stretches. Changing a layered
stretch's settings takes effect when the store is prepared again. The
stars of such an input follow it: the input with its stars put back is
stretched with the very stretch the starless input got (the same input
range and Seiza's resolved plans, `color_steps::DisplayStretch`), and the
stars are taken out of that with the unscreen Seiza's star screen uses, so
screening them over the starless input gives the input with its stars.
Stars already stretched by the star stretch are left as they are.

**Stopping a run.** While Studio prepares a store or a picture, the top bar
shows **Stop**. The run is started in a process group of its own, so Stop
ends `psf-guard` and the tools it started, and the partial store it was
writing is removed; the store and the picture stay as they were, and the
tool results already finished are kept for the next run. Stop works on
macOS and Linux; a run started from Studio does not end when Studio
quits.

**From stack files, with no server.** `psf-guard studio-files --channel
ha=Ha.xisf --channel oiii=… --output picture.pgtiles` prepares the same
store from FITS or XISF stacks a person has. It builds the same list of
channel stacks `studio-sources` downloads and runs the same preparation,
so tools, calibration, the star stretch and the layered stretch are the
same code on both paths. The files are read where they are; the sources
folder holds their paths (`paths.json`), the plate solve and the tools'
results. The first file is plate solved on this computer for color
calibration. A stack often records no pointing, so `--pointing
RA,DEC,SCALE` gives the solve its start; a blind index would remove the
need. On the Sh2-157 R, G and B stacks this path gave red x1.028 and blue
x1.304, against x1.027 and x1.303 through the server.

**One command to the editor.** `psf-guard studio` prepares a picture
from either source and opens it in Studio:

```bash
psf-guard studio --server http://localhost:3000 --db my-db --project 11 --target 17 --folder ~/Pictures/studio --name sh2-157
```

```bash
psf-guard studio --channel red=R.xisf --channel green=G.xisf --channel blue=B.xisf --pointing 348.94,60.17,1.03 --folder ~/Pictures/studio --name sh2-157-rgb
```

The picture's store and document are `<name>.pgtiles` and `<name>.json`
in the folder. A document that is already there is kept, and the store is
prepared for it. The application is given with `--app` or the
`PSF_STUDIO_APP` environment variable; `--no-open` prepares without
opening.

**Opening from inside Studio and from PSF Guard.** Studio lives in
`studio/`, a workspace of its own, and its program is `psf-studio`.

- **Open** in Studio's top bar asks for stack files and opens them. Studio
  passes the files to `psf-guard studio --file …`, which reads each file's
  channel from its `FILTER` header with PSF Guard's own filter classifier,
  or from the file's name.
- PSF Guard's color stacks show **Open *target* in PSF Studio**, a link of
  the form `psf-studio://open?server=…&db=…&project=…&target=…`. The
  application registers that scheme (`studio/bundle-macos.sh` writes the
  `Info.plist`). Studio takes only links whose server is HTTP or HTTPS,
  and runs `psf-guard studio --server …` for the target.
- Both show the run's last line in the top bar while it prepares, then
  the picture. Pictures are kept in Studio's own folder, by target or by
  the set of files, so opening one again keeps its document.

The link carries no token, so a server with user accounts refuses Studio's
requests; Studio has no sign-in yet. Nothing runs on the server for the
button: the page only makes the link.

**Cropping.** The document records what the picture keeps
(`Document.crop`): **Auto**, the largest rectangle every input covers,
which the preparation measures with Seiza's `crop_report` (the crop PSF
Guard's previews call "Inscribed") and records in `<store>.prepared.json`;
**Whole**, the full grid with the empty edges registration leaves; or a
rectangle drawn with the **Crop** tool, which stays inside the automatic
one. The canvas draws only inside the crop (`Frame::crop`), and a fit fits
the crop. Set it on the Final picture.

**Mask editing.** The brush has a size, and paints or erases on any mask:
a layer's, a step's, or a shared mask's touch-ups. **Undo stroke** removes
a mask's newest stroke (`Document::remove_last_stroke`). A painted mask has
a **Blur**, in document pixels, that softens the edge of every stroke
without changing them (`PaintedMask::blur`, drawn by `Stroke::blurred` on
the canvas and in the engine alike).

**Continuum subtraction** runs when a store is prepared, when the document
has the step "Subtract continuum" (Continuum › Subtract on an Ha, OIII or
SII input). The fit is `color_steps::fit_continuum_line`, which PSF Guard's
color previews call too; Seiza's `subtract_continuum` then removes the
continuum. The fit measures stars, so it runs on the stacks as they came;
the subtraction is applied to each input in its last linear state (with its
background corrected, else after its tools, else as stacked), after the
background step. The line and its references must agree on their stars:
all starless or all with their stars, or the subtraction is skipped with
that reason, since otherwise the line would keep or lose the stars of its
references. OIII's continuum comes from G and B, the others' from R. The
result is a new input, `<line>@line` ("Ha line"), stretched as its channel
is, and the Emission lines part uses it when it is there, at half opacity.
A color recipe adds the RGB stars part when R, G and B lost their stars to
a star removal.

The fit needs to know where each stack clipped. `studio-sources` downloads
the clipping map the stacker wrote (`…/stack-previews/{job}/{group}/clipped`,
added for this). A map cannot be made from a finished stack, because only
the stacker sees which frames clipped, so stack files from elsewhere need an
explicit saturation level per input in the step's settings.

Seiza checks the profiles of held-out stars after the fit and refuses a fit
whose stars would keep cores or halos. On the Sh2-157 stacks it refused all
three lines, with and without PSF matching, although the photometric checks
passed. The worst stars were bright ones, and their residual came mostly
from sub-pixel registration offsets and non-Gaussian profile differences
between the narrowband and broadband stacks. Those show only on stars. So
when the line and its references are starless, Studio fits with Seiza's
`ContinuumTarget::Starless` (theatrus/seiza#524): the check still runs and
is reported, and the photometric checks decide. Starless, the three lines
are subtracted with the check on (weights Ha 0.264, OIII 0.187 and 0.306,
SII 0.337); with their stars kept they are still refused. PSF Guard's
previews still fit for images with stars.

Input deconvolution runs when a store is prepared, before the input's
tools, with the step PSF Guard's color previews run
(`color_steps::deconvolve`), and its result is kept in the sources folder.
The store holds the input after its deconvolution and its tools together,
so the view shows that state from either step on. A mosaic is not color
calibrated, because PSF Guard calibrates a mosaic with a solve of its
canvas, which Studio does not make yet.

Not built: choosing a server target from a list inside Studio, sign-in, tools per input in one table, continuum subtraction, groups in groups, and a recipe's small picture of its
parts. No luminance stack was at hand, so the Luminance part is covered
by tests only.

### What it needs

1. A record of the setup in the document: the recipe's name, the routing
   table, and each part's options. It takes the place of today's record of
   one palette and its slots.
2. Whether a part follows Setup or is edited by hand.
3. The Setup page.
4. Stars for more than one input at once, and an RGB palette over stars
   inputs.
5. Continuum subtraction as a shared step that Studio's preparation runs,
   giving a `<line>@continuum` channel. This is the largest piece: PSF
   Guard runs it after the channels are resampled and normalized together.

6. The controls laid out by the rules above, and the layered stretch.

Order: the layout first, since every later piece adds controls. Then 1 to
3, for the recipes that need nothing new (narrowband, RGB, LRGB, HaLRGB,
narrowband into RGB). Then 4, for RGB stars, and the layered stretch. Then
5.

## Protected background regions (2026-11-27)

A Background extraction step keeps regions out of the sky it measures, so a
nebula's faint halo is not taken for sky and subtracted. With the step
selected, the canvas outlines its regions and its inspector lists them,
each with **Remove**. **Protect**, among the tools while the step is
selected, draws an ellipse: press at its centre and drag out to set its
width and height. Regions are stored as Seiza takes them,
`config.protected_regions`, in fractions of the image; the fit keeps them
and adds the catalog's when catalog protection is on. The background is
worked out when the store is prepared, so each input's background settings,
but for the strength the canvas applies, join the tools the toolbar compares
with the prepared record. A record written before then takes them as they
are when Studio opens the store.

## Driving Studio from an assistant (2026-11-26)

An assistant working on Studio needs to see what Studio shows, not what a
screenshot of a covered window or an SDR capture leaves of it. Each window
listens on a control socket, `$TMPDIR/psf-studio/<pid>.sock`, made by
`studio/app/src/control.rs`, which only its user can open
(`--no-control` turns it off). A request is a line of JSON; Studio answers
it on the UI thread, within a few frames, as it does a click.
`psf-studio-mcp` (`studio/mcp`) is a Model Context Protocol server over
standard input and output that forwards its tools to these sockets.

- **State.** The rows a person can select, with their ids and settings;
  the selection; each pane's label, the place wanted, the place drawn and
  what the pane leaves out;
  the status line; the inputs whose tools changed since the store was
  prepared; the histogram's clipping; and what Studio is still busy with.
- **Edits.** Select a row, divide the canvas, merge settings into a step
  or a layer, run the tools. In a merge a `null` removes its key, and a
  stretch switched to another kind starts from that kind's settings. Edits go through the document as a person's
  do, and are saved the same way.
- **Waiting.** Busy lists what keeps the canvas from showing the document
  as it is: scenes being built or not yet drawn in their panes, a frame
  to draw, tiles loading, tasks and tool runs, and samplings not read.
  The server waits until it has been empty twice.
- **Seeing.** A capture copies the next frame the render thread draws, as
  the display was given it: 8-bit, or half floats on an extended surface,
  which the request may divide by a value (such as the HDR peak) before
  it is encoded. A covered window's surface has no texture, so a frame to
  capture is then drawn into one of the surface's size. A sample renders
  a region of the final picture on the CPU at full resolution, as an
  export does, and returns each channel's values with the shares above
  SDR white and at the peak, and an image of the region.

The server opens a window on a copy of a document unless asked not to,
and closes only windows it opened: a window a person works in is theirs.

## Status and gaps (2026-11-27)

What works end to end, on the Cat's Eye (NGC 6543, C925 HOO and RGB) and
the Iris (NGC 7023, UltraCat 131 LRGB, `studio/experiments/iris-hdr/run.py`):

- HDR as a mode of the picture, through every stretch, Tone and export
  (AVIF HDR, float TIFF, FITS, XISF), with a Format choice on Export.
- BlurXTerminator, NoiseXTerminator and StarXTerminator per input or per
  color group, the removed stars kept as inputs and screened back up to the
  HDR peak, and a flag in the toolbar when tools changed since the store
  was prepared.
- Background extraction with regions drawn on the canvas, and a sky floor
  on the layered stretches for what it leaves.
- Seiza's Gaia color calibration, when the Gaia photometry set is
  installed.
- Image work off the UI thread, and `psf-studio-mcp` to drive and see a
  window.

Gaps to fix, roughly in the order they got in the way:

- [ ] **BlurXTerminator's tile grid.** A soft pattern about 350–415 px apart,
  about 0.004 ADU, appears in BlurXTerminator's output and becomes the
  strongest structure in a dark narrowband sky once noise reduction and
  background extraction remove the rest. An overlap of 0.5 did not remove
  it. To try: run BlurXTerminator on a copy shifted by half a tile and
  average, or keep the background model from following it.
- [x] **Sky floor on every stretch kind.** An input's stretch of any other
  kind has a **Sky floor** too, kept beside its model as `sky_floor` and
  drawn as PixInsight's histogram transformation after it, with shadows at
  the floor and midtones at the middle: `(x − floor) / (1 − floor)`, the
  layered stretches' floor in an SDR picture. After an HDR stretch it
  shapes the range below white and keeps the headroom. On the Iris's
  luminance under the automatic midtones stretch, a floor of 0.15 took
  the sky's median from 0.16 to 0.01. PSF Guard's settings leave it out.
- [x] **A knee for every stretch in HDR mode.** The picture's Knee moves
  where every stretch continues into the headroom, and the tail reaches the
  peak at the band's brightest sample, so no core is left flat at the peak.
  Still open: core compression, which raises local contrast in the core, is
  only in Layered, HDR; a stage any stretch can take, or more HDR
  stretches, would carry it further. The knee is the picture's, not each
  stretch's.
- [x] **Stars' own stretch and star reduction.** A stars input can take a
  stretch of its own, and a stars layer or group a Star reduction (see
  “A stretch of their own, and star reduction”). Still open: the star
  stretch of RGB stars is applied when the store is prepared, not on the
  canvas.
- [x] **Frame limits.** A frame now draws up to 24 layers, 15 masks and 12
  layered stretches. Past those a layer is still left out or drawn with its
  curve alone, and the pane says so.
- [x] **Inputs whose stars a layer shows.** An input was listed as not used
  when only its stars were in the picture; it now shows where its stars go,
  such as “→ RGB stars (stars)”.
- [x] **Tools after the background.** Each tool is a step of its own, in
  any order with the background extraction, per input (see “Each tool is
  a step”).
- [x] **Undo history.** **Undo** and **Redo** in the toolbar and the Edit
  menu (⌘Z, ⇧⌘Z, and `studio_undo` and `studio_redo` through
  `psf-studio-mcp`) go back through up to 200 edits of the document. Edits
  less than 0.7 seconds apart, such as one slider drag, are one edit.
  Opening another picture starts the history again. A run of the tools is
  not undone: undoing its edits flags the inputs to run again.
- [x] **Tool runs.** **Run tools** during a run queues one more run, which
  starts with the document as it is once the store the first run prepared
  is open; the toolbar says “Changed since this run started” and offers
  **Run again after**. A stopped or failed run drops the queued one. PSF
  Guard's color tools (`color_rc_astro`) run as a color group's tool
  steps. Still open: the narrowband star steps, which run after the
  channels are normalized together.
- [ ] **Layers.** No groups in groups or pass-through groups; a group cannot
  be clipped or healed; a shared mask cannot go on a layer's own
  adjustment; a layer cannot have a painted and a shared mask, or clip to
  its shared mask. Layers and parts move with Move up and Move down, and a
  part dragged moves whole; dragging a layer within a part still needs
  work.
- [ ] **One way to adjust.** Finish steps and adjustment layers are two;
  exposure, curves and saturation are Studio's shader code, not Seiza's.
- [ ] **Setup and hand-off.** No Setup page or recipe record in the
  document, no server target or sign-in in Studio, no table of every
  input's tools, no round trip to another editor.
- [x] **UI thread.** A store given at start with a readable document
  beside it opens on a thread of its own after the window shows: the
  window answers with the document's steps while the canvas draws one
  black pixel, then the store (the Iris answered in 0.8 s and drew its
  picture at 4.5 s). A store with no document still opens first, since the
  default flow needs its inputs' names. Export and the parity check build
  their scene on their own threads; a full-resolution TIFF of the Iris
  took 3.7 s.
- [x] **Tried in the app** on the Iris (`psf-studio-mcp`, with its new
  `studio_add_step`): two Tone and color steps with painted masks draw
  together; selecting each shows the histogram of its area (18.5% above
  SDR white in the brightened core, none in the darkened corner); and a
  midtones stretch after the HDR layered stretch keeps the headroom (the
  core's brightest 1.09 at one stop). A Finish step or a layer's own
  adjustment with a range mask or a shared mask was drawn over the whole
  picture; it is now left out, and the pane lists it, until the canvas
  draws such masks on steps.

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
