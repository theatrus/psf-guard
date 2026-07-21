# Project stack previews

PSF Guard can build an on-demand integration directly from the image grid.
This is a fast visual answer to “what does this project/channel look like so
far?”, with the grading and registration evidence kept beside the result. It
is deliberately labeled **Uncalibrated stack preview**: it is not a replacement
for a calibrated science or final-processing workflow.

![A three-frame B-channel stack preview in the project grid](stack-preview.png)

## Build a preview

Open one project in the image grid and choose **Build stack previews** to build
every current target/channel group, or **Build channel** on one card to test
only that group. Once a result exists, the corresponding actions become
**Rebuild current set** and **Rebuild channel**. An individual rebuild replaces
only that channel's remembered result; the other channel cards remain intact.

- A multi-selection of two or more images is the input when one exists.
- Otherwise the current visible set is used, including the status, channel,
  date, target, and search filters shown above the grid.
- The server always separates inputs by exact Target Scheduler target and
  filter/channel. It never combines different targets or filters.
- **Accepted only** removes Pending frames. By default both Accepted and usable
  Pending frames are eligible.

The build runs in the background and the panel polls its status. Different
target/channel groups are processed sequentially. Only one stacking job runs
in the PSF Guard process at a time, even when the server hosts multiple
databases, so full-frame accumulator buffers cannot multiply unexpectedly.
Cards are capped at two columns on wide displays so the inspection preview does
not become excessively wide.

PSF Guard remembers the last successful preview for every target/channel in the
project cache and restores those cards after navigation, page reload, or server
restart. Each card retains the exact input image IDs and scheduler grades used
to build it. The card is marked **Out of date**—without hiding the usable older
preview—when the current filter/selection changes the image set, an image is
accepted/rejected/pended, or the **Accepted only** policy changes. A failed
rebuild never replaces the last successful result.

## Frame selection and admission

PSF Guard owns project policy; Seiza owns image registration and integration.
Before handing frames to Seiza, PSF Guard excludes:

1. images marked Rejected in the scheduler database;
2. Pending images when **Accepted only** is enabled; and
3. images for which the current sequence analysis has a `regrade_reason`,
   including confirmed cloud/obstruction, off-target, tracking-loss, and
   corroborated no-solve decisions.

The highest-scoring remaining frame becomes the immutable reference. The other
eligible frames are offered to Seiza in acquisition order. Seiza decodes the
linear FITS samples, debayers when required, performs global normalization,
registers each source to the reference, applies its overlap/RMS/scale/rotation
admission gates, and accumulates accepted samples with online delta-sigma
rejection.

Expand **Frame decisions** to audit what happened. Each result retains the
PSF Guard quality score and disposition. Accepted frames also report matched
stars, registration RMS, registration drift, overlap, and integrated-sample
fraction; excluded or rejected frames retain their reason.

Choose **Inspect full size** to open the native-resolution integration in PSF
Guard's image inspector. It uses the same controls as individual images: scroll
to zoom, drag to pan, **F** or **0** to fit, and **1** for one image pixel per
screen pixel. The full-size stretched PNG is loaded only when the inspector is
opened, so the project grid continues to use the smaller screen preview.

![The native-resolution stack in PSF Guard's pan and zoom inspector](stack-preview-inspection.png)

Choose **Download linear FITS** on the card or in the inspector to retrieve the
full-resolution floating-point integration from the cache. The FITS is
unstretched and retains the reference frame's supported WCS headers plus
Seiza's accepted/rejected frame counters, making it suitable for inspection or
as an input to a separate processing workflow.

![Frame-by-frame stack admission details](stack-preview-decisions.png)

## Color previews from channel stacks

Once one target has completed mono stacks for **L/R/G/B** or **H-alpha/OIII**,
the grid adds a **Combine channel stacks** section. Color generation is a
separate on-demand job: rebuilding or changing a color palette never changes
the mono integrations or their admission evidence.

- **RGB** requires one unambiguous Red, Green, and Blue stack.
- **LRGB** requires one unambiguous Luminance, Red, Green, and Blue stack.
  Luminance supplies the output luminance while Seiza retains the RGB
  chromaticity.
- **Narrowband** requires H-alpha and OIII. HOO and Foraxx HOO are then
  available. Adding SII enables SHO, SOH, HSO, HOS, OSH, OHS, and Foraxx SHO.
- The palette picker is part of the cache key. Previously generated palettes
  remain available, and selecting another palette builds or restores its own
  artifact.

PSF Guard recognizes the ordinary short and long filter names (`L`, `Red`,
`Ha`, `H-alpha`, `OIII`, `SII`, `O3`, and `S2`) plus descriptive names such as
`Red`, `H-alpha`, and `OIII` as distinct tokens in vendor labels. It
deliberately does not guess when two stacks map to the same role or when a
multi-band filter name is ambiguous. Rename the Target Scheduler filters to
make those roles explicit before building color.

Each non-reference stack is registered to R for RGB, L for LRGB, or H-alpha
for narrowband, using the same bounded Seiza star/similarity registration used by
the Seiza color CLI. Seiza independently percentile-normalizes the channels
for a useful quick look, then performs the selected composition. Direct RGB, LRGB,
HOO, and three-filter palettes remain linear-light; their PNG receives a
display-only stretch. Foraxx works on display-prepared channels, so its PNG is
not stretched a second time. The RGB floating-point FITS records `COLORSPC`,
`SEIZACLR`, and `SEIZATRF` (`LINEAR` or `DISPLAY`) and preserves supported WCS
cards from the reference stack.

![LRGB and selectable Foraxx narrowband previews built from cached channel stacks](stack-color-previews.png)

Color cards retain the compact loading/status strip while channels are read,
registered, composed, and rendered. **Inspect** opens the same native-size
pan/zoom inspector as a mono stack. **FITS** downloads the full RGB result for
further processing. A color result is marked **Out of date**—but remains
viewable—when any source channel stack is rebuilt, a cached artifact goes
missing, or the Seiza/color-processing cache version changes.

## Output, caching, and invalidation

Each group produces a display-stretched PNG no larger than 2400 pixels on its
longest side, a native-resolution stretched PNG for interactive inspection,
and an unstretched, source-resolution, 32-bit floating-point FITS. A JSON
provenance manifest describes the job. Seiza sees the original star profiles
during integration, and its incremental accumulator keeps memory bounded
independently of frame count. A conservative memory estimate is checked against
the server worker policy before integration starts. Full-size PNGs and FITS
downloads stream from disk rather than buffering the full artifact in server
memory.

Artifacts live below the database cache directory:

```text
<cache>/<database>/stack-previews/<job-id>/
  manifest.json
  group-0.png
  group-0-original.png
  group-0.fits
  group-1.png
  group-1-original.png
  group-1.fits
<cache>/<database>/stack-previews/latest-project-<project-id>.json
<cache>/<database>/stack-previews/color/<color-job-id>/
  manifest.json
  preview.png
  preview-original.png
  color.fits
<cache>/<database>/stack-previews/color/latest-project-<project-id>.json
```

The content-addressed job ID includes the database/project, exact ordered
inputs and grouping, grades, quality scores and regrade reasons, source path
fingerprints, an explicit PSF Guard cache-policy version, Seiza stacking
revision, stretch parameters, and preview format. Repeating an unchanged
request loads the persistent result. A rebuild bypasses that lookup and
atomically replaces the PNG, FITS, and manifest. The per-project latest index
is also written atomically and is updated only for successfully completed
groups. Each run receives a distinct artifact revision in its download/display
URLs so clients cannot mistake an immutable cached response for the rebuilt
output.

## Deliberate limits

- No bias, dark, or flat masters are applied in this first version.
- The retained FITS is still an uncalibrated preview integration, not a final
  science product.
- Color is a visual channel combination, not photometric or
  spectrophotometric calibration. There is no gradient removal, custom mixing
  matrix UI, star removal, mosaic, drizzle, or cross-target integration.
- Satellite predictions and image-detail overlays are not applied to a stack.
  They describe individual shutter intervals, while one preview represents
  several exposures.

## HTTP API

The grid uses these per-database endpoints:

```text
POST /api/db/{db}/projects/{project}/stack-previews
GET  /api/db/{db}/projects/{project}/stack-previews/latest
GET  /api/db/{db}/projects/{project}/stack-previews/{job}
GET  /api/db/{db}/stack-previews/{job}/{group}/preview[?size=screen|original]
GET  /api/db/{db}/stack-previews/{job}/{group}/fits
GET  /api/db/{db}/projects/{project}/stack-previews/color
POST /api/db/{db}/projects/{project}/stack-previews/color
GET  /api/db/{db}/projects/{project}/stack-previews/color/{job}
GET  /api/db/{db}/stack-previews/color/{job}/preview[?size=screen|original]
GET  /api/db/{db}/stack-previews/color/{job}/fits
```

The POST body is `{ "image_ids": [...], "accepted_only": false, "force":
false }`. Status responses contain the group counters, captured image/grade
snapshot, and complete per-frame decision records used by the UI. The latest
endpoint returns the durable last-successful result for each target/channel.
The color catalog reports role/palette availability and durable results. Its
POST body is `{ "target_id": 42, "kind": "rgb", "force": false }`,
`{ "target_id": 42, "kind": "lrgb", "force": false }`, or
`{ "target_id": 42, "kind": "narrowband", "palette": "foraxx-hoo",
"force": false }`.
