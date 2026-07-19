# Astrometry Quality Analysis and Grading Plan

Status: **Implemented in `codex/astrometry-quality-analysis`; calibration follow-up remains**
Owner: psf-guard maintainers
Last updated: 2026-07-18
Baseline: PSF Guard main at `14c63b8` (on-demand Seiza plate solving)

## 1. Goal

Use pixel-derived plate solutions together with the existing sequence,
occlusion, photometry, guiding, and grading systems to identify:

- frames captured far from their intended Target Scheduler field;
- frames where pointing jumps or progressively drifts, including lost
  tracking;
- frames whose pixels cannot be plate solved, as a useful but explicitly
  confidence-weighted quality signal; and
- failures corroborated by star loss, localized obstruction, clouds, trails,
  or guiding metadata.

The feature explains what happened, preserves provenance, and lets users
review the evidence before grades change. A missing catalog, unsupported image,
or solver configuration error is never mistaken for a bad exposure.

## 2. Existing foundation

Main already provides:

- on-demand Seiza analysis at
  `GET/POST /api/db/{db_id}/images/{image_id}/astrometry`;
- embedded-WCS, hinted, and blind solution modes;
- solved center, WCS, footprint, match count, RMS, intended-target offset,
  target containment, and edge margin;
- successful-solve persistence under the per-database astrometry cache;
- restart-persistent spatial and photometric metrics in
  `spatial_metrics.json`;
- sequence scoring and issue classification; and
- explicit batch grading with undo/redo in the Sequence view.

This work should extend those seams rather than introduce a second solver or a
parallel grading model.

## 3. Invariants

- Keep intended coordinates, FITS hints, mount coordinates, solved WCS, and
  robust sequence references as distinct values with source provenance.
- Only a fresh pixel-derived hinted or blind solution is plate-solve evidence
  for grading. Embedded WCS is useful positional evidence, but may be stale and
  is not proof that the current pixels solve.
- Catalog-only matches are not pixel evidence.
- Do not write derived analysis into Target Scheduler tables or its metadata
  JSON. Derived state remains in PSF Guard-owned caches.
- Do not automatically reject a stable framing offset, an entire sequence that
  cannot be solved, or a frame whose failure is caused by missing resources.
- Preserve manual grades and existing rejections unless the user explicitly
  selects a supported overwrite operation.
- Keep astrometry, catalog association, sequence pointing, and future
  back-cataloging as separate provenance-bearing layers.

## 4. Acquisition context and intended-center sources

Add a read-only, schema-adaptive acquisition context query. The current PSF
Guard target model does not retain all fields needed for framing analysis.

```text
AcquisitionContext
  image_id, target_id, project_id
  exposure_id?, session_id?
  expected_framing?
  project_is_mosaic?
  guiding_rms_arcsec?, guiding_ra_arcsec?, guiding_dec_arcsec?
  pier_side?, rotator_position_deg?, rotator_mechanical_position_deg?

ExpectedFraming
  ra_deg, dec_deg
  coordinate_epoch
  rotation_deg?, roi_percent?
  source, provenance
```

Current Target Scheduler schemas store target RA, Dec, epoch, rotation, and ROI
on the target record. Per-image `ImageMetadata` supplies useful capture context
such as session ID, guiding values, rotator position, and pier side. Projects
also identify mosaics, and acquired images retain an exposure-plan ID. Read
these fields when present, while tolerating older or future schemas in which
some framing data is absent or encoded differently.

### 4.1 Intended-center precedence

Use the first authoritative source available:

1. Target Scheduler target fields or equivalent target metadata, including
   epoch, rotation, and ROI;
2. capture-specific intended target coordinates, if a TS schema provides them;
3. a FITS object/target coordinate, as advisory-only intent; and
4. a robust cluster of solved centers, for relative drift only.

A PSF Guard-owned per-target/panel override remains a future extension. The
implemented query is schema-adaptive and uses authoritative TS fields and
metadata without mutating the scheduler database.

Mount/telescope RA and Dec are solver hints, not intended-target coordinates.
Do not silently promote them. Convert a supported target epoch to the solver's
coordinate frame before measuring absolute offsets. When the epoch is absent
or unsupported, retain the coordinates for display but abstain from automatic
absolute-position grading.

For mosaics, each target/panel's own expected center is authoritative. The
project mosaic flag, target ID, exposure-plan ID, and session ID help prevent
different panels or deliberate reframing steps from being treated as drift.

## 5. Unified quality scan

Add a sequence-oriented scan endpoint rather than requiring users to run an
occlusion scan and then solve every image individually:

```text
POST /api/db/{db_id}/analysis/quality-scan
  { target_id, filter_name?, force_spatial?, force_astrometry? }

GET /api/db/{db_id}/analysis/quality-scan
  stage, total, processed
  spatial_done, solve_done
  solved, failed_image_quality, failed_capability, skipped
  current_file?, errors[]
```

The orchestrator should:

1. resolve the FITS file and load acquisition context;
2. reuse valid spatial/photometric results or compute missing results;
3. reuse a valid astrometry attempt or perform the pixel solve;
4. persist the two result families in their existing, separate caches; and
5. run sequence pointing analysis after the target/session group is complete.

Do not run independent full-frame scan pools concurrently: that doubles FITS
I/O and peak memory. Continue using `plan_workers`; spatial work may remain
parallel while plate solving initially respects the per-database solve mutex.
A later optimization may share a decoded image between detectors, but only
after confirming that Seiza detections can preserve the HocusFocus-derived
spatial and photometric behavior.

## 6. Structured solve attempts and negative caching

Replace string-only failure interpretation with a persisted attempt record:

```text
SolveAttempt
  outcome
  modes_attempted[]
  detected_star_count?
  matched_star_count?
  rms_arcsec?
  duration_ms
  source_fingerprint
  solver/catalog/index/detector fingerprints
  attempted_at
  error_detail?
```

Outcomes:

- `solved`;
- `no_match`;
- `insufficient_stars`;
- `decode_error`;
- `unsupported_image`;
- `resource_unavailable`;
- `cancelled`; and
- `internal_error`.

Persist deterministic pixel-quality failures only when the image decoded and
the requested solver resources were available. Resource, configuration,
missing-file, cancellation, and internal failures are operational failures,
not image-quality signals. Cache fingerprints must include the source file,
Seiza version, star catalog, blind index, and detector/solver settings. `force`
retries any outcome.

If every comparable frame in a group is unsolved, report a capability or
unsupported-scale warning at sequence level and abstain from grading. A failed
frame between successfully solved neighboring frames at the same expected
field and scale is materially stronger evidence.

## 7. Pointing and tracking analyzer

Add a separate `PointingQuality` sub-result to sequence output. Do not fold it
into the current scalar quality score during validation.

```text
PointingQuality
  verdict: ok | warn | reject_recommended | abstain
  flags[]
  expected_offset_east/north/total?
  reference_offset_east/north/total?
  field_fraction_offset?
  drift_rate_arcsec_per_hour?
  dither_scatter_arcsec?
  solve_attempt
  evidence[]
```

The current single `IssueCategory` cannot faithfully represent simultaneous
cloud, occlusion, and pointing failures. Preserve it for compatibility, but
add a multi-flag evidence list before grading decisions depend on astrometry.

### 7.1 Grouping and reference model

- Group by database, target, expected framing, and observing session.
- Prefer TS `SessionId`; otherwise split on the existing 60-minute gap.
- Use exposure-plan identity where it distinguishes intentional framing.
- Do not split pointing analysis by filter unless scale or framing differs.
- Work in a tangent plane derived from WCS so RA wrap and polar fields remain
  correct.
- Discover robust solved-center clusters before fitting drift. Stable mosaic
  panels, meridian-flip recentering, and deliberate framing changes become
  separate segments rather than outliers.
- Within a segment, estimate a robust median center, dither envelope (MAD),
  and robust time slope. A transient excursion that returns is a jump; a
  sustained monotonic movement is drift.

### 7.2 Initial flags

- `off_target`: the authoritative target lies outside the solved footprint
  with margin, or the absolute offset is a very large fraction of the field.
- `pointing_jump`: a frame is far from both neighboring solved frames and the
  robust segment center, followed by a return.
- `tracking_drift`: a robust monotonic trend exceeds both the dither envelope
  and a field-fraction threshold.
- `stable_offset`: a whole segment is consistently displaced from the
  intended center; warn at sequence level because this may be deliberate.
- `solve_failed`: a deterministic pixel solve failure.
- `low_solve_confidence`: low matched-star count or high residual; advisory.
- `tracking_shape`: severe eccentricity or guiding excursions corroborate a
  tracking failure even when the frame center itself does not move.

Thresholds must combine an angular floor, a fraction of the shorter field
dimension, and robust sequence scatter. During validation, expose thresholds
in diagnostics rather than treating provisional numbers as settled defaults.
A reasonable starting experiment is warning beyond `max(6*MAD, 5% field)` and
reject recommendation beyond `max(10*MAD, 20% field)`, with target-outside-
footprint as an independent strong signal.

## 8. Evidence fusion and grading policy

Astrometry verdicts should combine independent signals rather than make
`solve_failed` synonymous with `Rejected`.

| Evidence | Default action |
| --- | --- |
| Authoritative target clearly outside a confident pixel solution | Reject recommendation |
| Large isolated pointing jump, corroborated by adjacent solutions | Reject recommendation |
| Sustained tracking drift beyond field/scatter thresholds | Reject recommendation for affected tail |
| Solve failure plus obstruction, cloud, star-collapse, or severe tracking evidence | Reject recommendation |
| Isolated solve failure with otherwise healthy spatial/photometric metrics | Warn/select only |
| All comparable frames fail to solve | Sequence capability warning; no grade |
| Stable solved offset or deliberate framing cluster | Warn; no automatic grade |
| Embedded WCS without a fresh pixel solve | Display/advisory; no automatic grade |
| Missing catalog/index, decode support, or file | Operational error; no grade |

The existing occlusion/cloud rules may still reject independently. Pointing
evidence is another reason, not a replacement for those rules.

## 9. Sequence UI and grading workflow

Replace or extend **Scan Occlusion** with **Scan Quality**, showing stage-aware
progress. The Sequence view should add:

- east/north solved-center scatter with expected and robust-reference
  crosshairs;
- target-distance, drift residual, and solve-quality time series;
- per-frame solve state, field-fraction offset, match count, and RMS;
- selectors for Off target, Tracking lost, Unsolved, Corroborated failures,
  and existing Clouded/Obstructed categories; and
- a **Reject Recommended** review dialog listing every image, proposed reason,
  confidence, and evidence before writing grades.

Use server image IDs for UI grading. Preserve existing rejections and manual
grades by default. Reuse the existing undo/redo mechanism. Implemented reasons:

- `[Auto] Astrometry: Off target - ...`
- `[Auto] Astrometry: Tracking lost - ...`
- `[Auto] Quality: Plate solve failed + occlusion - ...`

The CLI `screen-fits --regrade-db` path supports `--dry-run`, structured
JSON/CSV output, and the existing basename-plus-timestamp guard when matching
files from a raw directory to database rows. It runs fresh pixel solves before
the shared sequence grader, so UI and CLI regrading use the same evidence.

## 10. Delivery phases

### Phase 1 — Contracts and acquisition context

- Add schema-adaptive framing/capture queries and typed provenance.
- Handle target epoch, rotation, ROI, mosaic, exposure, session, guiding,
  rotator, and pier-side fields when present.
- Define solve-attempt and pointing-quality API contracts.
- Add pure tests for coordinate conversion and intended-source precedence.

### Phase 2 — Batch scan and failure semantics

- Persist structured successful and deterministic failed attempts.
- Implement cache fingerprint invalidation and forced retry.
- Add the unified scan endpoint and stage-aware progress.
- Keep scanning read-only; grading writes happen only through the explicit UI
  review action or `screen-fits --regrade-db`.

### Phase 3 — Sequence pointing analysis

- Implement session segmentation, clustering, dither envelope, jump detection,
  robust drift fitting, and footprint-based off-target detection.
- Merge pointing results into sequence and image-quality APIs, reduce the
  composite score for solved pointing failures, and preserve score behavior
  when astrometry has not been scanned.

### Phase 4 — Review UI

- Add plots, badges, evidence details, selectors, and recommendation preview.
- Validate interactions alongside existing occlusion/cloud selections and
  undo/redo.

### Phase 5 — Real-sequence calibration

- Measure clean stationary, dithered, mosaic, meridian-flip, wrong-field,
  tracking-loss, clouded, and obstructed sequences.
- Tune thresholds and define confidence requirements from observed data.
- Document known abstention cases and false-positive boundaries.

### Phase 6 — Guarded grading

- Enable explicit user-confirmed batch rejection and CLI dry-run/write-back.
- Preserve manual and already-rejected grades.
- Emit structured reason/evidence for auditability.

### Phase 7 — Performance follow-up

- Profile shared FITS decode and detector reuse.
- Parallelize solving only within measured CPU/RAM budgets.
- Consider rig-signature baselines for stable offsets after sufficient data.

## 11. Validation matrix

Unit and property tests:

- stationary and normally dithered sequences;
- linear drift, single wrong-field jump, and return-to-field behavior;
- stable intentional offset and stable mosaic panels;
- meridian flip/recenter step;
- RA wrap, high declination, and target-outside-footprint geometry;
- isolated solve failure on a healthy frame;
- solve failure combined with cloud/occlusion/star collapse;
- whole-session unsolved behavior;
- embedded WCS excluded from automatic grading;
- unsupported/non-J2000 target epoch abstention; and
- multi-signal issue preservation.

Cache/API tests:

- restart persistence, source and resource invalidation, and forced retry;
- missing catalogs/indexes, unsupported images, and partial scan errors;
- progress accounting and scan deduplication; and
- no Target Scheduler database mutation during analysis.

Integration and E2E tests:

- a real solvable FITS fixture plus controlled WCS/center variants;
- clean, mosaic, tracking-failure, wrong-target, cloud, and obstruction data;
- progress UI, plots, selectors, review reasons, reject, and undo; and
- grading writes only for the reviewed image IDs.

## 12. Deferred options

- Shared star detections between HocusFocus metrics and Seiza solving, pending
  accuracy validation.
- A persistent PSF-owned user framing override editor.
- Automatic rig/session stable-offset baselines.
- A raw-directory astrometry screening command independent of TS databases.
- Back-catalog construction from solved FITS directories; it remains a
  separate feature and storage model.
