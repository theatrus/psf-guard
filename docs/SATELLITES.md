# Satellite track prediction

PSF Guard can identify satellites predicted to cross a solved image during
its actual exposure. Open an image, find **Satellite tracks**, and choose
**Identify satellite tracks** (or press `T`). The resulting overlay draws the
clipped orbital path inside the sensor, searches the nearby FITS pixels for a
matching linear trail, and labels the candidate with the satellite name and
NORAD identity supplied by the orbital catalog.

## Evidence boundary

Satellite results begin as orbital predictions: “which cataloged objects
should cross this solved footprint between shutter open and shutter close?”
PSF Guard then runs a constrained pixel-path alignment around each projected
track. A match says that a linear brightness feature follows the nearby path;
it does not prove that the cataloged object caused the feature. PSF Guard keeps
these evidence types separate from:

- the pixel-derived WCS used to project the path;
- catalog-only coordinate association;
- star/PSF, occlusion, cloud, and photometric measurements from pixels.

The dashed risk-colored line is always the unmodified orbital prediction. A
solid green line is the independently fitted pixel path. Responses distinguish
`predicted_with_pixel_alignment`, `predicted_pixel_checked`, and
`predicted_not_pixel_detected`, while retaining the exact WCS, FITS source
fingerprint, exposure/site provenance, orbital-element source/state, alignment
version, and Seiza dependency versions used to compute the result.

The shared `seiza-satellites` matcher downsamples the image once to at most
2,048 pixels on its long axis, estimates local noise, and performs a
coarse-to-fine matched-filter search in a narrow normal corridor around the
full clipped prediction polyline. A detection requires at least 2.0 sigma
robust line contrast, 65% path continuity, and 50% usable sample coverage.
Tracks without enough in-frame sideband evidence return `not_evaluated`
instead of being treated as a clean non-detection. The serialized result
includes its actual search radius, offsets, angle delta, physical-ADU
contrast, contrast significance, continuity, coverage, and aligned polyline
segments. It does not run a full-frame line search or invent an identity for
an unrelated trail.

## Required FITS metadata

Prediction requires all three inputs:

1. A solved pixel WCS. The on-demand action will run the existing hinted/blind
   solver if no valid solution is cached.
2. UTC shutter timing, in precedence order: explicit `DATE-BEG`/`DATE-OBS`
   through `DATE-END`; `DATE-AVG` plus `EXPTIME`/`EXPOSURE`; or a start time
   plus `EXPTIME`/`EXPOSURE`. Using `DATE-AVG` centers the interval on the
   writer's measured midpoint, avoiding assumptions about whether a filename
   timestamp represents shutter open or readout completion.
3. Observer latitude and longitude: `SITELAT`/`SITELONG`, `LAT-OBS`/
   `LONG-OBS`, or `OBSGEO-B`/`OBSGEO-L`. Altitude comes from `SITEELEV`,
   `SITEELEVATION`, `ALT-OBS`, or `OBSGEO-H`; missing altitude safely defaults
   to sea level.

Longitude is normalized east-positive to −180…180 degrees. A missing time,
site, WCS, or usable orbital snapshot causes the satellite analysis to
abstain; it does not turn missing evidence into a clean-frame claim.

## Orbital-element cache

The explicit on-demand action chooses the orbital source from the exposure
time through `seiza-satellites::OrbitalCatalogSource`; PSF Guard does not own a
parallel age cutoff or provider list. Recent images use CelesTrak's active-
satellite catalog. Historical images try a nearby durable cache entry, the
content-addressed Seiza rolling mirror, and finally the public IAU SatChecker
endpoint `/tools/tles-at-epoch/?epoch=<julian-date>&format=txt`, using the
shutter midpoint. A nearby validated historical response is reused for the
same observing night rather than issuing another large query.

Current and historical responses share one durable cache. They remain
available for re-tracing until that cache reaches its 5 GiB default upper
bound; then the oldest downloads are pruned while the newest is always kept.
Historical provenance records the provider, requested epoch, and download time.
Cache-only quality scans and regrades never call either network service.
Shared orbital data lives under `<cache>/satellites/`, with retrieval, locking,
validation, and pruning handled by `seiza-satellites`.
The mirror schema, twice-daily publisher, S3 transaction, retention, and
backfill procedure are documented in the
[Seiza satellite mirror runbook](https://github.com/theatrus/seiza/blob/main/docs/SATELLITE_MIRROR.md).

For reproducible or offline work, set `astrometry.satellite_elements` in the
JSON registry to a local OMM JSON or TLE file. Relative paths resolve below
`astrometry.data_dir`:

```json
"astrometry": {
  "data_dir": "/var/lib/psf-guard/seiza",
  "satellite_elements": "active.json"
}
```

Per-image results are written atomically to
`<cache>/<db-slug>/satellites/<image-id>.json`. They carry the exact orbital
payload SHA-256 and are accepted only when the FITS fingerprint, exact WCS,
Seiza version, seiza-satellites version, and pixel-alignment version still
match.

## Bright-trail risk and grading

Track colors express a conservative heuristic:

- **cyan / low**: a crossing is predicted, but illumination/geometry does not
  suggest a bright trail;
- **yellow / possible**: sunlit and close enough to warrant visual review;
- **red / high**: a longer, close, sunlit path with stronger trail risk.

The 0–1 risk combines sunlight fraction, range, elevation, and clipped path
length. It is deliberately not called magnitude: the active catalog does not
provide a reliable exposure-band brightness model, and attitude/flares can
change observed brightness.

Possible or high orbital risk adds `SatelliteTrailRisk` and caps the frame
score at 0.75 for review. Prediction alone never proposes an automatic
rejection. A high-risk candidate must also have a pixel-aligned trail before
the score is capped at 0.35 and regrade can propose a reason such as:

```text
[Auto] Pixel-aligned bright satellite trail - 1 high-risk candidate(s), risk 0.82; verify overlay
```

The Sequence view still requires the normal per-image review and explicit
confirmation before writing a rejection. Existing rejected grades are not
overwritten.

## Real California Nebula exposures

The screenshots and thresholds were validated on two unmodified 60-second
G-filter exposures from October 2025, not mocked UI fixtures. Their FITS
headers provide the observing site, exposure duration, and `DATE-AVG`, so PSF
Guard uses the header-provided location and centers each shutter interval on
the recorded midpoint. Historical elements came from the
[IAU SatChecker archive](https://satchecker.readthedocs.io/en/latest/tools_tle.html)
near each exposure epoch.

For the brighter frame, Seiza 0.10 solved 101 matched stars at 1.90 arcsec RMS.
`seiza-satellites 0.2` projected four high-risk crossings, but pixel alignment
found only the two trails visible in the frame: **CZ-4B R/B [48624]** at 57.8
sigma and **STARLINK-3093 [49141]** at 4.2 sigma. Their fitted paths are about
30 and 76 sensor pixels from the raw orbital projections, with more than 98%
usable-path coverage. The other two
predictions have low contrast and continuity and remain prediction-only.

The preceding night's fainter frame solved with 102 matched stars at 1.96
arcsec RMS. Of three predicted crossings, only **STARLINK-5450 [54778]**
matches the pixels: 5.8 sigma, 99.7% continuity, and roughly a 43-pixel normal
offset. Both dry-run regrade checks reject the affected image for pixel-aligned
evidence. Predictions without a pixel match remain warnings. Names continue
to link to an external satellite information page and remain candidate
associations rather than asserted identities.

| Solved image and on-demand identifier | Sequence score and recommendation |
|:--:|:--:|
| ![California Nebula frame with dashed orbital predictions and solid green pixel-aligned satellite trails](satellite-california-overlay.png) | ![California Nebula frame selected in Sequence Analysis with a pixel-matched satellite rejection recommendation](satellite-california-sequence.png) |

## Background and CLI behavior

Quality scans and `screen-fits --regrade-db` are cache-only consumers: they
never download orbital data. If a configured or previously downloaded
snapshot exists, they compute and persist exposure predictions alongside the
fresh plate solution. Otherwise satellite grading simply abstains.

When CLI and server share the default `./cache`, no extra option is needed. If
the server uses another cache root, give `screen-fits` the same path:

```bash
psf-guard screen-fits /path/to/lights \
  --regrade-db my-db --cache-dir /var/cache/psf-guard --dry-run
```

Review the dry run, then repeat without `--dry-run` to apply supported
recommendations.
