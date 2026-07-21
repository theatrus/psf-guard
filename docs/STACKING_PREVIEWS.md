# Project stack previews

PSF Guard can build an on-demand integration directly from the image grid.
This is a fast visual answer to “what does this project/channel look like so
far?”, with the grading and registration evidence kept beside the result. It
is deliberately labeled **Uncalibrated stack preview**: it is not a replacement
for a calibrated science or final-processing workflow.

![A three-frame B-channel stack preview in the project grid](stack-preview.png)

## Build a preview

Open one project in the image grid and choose **Build stack previews**.

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

Choose **Download linear FITS** on a ready group to retrieve the full-resolution
floating-point integration from the cache. The FITS is unstretched and retains
the reference frame's supported WCS headers plus Seiza's accepted/rejected
frame counters, making it suitable for inspection or as an input to a separate
processing workflow.

![Frame-by-frame stack admission details](stack-preview-decisions.png)

## Output, caching, and invalidation

Each group produces a display-stretched PNG no larger than 2400 pixels on its
longest side and an unstretched, source-resolution, 32-bit floating-point FITS.
A JSON provenance manifest describes the job. Seiza sees the original star
profiles during integration, and its incremental accumulator keeps memory
bounded independently of frame count. A conservative memory estimate is
checked against the server worker policy before integration starts. FITS
downloads stream from disk rather than buffering the full artifact in server
memory.

Artifacts live below the database cache directory:

```text
<cache>/<database>/stack-previews/<job-id>/
  manifest.json
  group-0.png
  group-0.fits
  group-1.png
  group-1.fits
```

The content-addressed job ID includes the database/project, exact ordered
inputs and grouping, grades, quality scores and regrade reasons, source path
fingerprints, an explicit PSF Guard cache-policy version, Seiza stacking
revision, stretch parameters, and preview format. Repeating an unchanged
request loads the persistent result. **Rebuild** bypasses that lookup and
atomically replaces the PNG, FITS, and manifest. Each run receives a distinct
artifact revision in its download/display URLs so clients cannot mistake an
immutable cached response for the rebuilt output.

## Deliberate limits

- No bias, dark, or flat masters are applied in this first version.
- The retained FITS is still an uncalibrated preview integration, not a final
  science product.
- Channels remain separate. There is no LRGB/SHO combination, mosaic, drizzle,
  or cross-target integration.
- Satellite predictions and image-detail overlays are not applied to a stack.
  They describe individual shutter intervals, while one preview represents
  several exposures.

## HTTP API

The grid uses four per-database endpoints:

```text
POST /api/db/{db}/projects/{project}/stack-previews
GET  /api/db/{db}/projects/{project}/stack-previews/{job}
GET  /api/db/{db}/stack-previews/{job}/{group}/preview
GET  /api/db/{db}/stack-previews/{job}/{group}/fits
```

The POST body is `{ "image_ids": [...], "accepted_only": false, "force":
false }`. Status responses contain the group counters and complete per-frame
decision records used by the UI.
