# Seiza Integration — Design and Implementation Plan

Status: **Phase 0 implemented; managed download and Phase 1 next**
Owner: psf-guard maintainers
Last updated: 2026-07-14
Baseline: PSF Guard 0.4.2; Seiza 0.4.1; seiza-fits 0.1.4

## 1. Goal

Integrate Seiza as PSF Guard's offline astronomy layer so the application can:

1. associate catalog objects with an image from known coordinates, without
   requiring a plate solve;
2. obtain or refine an image WCS and render pixel-positioned annotations;
3. compare the solved field with the intended Target Scheduler target during
   sequence analysis, exposing drift and out-of-target frames; and
4. eventually reconstruct a browsable catalog from a directory of FITS files
   even when no Target Scheduler database is available.

These are related but distinct capabilities. Catalog association is not proof
that an object is visible in the pixels, and a solved center must not replace
the intended N.I.N.A. target coordinate. PSF Guard keeps the original target,
FITS hint, and derived solution as separate values with explicit provenance.

## 2. Non-goals and invariants

- Do not write derived astrometry into the Target Scheduler `metadata` JSON.
- Do not update `target.ra` or `target.dec` from a plate solution.
- Do not present coordinate-only catalog hits as detected objects.
- Do not run a blind solver synchronously when an image-detail page opens.
- Do not load or duplicate large catalogs once per configured database.
- Do not let a future PSF Guard-owned FITS catalog participate in N.I.N.A.-
  specific grade sync or reject-archive commands.
- Do not make pointing classifications affect quality scores or grades until
  thresholds have been validated against real sequences and dithers.

## 3. Seiza 0.4.1 baseline

PSF Guard currently depends on `seiza-fits = 0.1.3`. The integration starts by
updating to the published 0.4 family:

```toml
seiza = "0.4.1"
seiza-fits = "0.1.4"
```

### 3.1 Changes that directly improve this integration

#### Streamed FITS decoding

`seiza-fits` 0.1.4 retains the existing `FitsImage::open` API but streams the
payload into its final typed pixel vector using a bounded conversion buffer.
PSF Guard already enters `seiza-fits` through `src/image_analysis.rs`, so this
is an immediate reduction in peak scan memory before solving is added.

The first dependency PR should bump `seiza-fits`, run the existing FITS and
spatial-scan tests, and profile at least one representative large mono frame
and one OSC frame. No custom compatibility layer is expected.

#### Indexed object catalog v3

Seiza 0.4.1's `SEIZAOB3` object catalog is memory-mapped and contains spatial and
normalized-name indexes. Ordinary cone, footprint, exact-name, and prefix
queries materialize only returned records instead of eagerly decoding the
entire catalog.

This changes the earlier resource plan:

- `objects.bin` can be opened once at process scope and used immediately for
  image-detail queries; no PSF Guard-side spatial index is needed.
- `ObjectHit` and `PlacedObject` own their returned `SkyObject` values, so API
  conversion does not need to preserve a borrow into the mmap.
- `query_region` and `objects_in_footprint` return `Result`; catalog I/O and
  invalid-region errors must be exposed as capability/analysis errors rather
  than silently producing an empty field.
- normal startup performs the lazy bounded open; exhaustive `validate()` is a
  settings/diagnostic action, not a startup requirement.

The PSF Guard API should preserve v3 identity metadata:

```text
id
source
aliases
parent_ids
alternate_ids
alternate_sources
```

Stable source-qualified IDs, rather than display names, become the durable
identity for selected annotations and future imported target associations.

#### Offline name lookup

Object catalog exact and prefix search now resolve primary names, common names,
aliases, and stable IDs. This should power target search/autocomplete and the
review step of back-catalog import instead of adding a second name index.

The optional `SEIZASI1` stellar identifier sidecar resolves identifiers and
names such as TYC, HIP, HR, HD, SAO, FK5, IAU proper names, variables, and WDS
designations. In Seiza 0.4.1 it supports exact and prefix lookup, but not a
spatial cone or footprint query. Therefore:

- use it initially for search, target resolution, and inspecting a known
  stellar designation;
- do not promise a complete named-star overlay from the sidecar yet; and
- add that overlay when Seiza provides a spatial identified-star query, or
  after a separately reviewed PSF Guard join strategy exists.

#### Coherent hosted catalog bundle

Seiza 0.4.1 replaces the split hosted manifests with one complete catalog
bundle at `/data/v2/manifest.json`. It covers solver tiles, the blind index,
stellar identifier sidecar, object and transient catalogs, and minor bodies.
Local downloads remain flat and retain the canonical filenames.

The bundle version and each file's wire format are separate concepts. For
example, a `SEIZAOB3` object file belongs to the hosted v2 bundle; the `3` is
the object file schema, not the bundle version. PSF Guard should expose both
values in diagnostics rather than deriving one from the other.

For PSF Guard-managed data:

- use only the complete v2 manifest as the acquisition authority;
- reject an unsupported, incomplete, or duplicate-file manifest even when the
  user requested only one file;
- verify each selected file's SHA-256 before activation;
- record the exact manifest version plus per-file hashes in capability and
  astrometry cache signatures; and
- never assemble one managed installation from different hosted bundle
  versions.

Selective installation remains useful: catalog-only deployments can download
`objects.bin`, while hinted or blind solving adds the appropriate star data.
The selection still comes from a validated complete manifest. Explicit custom-
built catalogs remain supported as an unmanaged data directory and are
validated through their self-describing file headers.

### 3.2 Relationship to the shared overlay package and existing applications

`@seiza/astro-overlay` 0.1.1 is now the canonical browser overlay geometry and
rendering contract. Its core entry point owns WCS transforms, coordinate-grid
geometry, semantic layers, prominence density, and marker geometry; its React
entry point supplies the SVG-only `AstroOverlay`; and its export entry point
serializes/composites the live overlay for PNG output. PSF Guard should consume
those entry points rather than porting the Seiza-server/Tenrankai SVG code.

PSF Guard still owns HTTP state, progress and errors, toolbar controls,
preference persistence, the transformed raster/SVG container, and application
branding. Its `AstrometrySolutionResponse` is designed as a compatible
superset of the package `OverlaySolution`; projected objects must expose
`prominence` so the shared density selection works without an adapter.

Seiza-server and Tenrankai remain useful references for WCS persistence,
re-projection behavior, and application-level controls, but their local
overlay implementations are no longer source material to copy.

Tenrankai's current hinted-solving path also establishes the preferred scale
policy: try a known pixel scale once with a narrow tolerance, then fall back to
an overlapping ladder ordered by common astrophotography scales rather than
numerically. PSF Guard can improve on its sidecar input by deriving the known
scale from FITS headers and equipment metadata.

Backend adapters must use the Seiza 0.4.1 owned-result and fallible-query APIs.
This avoids embedding an obsolete compatibility layer just to mirror the two
downstream applications.

## 4. Capability ladder

### 4.1 Catalog association without solving

Inputs, in priority order:

1. a valid embedded WCS, which gives an exact polygon footprint;
2. target RA/Dec plus a known or estimated field of view, which gives a cone;
3. target RA/Dec plus a conservative configurable radius when field size is
   unknown.

Use `ObjectCatalog::query_region` and return Seiza's semantics:

- `center_inside`
- `extent_only`
- `distance_from_center_deg`
- `predicted_prominence`
- complete object identity/provenance metadata

The image detail page labels this section **Expected objects in field**. When
only a conservative cone is available, it labels it **Objects near target**.
`predicted_prominence` may order the list but must not be described as a pixel
detection confidence.

An embedded WCS is sufficient to upgrade these hits to exact pixel-positioned
annotations without invoking a solver.

### 4.2 Hinted astrometric solution

When no usable embedded WCS exists, solve using separate hint and expectation
coordinates:

- center hint: FITS RA/Dec first, then database target RA/Dec;
- expected target: database target RA/Dec;
- scale: embedded WCS scale, explicit FITS scale, camera/focal-length-derived
  scale, then the scale ladder proven in Tenrankai.

The centralized FITS astro-header parser must support numeric and sexagesimal
RA/Dec and record which header supplied each value. Pixel scale derivation is:

1. determinant of a valid CD matrix or equivalent WCS;
2. explicit pixel-scale header;
3. `206.265 * XPIXSZ_um * XBINNING / FOCALLEN_mm`;
4. controlled scale ladder.

A known or derived scale is attempted first with approximately 15% tolerance.
The fallback uses Tenrankai's current common-case-first order, each with 35%
tolerance: `2.8, 1.4, 0.7, 5.6, 0.35, 0.19, 0.11` arcseconds/pixel. The rungs
overlap despite their execution order. Record the successful scale source and
the number of failed attempts because each preceding rung is a full solve.

The existing spatial scan already loads the FITS image and retains the 300
brightest HocusFocus stars. Add an adapter to produce brightest-first
`seiza::DetectedStar` values and try that catalog first. Benchmark solving
quality against Seiza's own detector; fall back to the Seiza detector when the
adapter fails or produces insufficient matches.

### 4.3 Blind astrometric solution

Blind solving is an explicit background scan or back-catalog fallback. It is
not an automatic image-detail request because the deep catalog and maintained
blind index are each large and solving is CPU intensive.

The prebuilt blind index is memory-mapped and initialized lazily at process
scope. PSF Guard must not build a whole-sky index during normal application
startup. Missing blind data reduces capability rather than preventing the
server from starting.

### 4.4 Capture-time annotation layers

After static object overlays work, add refreshable layers using the capture
time retained from FITS/Target Scheduler metadata:

- current transients;
- historical transients;
- minor bodies and solar-system objects; and
- RA/Dec grid.

Catalog changes re-project through stored WCS. They do not trigger a new plate
solve.

## 5. Runtime architecture

### 5.1 Process-global catalog state

Add an `AstrometryContext` to `AppState` containing lazy, shared resources:

```text
AstrometryContext
  object_catalog: unavailable | lazy ObjectCatalog
  star_catalog: unavailable | lazy Catalog
  star_identifier_catalog: unavailable | lazy StarIdentifierCatalog
  blind_index: unavailable | lazy BlindIndex
  transient_catalog: optional
  minor_body_catalog: optional
  versions/capabilities
```

Opening and normal queries remain demand-paged. A user-invoked validation
operation calls Seiza's exhaustive `validate()` and reports attribution,
format, file size, and error details.

Catalog resources are global because every registered database uses the same
sky. Per-image derived state remains database-scoped.

### 5.2 Per-database astrometry store

Add an astrometry store to `DatabaseContext`, following the existing
`SpatialMetricsStore` lifecycle and worker/progress policy. Persist it below
`<cache_root>/<db_slug>/astrometry.json` initially. If the record count or
write amplification becomes material, migrate the store to a PSF Guard-owned
SQLite cache without changing the API model.

Each record includes:

```text
image_id
source_fingerprint { canonical path, size, modified time }
analysis_schema_version
seiza_version
catalog/index signatures
status and solve mode
hint coordinate and provenance
expected coordinate and provenance
WCS and quality
static annotation catalog version
computed_at
last_error
```

The image ID and basename are insufficient cache keys. A file replacement at
the same path must invalidate its analysis. Catalog-only results invalidate
when the object catalog changes. A stored WCS remains valid across catalog
updates; only projected annotations are refreshed.

Failures are cached with their fingerprint and solver inputs so background
views do not repeatedly solve a known-bad frame. A forced scan bypasses the
failure cache.

### 5.3 Scan scheduling

Evolve the spatial scan into a shared image-analysis pass rather than adding a
second unbounded FITS reader:

1. cheap header extraction;
2. one streamed FITS decode;
3. statistics and HocusFocus detection;
4. spatial metrics and photometric catalog;
5. optional astrometric solve;
6. atomic persistence of independently versioned results.

Interactive single-image work uses the existing interactive-job guard.
Background scans use the existing worker policy and yield to previews and
interactive analysis.

## 6. API contract

Keep the astrometry response separate from the existing unstructured image
metadata so it can express availability, progress, errors, provenance, and
catalog versions.

```text
AstrometryAnalysis
  image_id
  status: unavailable | catalog_only | solved | failed
  mode: embedded_wcs | hinted | blind | null
  hint_source
  expected_source
  solution: SolutionResponse | null
  catalog_hits: CatalogHit[]
  pointing: PointingResult | null
  source_fingerprint
  computed_at
  error
```

Keep the backend response compatible with `@seiza/astro-overlay`'s
`OverlaySolution` and `OverlayObject` vocabulary:

```text
SolutionResponse
  center_ra_deg
  center_dec_deg
  pixel_scale_arcsec_per_pixel
  matched_stars
  rms_arcsec
  image_width
  image_height
  wcs
  footprint
  objects
  catalog_version
  capture_time

OverlayObject
  stable_id
  name
  common_name
  kind
  mag
  x, y
  semi_major_px, semi_minor_px, angle_deg
  source
  aliases
  parent_ids
  alternate_ids
  alternate_sources
  ra_deg, dec_deg
  prominence
  optional capture-time fields
```

The stable identity/provenance fields are the intentional Seiza 0.4.1 extension
to the existing seiza-server response.

Initial routes:

```text
GET  /api/astrometry/capabilities
POST /api/astrometry/catalogs/validate
GET  /api/db/{db_id}/images/{image_id}/astrometry
POST /api/db/{db_id}/analysis/astrometry-scan
GET  /api/db/{db_id}/analysis/astrometry-scan
```

The scan request selects `catalog_only`, `hinted`, or `blind_fallback`, with a
`force` flag. Missing data returns structured capability information rather
than a generic 500 response.

## 7. Image-detail UI

The first UI increment is a catalog list and requires no solver:

- expected/nearby object name and common name;
- kind, magnitude, angular size, and catalog source;
- extent-only indicator;
- distance from expected center;
- alias/provenance details on expansion; and
- a clear explanation of the coordinate and field-size source.

Once WCS is available, render `AstroOverlay` from
`@seiza/astro-overlay/react` and use the package's core defaults and geometry.
Place it in the same transformed image container as the raster, using full
image dimensions as its `viewBox`, so pan, zoom, and preview-to-original
transitions apply identically. PSF Guard supplies its own layer controls and
can use `@seiza/astro-overlay/export` for annotated PNG output.

Selection and layer preferences should use stable IDs and explicit `source`
values. They must not infer a layer from the display name.

## 8. Sequence pointing analysis

For every solved image compute:

- solved center RA/Dec;
- east/north tangent-plane offset from the intended target;
- angular separation from the intended target;
- expected target pixel location;
- whether the target is in the solved footprint;
- target distance from the nearest image edge;
- east/north offset from a robust sequence reference;
- solve mode, match count, and RMS.

Expose both absolute and relative behavior:

- **distance from target** detects a wrong field or a target leaving the frame;
- **drift from sequence reference** detects progressive pointing movement even
  when the intended framing is an offset or mosaic panel.

Use a robust median reference and robust slope in arcseconds/hour. Dither
steps are high-frequency offsets and must not be classified as progressive
drift. Near the celestial poles, use WCS/tangent-plane projection rather than
only multiplying RA difference by `cos(dec)`.

Initial advisory categories:

- `off_target`: the intended coordinate lies outside the solved footprint,
  with a small configurable margin;
- `pointing_drift`: robust trend or excursion exceeds a configurable angular
  or field-fraction threshold.

The sequence UI adds:

- an east/north scatter plot with target and sequence-reference crosshairs;
- target-distance and drift-residual time series;
- solve-quality indicators; and
- filters/selectors for unsolved, off-target, and drifting frames.

These fields are merged into sequence results after the astrometry cache is
loaded, just as spatial and photometric metrics are today. They remain outside
the composite quality score during the validation phase.

## 9. Back-catalog from FITS directories

Back-catalog construction uses a staged cost funnel:

1. recursively inventory FITS files using canonical path, size, modified time,
   and a stable content-prefix fingerprint;
2. stream headers only and normalize capture metadata;
3. accept valid embedded WCS;
4. hinted-solve files with coordinate/scale clues;
5. blind-solve unresolved files when the capability is installed and enabled;
6. cluster solved centers and overlapping footprints on the sphere;
7. suggest target identity using object v3 stable IDs, exact aliases, and
   prominence-ranked regional hits;
8. group captures into sessions by time, filter, exposure, and geometry; and
9. require a review step for split, merge, rename, and unresolved images.

The object v3 name and spatial indexes materially reduce the amount of custom
back-catalog infrastructure: PSF Guard should persist Seiza stable IDs and the
name that matched, not a separately normalized copy of the catalog.

### 9.1 Source abstraction prerequisite

The current database layer assumes Target Scheduler tables. Before persisting
a disk-only catalog, introduce a repository/source abstraction:

```text
ImageRepository
  NinaRepository
  FitsCatalogRepository
```

The API/UI domain can continue to expose projects, targets, sessions, and
images. The registry gains an explicit source kind, for example:

```jsonc
{
  "kind": "nina" | "fits_catalog"
}
```

This is the point where the registry schema should advance from v2 to v3 with
an atomic migration. A `fits_catalog` entry points to a PSF Guard-owned SQLite
database plus image roots. N.I.N.A.-specific sync/archive commands reject that
source kind explicitly.

## 10. Configuration and data management

Astrometry catalogs are process-global, so their paths belong in a top-level
configuration block rather than under each database:

```jsonc
{
  "astrometry": {
    "data_dir": "/path/to/seiza-data",
    "objects": "objects.bin",
    "stars": "stars-gaia.bin",
    "star_identifiers": "stars-lite-tycho2.ids.bin",
    "blind_index": "blind-gaia16.idx",
    "transients": "transients.bin",
    "minor_bodies": null
  }
}
```

The paths are optional and resolved relative to `data_dir`. Auto-discover the
canonical Seiza filenames and environment conventions used by seiza-server,
then allow explicit overrides. Adding the optional block is backward-
compatible with the current registry parser; the source-kind migration is
reserved for registry v3.

For a PSF Guard-managed directory, persist installation state beside the data:

```text
hosted manifest URL
catalog bundle version
installation/update time
installed filename, size, and SHA-256
```

Runtime capability checks use the installed files and their wire headers. The
recorded manifest state establishes that the files came from one coherent
hosted bundle and supplies stable cache signatures. A custom unmanaged
directory has no bundle version and reports individual file signatures only.

Settings report capabilities independently:

```text
object association       objects.bin
object name search       SEIZAOB3 name index
stellar name search      *.ids.bin
hinted solve             star catalog
blind solve              deep star catalog + matching blind index
dynamic annotations      transient/minor-body data
```

Do not bundle the multi-gigabyte deep catalog or blind index into the binary.
Manual/path-based configuration remains the Phase 0 fallback. Managed fetching
should consume the new `seiza-download` library once released: its
`CatalogManager`, `CatalogSet`, `Dataset`, cache policy, and `DownloadEvent`
API already provide async selective installation from the complete v2
manifest, streaming SHA-256 verification, content-addressed caching, and
progress reporting. PSF Guard should pass the returned paths directly to the
memory-mapped Seiza readers and reserve exhaustive `CatalogBundle::verify` for
explicit validation. It must not combine the legacy unversioned manifest with
v2 files.

## 11. Delivery phases

### Phase 0 — dependency and data-contract foundation

**Implemented 2026-07-14.** The dependency bump, normalized header reader,
global lazy catalog context, registry configuration, capability/validation
routes, and stable response contracts are in place. Exhaustive validation is
explicit, singleton, and runs on the blocking pool; ordinary capability checks
remain bounded. Focused contract tests cover absent/missing/malformed catalogs
plus legacy-v1 and indexed-v3 object catalogs.

- Bump `seiza-fits` to 0.1.4 and verify streamed decoding through PSF Guard.
- Add `seiza` 0.4.1.
- Add centralized FITS astrometry-header parsing and provenance.
- Define shared API types with Seiza 0.4.1 stable object metadata.
- Add global catalog configuration, capability reporting, lazy open, and
  explicit validation.
- Model catalog signatures as hosted bundle version plus per-file hash, or as
  individual signatures for custom unmanaged data.
- Add tests for missing, invalid, legacy-v1, and indexed-v3 object catalogs.

Exit: existing image analysis is unchanged functionally, large FITS opens use
the streamed reader, and the server can accurately report installed Seiza
capabilities.

### Phase 0.5 — managed catalog installation

- Add the released `seiza-download` crate.
- Map PSF Guard capability selections onto `CatalogSet`/`Dataset` values.
- Expose installation status and `DownloadEvent` progress without performing
  surprise network access during image analysis.
- Persist bundle version and returned artifact hashes as catalog signatures.
- Keep explicit custom paths as the unmanaged/offline alternative.
- Cover manifest fetch, verified download, cache hit, offline fallback, and
  corrupt-artifact recovery with a local HTTP fixture in end-to-end tests.

Exit: users can install only the catalogs they need from Settings, every
activated file belongs to one verified bundle, and normal startup remains
network-independent.

### Phase 1 — coordinate-only catalog association

- Query objects from target coordinates and estimated fields.
- Add exact/prefix object search for target selection.
- Add optional stellar designation search from `SEIZASI1`.
- Add the image-detail expected/nearby object list.
- Preserve stable IDs, aliases, hierarchy, and provenance in the API.

Exit: useful offline annotations work without a plate solver and the UI never
implies they were detected in the pixels.

### Phase 2 — persistent WCS pipeline

- Parse and validate embedded WCS.
- Add the HocusFocus-to-Seiza star adapter and detector fallback.
- Implement hinted solving and scale fallback.
- Add the per-DB astrometry store, fingerprints, progress, failure caching,
  and scan routes.
- Persist WCS separately from refreshable annotations.

Exit: supported images produce durable, quality-scored WCS solutions without
blocking the detail page.

### Phase 3 — overlay and dynamic annotations

- Add `@seiza/astro-overlay` and render its React `AstroOverlay` directly.
- Keep PSF Guard-specific layer controls, preference storage, zoom/pan layout,
  and annotated-export branding outside the shared component.
- Project v3 objects using stored WCS.
- Re-project annotations when their catalog version changes.
- Add capture-time transient/minor-body layers and coordinate grid as their
  datasets become available.

Exit: overlays remain aligned across zoom/pan/resolution changes and catalog
refresh does not re-solve the image.

### Phase 4 — sequence pointing

- Merge pointing fields into sequence responses.
- Add target containment, target distance, robust reference, and drift trend.
- Add scatter/time-series UI and advisory classifications.
- Validate against real stationary, dithered, mosaic, drifting, and wrong-
  target sessions before enabling any automated action.

Exit: users can identify frames that drift from a session or leave the target
without conflating those events with ordinary quality degradation.

### Phase 5 — blind solving

- Add lazy prebuilt-index support and explicit blind-fallback scans.
- Add data/capability diagnostics and resource limits.
- Measure solve rate, time, and memory against the supported image-scale range.

Exit: header-poor historical files can be solved in the background when the
required data is installed.

### Phase 6 — FITS back-catalog

- Introduce `ImageRepository` and registry source kinds.
- Add PSF Guard-owned FITS catalog schema and migration-safe persistence.
- Implement discovery, solve funnel, spherical clustering, naming suggestions,
  session grouping, and reconciliation UI.
- Protect all N.I.N.A.-specific commands at the source boundary.

Exit: a directory of FITS files can become a reviewed PSF Guard catalog without
fabricating or mutating a Target Scheduler database.

## 12. Verification strategy

### Rust/API

- Unit tests for sexagesimal/numeric coordinate parsing and pixel-scale sources.
- Object v3 cone/polygon/error mapping and stable metadata serialization.
- Legacy object-v1 compatibility where Seiza supports it.
- Source fingerprint invalidation and WCS/catalog version separation.
- HocusFocus adapter brightness ordering and solve fallback.
- Pointing projection, RA wrap, polar fields, footprint containment, edge
  distance, dither rejection, and robust drift slope.
- Missing catalogs degrade capabilities without preventing server startup.

### Performance

- Compare `seiza-fits` 0.1.3 and 0.1.4 peak RSS on representative frames.
- Confirm one global mmap is reused across multiple configured databases.
- Measure catalog-only detail latency with a cold and warm page cache.
- Measure hinted/blind solve throughput under the existing worker policy.

### Frontend/end-to-end

- Catalog-only wording and provenance.
- Overlay alignment through zoom, pan, and original-image replacement.
- Layer toggles use explicit sources and stable IDs.
- Astrometry scan progress, partial failures, retry, and missing-data states.
- Sequence navigation preserves `db`, `project`, `target`, filter, and session
  query state.
- Pointing plots distinguish dithers, drift, intentional offset, and target
  outside the solved footprint.

## 13. Decisions still requiring evidence

- Whether HocusFocus detections solve as reliably as Seiza detections across
  undersampled, crowded, narrowband, OSC, and partially obscured frames.
- The minimum field information required before a coordinate-only result is
  labeled `in field` rather than `near target`.
- Default advisory drift thresholds in pixels, arcseconds, and field fraction.
- Whether JSON remains adequate for the per-image astrometry store at expected
  catalog sizes.
- The UX and storage rules for intentional mosaic panels and user-supplied
  expected-center overrides.
- Whether identified-star spatial lookup lands in Seiza before the overlay
  phase; if not, named-star overlay remains object-catalog-only in the first
  release.
