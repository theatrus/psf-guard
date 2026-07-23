# Astrometry quality grading

PSF Guard's **Scan Quality** workflow combines fresh Seiza plate solutions
with the existing sequence, occlusion, photometry, guiding, and grading
signals. It identifies images that were captured away from the intended
Target Scheduler field, sequences that lost tracking, and frames whose pixels
could not be matched to the sky.

![Sequence quality analysis showing ten solved frames with varied quality scores](sequence-quality-astrometry.png)

## Run a quality scan

Open a Target Scheduler target in the **Sequence Analysis** view and press
**Scan Quality**. The background job runs two stages:

1. spatial and photometric screening for clouds, occlusion, transparency, and
   errant light; and
2. a fresh pixel-derived plate solve for each frame.

The older `/analysis/spatial-scan` API remains as a compatibility alias, but
the UI and new integrations use `/analysis/quality-scan`. Results are cached
per database. PSF Guard checks the FITS size and modification timestamp plus
the Seiza/star/blind-index resource fingerprints before reusing solve results.
A same-name replacement file is therefore never graded from stale WCS.

## What is flagged

| Flag | Evidence | Score/regrade behavior |
| --- | --- | --- |
| **Off target** | The authoritative target is outside the solved footprint, or a short/unclustered departure is at least 20% of the short field dimension away | Pointing score becomes 0; total score is capped at 0.20; rejection is recommended |
| **Stable offset** | At least three consecutive solves form a stable deliberate framing cluster while the target remains inside the solved footprint | Advisory only; no score cap or automatic rejection |
| **Pointing jump** | A short run leaves one framing cluster and later returns to it | Total score is capped at 0.30; rejection is recommended |
| **Pointing drift** | A robust Theil-Sen trend within one contiguous framing segment exceeds the field/scatter threshold after detrending | Affected tail frames are capped at 0.30; rejection is recommended |
| **Plate solve failed** | Pixels decoded and the configured solver had enough information, but no field matched (or too few stars were detected) | Modest score reduction; no automatic rejection unless independent cloud, obstruction, or tracking evidence corroborates it |
| **Solve unavailable** | Missing catalogs/index, decode error, unsupported image, cancellation, or internal/resource failure | Operational error only; no quality flag or automatic grade |

Embedded FITS WCS remains useful for display and overlays, but it is not
grading evidence: a quality scan always solves the current pixels. Coordinate-
only catalog association is likewise kept separate from pixel evidence.

Solved centers are clustered before any offset, jump, or drift decision. A
sustained `A → B` reframing step (or `A → B → C` mosaic sequence) becomes a
new stable framing segment instead of making every later frame look off target
or manufacturing a session-wide drift. A short `A → B → A` excursion is a
pointing jump. Drift is fitted separately inside each contiguous segment.

## Where the intended target comes from

PSF Guard reads a schema-adaptive acquisition context without changing the
Target Scheduler database. Current TS target RA/Dec fields are preferred;
equivalent target metadata and capture-specific intended coordinates are
fallbacks when present. Absolute grading only runs for supported coordinate
epochs (J2000/unspecified today).

If no authoritative target is available, solved centers still support
**relative** jump and drift analysis. PSF Guard uses a gnomonic tangent plane
anchored at the first solved frame, which handles RA=0 wrap and polar fields.
It does not label a frame "Off target" without an authoritative target.

## Review and apply recommendations

Use **Select Off Target**, **Select Unsolved**, or **Select Recommended**.
Recommended rejection is deliberately two-step: **Reject Selected** opens a
review listing every image, score, proposed `[Auto]` reason, and evidence.
Nothing is written until **Confirm rejection** is pressed.

![Rejection review showing the astrometry evidence and proposed reason](sequence-quality-review.png)

Existing rejected images remain untouched, and UI actions use the normal
undo/redo stack. Reasons are specific and auditable, for example:

```text
[Auto] Astrometry: Off target - score 0.20; offset 36% of field
[Auto] Astrometry: Tracking lost - score 0.30; pointing jump 420 arcsec
[Auto] Quality: Plate solve failed + image degradation - score 0.30
```

## CLI regrading

`screen-fits` remains database-free unless `--regrade-db` is supplied. With a
database, it first matches each raw FITS frame by **basename and capture time
(within 10 minutes)**, loads that image's intended target, runs fresh pixel
solves, adds satellite predictions when orbital elements already exist in the
selected cache root, and feeds the results through the same sequence grader
used by the UI.

```bash
# Inspect proposed writes first
psf-guard screen-fits /path/to/lights --regrade-db my-db --dry-run

# Apply supported recommendations; already-rejected rows are untouched
psf-guard screen-fits /path/to/lights --regrade-db my-db
```

If the server uses a non-default cache root, pass the same location with
`--cache-dir`. CLI regrading never downloads orbital elements; it abstains
from satellite grading when no configured or cached snapshot exists.

An isolated deterministic no-solve is reported and lowers its score, but is
not written as a rejection. Off-target/jump/drift recommendations and no-solves
corroborated by independent image degradation can be written. Operational
solver errors always abstain.

## API and tuning

```text
POST /api/db/{db_id}/analysis/quality-scan
  { target_id, filter_name?, force?, force_spatial?, force_astrometry? }

GET  /api/db/{db_id}/analysis/quality-scan
GET  /api/db/{db_id}/analysis/sequence?target_id=...&weight_pointing=...
```

The default pointing weight is additive and missing-metric-safe: databases
that have not been scanned keep their previous scores because the remaining
weights are renormalized. Synthetic and API regression tests cover the initial
field-fraction thresholds, deliberate reframing clusters, returned excursions,
and per-segment drift; calibration against additional real rigs and mosaic
panels remains ongoing.
