# Export ("take out") Plan

> **Implemented for light frames; retained as the design record.** Calibration
> frame export remains deferred. See the main `README.md` for the current
> workflow.

Get non-rejected lights out of the graded library and into a stacking
pipeline — as a folder (CLI) or a download (server). Rejects are **never**
exported; ungraded (Pending) frames are opt-in.

Status legend: ☐ todo · ☑ done

## Layout (WBPP-style) ☑

```
<dest>/<target>/LIGHT/<filter>/<basename>.fits     ← implemented
<dest>/<target>/FLAT/<filter>/…                    ← reserved (calibration)
<dest>/DARK/<exposure>_G<gain>/…                   ← reserved
<dest>/BIAS/…                                      ← reserved
```

PixInsight WBPP and Siril auto-group cleanly from this. Target/filter names
are sanitized to single path components.

## Core — `src/commands/export/mod.rs` ☑

- `plan_export`: Accepted (+ Pending with `include_pending`) via
  `query_images`; substring project/target filters (CLI) and exact-id filters
  (server); optional exact filter-name restriction. Files resolved by basename
  through `DirectoryTree::build_multiple`; missing files reported per-image,
  never fatal. Destination collisions get numeric suffixes. Deterministic
  ordering.
- `execute_plan`: copy (default) or hardlink (`--link`, cross-device falls
  back to copy); skip when the destination exists with matching size —
  re-running after a new session only adds new subs. `--dry-run` counts
  without writing.
- `FrameKind { Light, Flat, Dark, Bias }` on every `ExportItem`: the
  calibration matcher only has to emit more items; planning, placement,
  idempotency, and archive streaming are already per-item.

## CLI ☑

`psf-guard export <slug-or-path> --dest <dir> [--include-pending]
[--project S] [--target S] [--filter F] [--link] [--dry-run]
[--image-dirs a,b] [--registry P]` — DB opened READ_ONLY. Verified on real
frames: 0.46 GiB hardlinked instantly, second run fully idempotent.

## Server ☑

`GET /api/db/{id}/export?project_id&target_id&include_pending&filter_name` —
streams a **store-mode zip** (FITS doesn't compress; store streams at wire
speed with zero server staging) via `async_zip` feeding a duplex pipe.
Entries use the same WBPP layout. Read-only, so not management-gated. A
mid-stream file error truncates the download (logged) rather than silently
succeeding partially. Covered by `export_streams_zip_of_non_rejected_lights`.

## UI ☑ (minimal)

"⬇ Export" links on Overview project and target rows (shown when accepted
frames exist) pointing at the streaming endpoint.

## Calibration frames — designed, not yet implemented ☐

We have no first-class index of flats/darks, but two real signals exist:

1. **`flathistory`** (TS table): per light-session rows with `filterName`,
   `gain`, `offset`, `bin`, `rotation`, `roi`, `flatsTakenDate`,
   `lightSessionId` — tells us *which flat session belongs to which lights*,
   but stores no file paths.
2. **FITS headers**: N.I.N.A. writes `IMAGETYP` FLAT/DARK/BIAS; the import
   header scanner (`commands::import::headers`) already extracts everything
   needed to match (filter, gain, offset, binning, exposure, CCD temp, dates).

Planned matcher (follow-up PR):
- Scan configured image dirs plus `--calibration-dirs` for calibration-typed
  FITS (header scan is parallel and cheap).
- **Flats**: match exported lights by (filter, gain, offset, bin, rotation
  tolerance), preferring the session `flathistory` points at, else nearest
  `flatsTakenDate` to the light session.
- **Darks/bias**: match by (exposure, gain, offset, bin) with ±2 °C setpoint
  tolerance; darks land in the shared `DARK/<exposure>_G<gain>/` tree.
- Emit as `FrameKind::Flat/Dark/Bias` items; report per-group "no matching
  flat/dark" so gaps are visible instead of silent.
