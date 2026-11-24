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
2. **Pipeline parity.** Rebuild one section of `color.rs` as engine nodes
   with identical output and identical cache keys. This is the largest cost
   and risk in the plan: the automatic previews must move without changing
   results. Report how the rest of `color.rs` would divide into nodes.
3. **Crop preview loop.** One heavy step (Seiza deconvolution or the noise
   model) previewed on a full-resolution crop. Measure slider latency.
4. **Step view and tile keys.** Before and after on one chain, to settle how
   tiles and pyramids are keyed for intermediate outputs.
5. **Tile format.** Sample depth (half or full float), tile size,
   compression, and whether tiles can be mapped from disk straight to the
   GPU.
6. **Out-of-core heavy steps.** Which Seiza operations already work in tiles,
   which need a tiled form, and how RC-Astro tools behave on a mosaic-sized
   file.
7. **Command layer shape.** Define the commands before the first editor
   control exists, and show an MCP tool list generated from them.
8. **PSB writer scope.** Confirm the minimum the format needs for 16-bit
   layers with masks, and whether PhotoshopAPI is worth binding instead.

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
