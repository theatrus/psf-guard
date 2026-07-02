# CLAUDE.md - Development Notes

## Project Overview

PSF Guard is a Rust CLI utility for analyzing N.I.N.A. Target Scheduler databases and managing astronomical image files. Features N.I.N.A. star detection algorithm, PSF fitting, and React web interface.

## Quick Start

```bash
# Development
cargo fmt && cargo clippy && cargo test

# CLI mode - Run server (supports multiple directories)
cargo run -- server db.sqlite images1/ images2/

# GUI mode - Launch desktop app (with tauri feature)
cargo run --features tauri

# Tauri desktop development
cargo tauri dev

# Build for production
cargo build --release                    # CLI only
cargo build --release --features tauri   # GUI capable
```

## Architecture

### Core Components
- **CLI**: Command-pattern with clap-derive (`src/cli_main.rs`)
- **Tauri Desktop**: Desktop app with server integration (`src/tauri_main.rs`)
- **Database**: SQLite via rusqlite
- **Star Detection**: N.I.N.A. algorithm + HocusFocus detector
- **Web Server**: Axum + embedded React frontend
- **Cache System**: Directory tree + file cache with 5-minute TTL

### Multi-database support (2026-05)
The server can manage many N.I.N.A. scheduler databases at once. Both the
Tauri app and the CLI `server` command read from a shared JSON registry at
the platform config location (`<config>/psf-guard/config.json` by default).

- **Registry**: `src/db_registry.rs` owns the on-disk schema (v2). Each entry
  is `{ id, name, db_path, image_dirs }` where `id` is a user-editable
  URL-safe slug seeded from a hash of the canonical DB path. Migration from
  v1 (`{database_path, image_directories}`) happens on first load and writes
  a `.bak`.
- **Server state**: `AppState.databases` is a `HashMap<slug, Arc<DatabaseContext>>`.
  Each context owns its own SQLite connection, directory tree cache, file
  cache, and refresh mutex.
- **API**: every per-DB endpoint is nested under `/api/db/{db_id}/...`.
  Cross-DB endpoints (`/api/info`, `/api/databases`, and CRUD on
  `/api/databases/{id}`) sit at the top level.
- **Frontend model**: no "active DB" switcher. The Overview merges projects
  and targets across all configured databases, grouping by DB. Scoped views
  (grid, detail, comparison) read `?db=<slug>` from the URL alongside the
  other identifiers; missing `?db=` shows an empty state.
- **Per-DB cache**: artifacts live under `<cache_root>/<slug>/`. Slug rename
  via `PUT /api/databases/{id}` renames the cache subdirectory too so
  previously generated previews carry over.
- **CLI persistence**: `psf-guard server <db> <dirs>` registers the DB into
  the shared registry on first run. Use `--registry /tmp/scratch.json` for
  ad-hoc sessions that should not touch the user's real config.

Implementation tracker and design rationale: [MULTI_DB_PLAN.md](./MULTI_DB_PLAN.md).

### Out-of-tree reject archive (2026-05)
- **Command**: `psf-guard move-rejects --db <slug>` (multi-DB-aware via the
  registry). Moves files marked `gradingStatus = 2` to
  `<image_dir>/<P>/REJECT/<rest>` by default, plus same-stem sidecars
  (`.xisf`, `.json`, `.txt` by default). Idempotent across re-runs. Files
  stay findable in the web UI — the directory tree indexes the REJECT
  subtree and resolves previews by basename. `--dry-run` performs no DB
  writes (the `psf_guard_archive` table is not even created).
- **Restore**: `psf-guard restore-rejects --db <slug>` reverses a move. By
  default restores only rows whose current grade is no longer `Rejected`
  (un-rejected in the UI, to Accepted *or* Pending); `--all` /
  `--image-id` / `--guid` override that. Never overwrites — restores
  beside an occupant with a `.restored[.N]` suffix. Deletes the archive
  row and prunes emptied REJECT dirs (a dir still holding the manifest is
  kept).
- **State**: a `psf_guard_archive` sibling table in the same SQLite file
  records each move, keyed on `acquiredimage.guid` (TS plugin migration
  22). The plan deliberately avoids stamping the upstream `metadata`
  JSON column (TS's `ImageMetadata` DTO drops unknown keys on
  round-trip, see plan §3). Each archive root
  (`<image_dir>/<P>/REJECT/.psf-guard-manifest.json`, one per tree — not
  per leaf) also gets a redundant manifest for disaster recovery.
- **Config precedence**: CLI flags > per-DB `reject_archive` block in
  the registry (`segment_name`, `depth`, `sidecar_exts`) > compiled-in
  defaults (`REJECT`, `1`, `[.xisf, .json, .txt]`).
- **Schema requirement**: TS plugin schema v22+ (the `guid` column).
  Older DBs are refused with an actionable error pointing at the
  legacy `filter-rejected` command.
- **Legacy**: `psf-guard filter-rejected <db> <base>` still works
  (still useful for its `--stat-*` statistical-regrading flags) but
  prints a deprecation banner pointing at `move-rejects`.

Design, phases, tracker: [REJECT_ARCHIVE_PLAN.md](./REJECT_ARCHIVE_PLAN.md).

### Occlusion / cloud screening (2026-07)
- **Spatial metrics**: `src/spatial_analysis.rs` — grid-based (8x6 default)
  per-frame metrics: `star_dead_cell_fraction` (fraction of cells whose star
  density collapsed; from detector star positions, ~free), `star_uniformity`,
  `bg_cell_spread`/`bg_cell_max_dev` (per-cell background medians in physical
  ADU). Rationale: partial occlusion (trees, dome, stray light) leaves global
  star count/HFR within normal variation while killing part of the frame —
  validated on NGC 6820 2026-05/06 sessions where HFR stayed ~2.6 until the
  frame was >60% occluded. Clean-frame envelope across 4 nights/4 filters:
  dead ≤ 0.04, bg_spread ≤ ~0.09.
- **FitsImage ADU calibration**: `from_file` rescales each frame by its own
  min/max, so stored values are NOT comparable across frames. `raw_min`,
  `raw_scale`, `bzero` fields + `stored_to_adu()` recover physical ADU; any
  cross-frame background comparison must use them.
- **Sequence analyzer** (`src/sequence_analysis.rs`): `ImageMetrics` carries
  `dead_cell_fraction` + `bg_cell_spread`; quality score has an absolute
  spatial-coverage component (weight additive, missing-metric-safe for
  DB-only flows); EWMA baseline freezes when a frame's temporal score exceeds
  `baseline_freeze_threshold` and classification baselines skip anomalous
  frames (prevents slow occlusions absorbing into the baseline). The freeze
  is bounded: after `baseline_freeze_max_frames` (default 15) consecutive
  anomalous frames the run is accepted as a new steady state and baselines
  re-seed, so a permanent condition change (moonrise, light dome) cannot
  flag the rest of a session — occluded frames stay penalized via the
  absolute spatial term. Same bounded pattern in `grading.rs`
  `check_cloud_sequence` (re-seeds after `2*cloud_baseline_count`).
  `classify_issues` separates localized occlusion (dead-cell rise →
  `PossibleObstruction`, fires even when the composite score is still good,
  but requires an adjacent frame's dead fraction to corroborate so a
  single-frame blip never rejects) from uniform veiling (`LikelyClouds`)
  and stray-light gradients (bg-spread rise → `SkyBrightening`). Star-grid
  metrics abstain (None) on uniformly sparse frames (narrowband/short subs
  on slow rigs) instead of reporting phantom dead cells.
- **CLI**: `psf-guard screen-fits <dir>` — no DB needed. Detects stars +
  spatial metrics per frame (parallel), groups by (filter, exposure) from
  FITS headers, splits sessions, runs the sequence analyzer, prints per-frame
  verdict OK/WARN/REJECT (`--min-score`, `--dead-cell-rise` strictness,
  `--format table|csv|json`). Occlusion/cloud categories reject regardless of
  composite score; sky-gradient warns (recoverable via gradient removal).
- **DB regrade**: `screen-fits <dir> --regrade-db <slug-or-path> [--dry-run]`
  writes `[Auto] Obstruction/Clouds - score …` rejections for REJECT
  verdicts into the scheduler DB. Matching requires FITS basename AND
  |DATE-OBS − acquireddate| ≤ 10 min (both UTC epoch; observed skew ~1s on
  real N.I.N.A. data), so screening an unrelated directory can never
  regrade the wrong row; ambiguous or timestamp-less matches are skipped
  and counted. Already-Rejected rows untouched — but wrongly Accepted ones
  ARE regraded, since N.I.N.A.'s rolling star-count baseline absorbs slow
  occlusions and accepts frames that are >80% blocked (observed on the real
  DB where 31/33 occluded 06-30 R frames were Accepted). Opens the DB
  READ_WRITE without CREATE so a stale registry path errors instead of
  leaving a junk sqlite file. Rejections then flow through `move-rejects`
  as usual.
- **Photometric screening (2026-07)**: `src/photometry.rs` — cross-frame
  differential photometry + per-cell temporal baselines, for small clouds
  and errant light that grid metrics dilute away. Stars are matched across
  a session (grid-hash NN after estimating the global dither offset) against
  a presence-filtered reference catalog (median flux per star). Signals per
  frame: **transparency** (median matched-star flux ratio; thin uniform
  cloud dims ~10-40% long before stars vanish), **extinction_cell_fraction**
  (per-cell flux ratios ÷ global transparency < 0.75 → localized small
  cloud), **star_cell_drop_fraction** (cell's share of stars vs its own
  temporal median, Poisson floors), **bg_cell_rise_fraction** (plane-
  detrended per-cell background vs temporal median → transient errant
  light; static gradients live in the plane + baseline). Fluxes MUST be
  ADU (`stored_flux / raw_scale`) — stored units are per-frame rescaled.
  Sessions split on the 60-min gap and group by (filter, exposure) before
  matching. `screen-fits --annotate <dir>` renders a diagnostic PNG per
  WARN/REJECT frame (`src/commands/screen_annotate.rs`): grid overlay with
  RED = dead cells, ORANGE = localized extinction (labeled with the cell's
  flux ratio), MAGENTA = transient star-share drop, YELLOW = background
  rise, plus a caption strip (verdict/score/metrics/details, built-in
  bitmap font — no font-file dependency). Classification: localized extinction / star-cell drop →
  `LikelyClouds` (small cloud, REJECT; single-frame allowed — multi-star
  evidence), transparency < 0.8 → `LikelyClouds` (veil), bg-cell rise with
  stable stars → `SkyBrightening` (errant light, WARN). Quality score gains
  an absolute transparency term (weight 0.15 additive). Wired into BOTH
  `screen-fits` (Transp/Ext% columns) and the server scan
  (`StoredSpatialMetrics` persists a 300-star catalog + per-cell grids;
  `analyze_sequence`/`get_image_quality` run the pass at query time —
  pre-photometry cache entries lack catalogs and simply skip it until a
  re-scan). Photometry is blind to regions occluded most of a sequence
  (reference presence filter) — that remains the dead-cell metric's job.
- **Server/UI trigger**: `src/server/spatial_scan.rs` + `POST
  /api/db/{id}/analysis/spatial-scan` runs the same computation as a
  singleton per-DB background task (2 worker threads, ~8s/frame full-frame)
  over a target's FITS files (paths via `find_fits_file`). Results live in
  `DatabaseContext.spatial_metrics` and persist to
  `<cache_dir>/spatial_metrics.json` (survives restarts; entries invalidated
  by filename change; re-scan skips cached, `force` recomputes).
  `analyze_sequence` + `get_image_quality` merge the stored metrics into
  `ImageMetrics` so the SequenceView gains occlusion classification once a
  scan has run. Frontend: "Scan Occlusion" button in SequenceView
  (`useSpatialScan` hook, 1s progress poll, auto-invalidates
  sequence-analysis queries when the scan finishes).

### Two-DB sync (2026-06)
Lives in `src/commands/sync/` (`mod.rs` shared helpers + `grades.rs` + `pull.rs`).
Two complementary single-direction kinds, structured as `sync <kind>` so more
kinds slot in without breaking the CLI. Both match by `guid` (TS schema v22+),
accept a registry slug *or* a raw `.sqlite` path for `--from`/`--to` (registry
loaded only when a side isn't already a file), open source READ_ONLY / dest
READ_WRITE, refuse same-path source/dest, and have `--dry-run` + `--verbose`.

**`sync grades --from <our> --to <telescope>`** — push grading state.
- Pushes `gradingStatus` + `rejectreason` one-way; source wins. Match by
  `acquiredimage.guid`, guard via `require_target_scheduler_guid`.
- `--status pending|accepted|rejected` scopes source rows; `--project` /
  `--target` substring filters. One transaction (`batch_update_grading_status`),
  reuses `query_images` for both sides.
- Reports considered / matched / changed / unchanged / unmatched-source /
  dest-only + per-transition breakdown. NULL/empty and within-DB-duplicate guids
  skipped (counted, not fatal).

**`sync pull --from <telescope> --to <our>`** — pull structure + captures.
- Mirrors `exposuretemplate`, `project`, `ruleweight`, `target`, `exposureplan`,
  `acquiredimage`, and `imagedata` blobs (copied by default; `--no-image-data`
  to skip) into our DB.
  Processed in FK order, building `src_guid → dest_Id` maps so child FKs
  (target→project, plan→target+template, image→project+target+exposureId,
  ruleweight→project) are remapped onto the destination's local autoincrement
  Ids. Generic guid-keyed upsert reads all columns via `pragma_table_info` so it
  survives TS schema additions; `ruleweight` (no guid) matches by
  `(projectId, name)`; `imagedata` is insert-only.
- **Telescope wins for structure** (upsert: insert new, update changed fields —
  project state, plan desired/acquired counts, coordinates). **Local grading is
  preserved**: a new image takes the telescope grade; an existing image keeps
  its grade unless still Pending (0), in which case it adopts the telescope's
  grade. Guard via `require_pull_capable` (guid on all 5 core tables).
- `--project <substr>` scopes the pull (cascades to that project's targets,
  plans, images; templates always synced so plan FKs resolve). Whole pull runs
  in one transaction, rolled back on `--dry-run`.
- Reports per-table inserted/updated/unchanged plus grades filled/preserved.

### Smart Binary Mode Selection
- **Single binary** `psf-guard` with intelligent mode detection
- **GUI mode**: When tauri feature is enabled and no arguments passed → Desktop app launches
- **CLI mode**: When arguments are provided OR tauri feature is disabled → Command-line interface
- `src/main.rs`: Smart dispatcher that checks for arguments to determine mode
- `src/cli_main.rs`: Traditional command-line interface implementation
- `src/tauri_main.rs`: Tauri desktop application implementation

### Cache System (Current)
- **File Cache**: Database-based existence checking, auto-refreshed every 5 minutes
- **Directory Tree**: In-memory filename→path mapping, auto-refreshed every 5 minutes
- **Singleton Refresh**: Non-blocking with real-time progress tracking via SSE
- **Manual Refresh**: Button (file cache) + Shift+click (both caches)
- **Multi-Directory**: Scans multiple directories with first-hit preference

## Database Schema

```sql
project (1:many) → target (1:many) → acquiredimage

acquiredimage:
- gradingStatus: 0=Pending, 1=Accepted, 2=Rejected
- metadata: JSON with FileName
```

**Column naming**: Use exact case - `Id`, `projectId`, `acquireddate`, `filtername`

## Web Server

### API Endpoints
```
# Global
GET    /api/info
GET    /api/databases                    # list configured DBs
POST   /api/databases                    # register a new DB
PUT    /api/databases/{db_id}            # rename / re-point / change image dirs
DELETE /api/databases/{db_id}            # drop a DB

# Per-DB (nested)
GET    /api/db/{db_id}/projects
GET    /api/db/{db_id}/projects/overview
GET    /api/db/{db_id}/targets/overview
GET    /api/db/{db_id}/stats/overall
GET    /api/db/{db_id}/images?project_id=X&target_id=Y
PUT    /api/db/{db_id}/images/{id}/grade
GET    /api/db/{db_id}/images/{id}/preview?size=screen|large|original
PUT    /api/db/{db_id}/refresh-cache
PUT    /api/db/{db_id}/refresh-directory-cache
GET    /api/db/{db_id}/cache-progress    # polling (1s); aggregated indicator
                                          # on the frontend fan-outs across DBs
POST   /api/db/{db_id}/analysis/spatial-scan  # start background occlusion scan
                                               # {target_id, filter_name?, force?}
GET    /api/db/{db_id}/analysis/spatial-scan  # scan progress (1s poll) + cache size
```

### Frontend Architecture
- React 18 + TypeScript + Vite
- TanStack Query for server state
- Hash router with URL state management
- Embedded in binary for single-file deployment

### Navigation Fix (2025-09-01)
Fixed overview→grid navigation by building URLs directly:
- `navigate('/grid?project=5')` instead of state coordination
- Eliminates race conditions and timing issues
- Works for projects, targets, and "all projects"

## Key Features

### Web UI
- Smart image loading (preview → full resolution)
- Batch operations with multi-selection
- Undo/redo system (Ctrl+Z/Y)  
- Side-by-side comparison with zoom sync
- Real-time cache refresh with progress tracking

### Cache Progress UI (2025-09-01)
- Smart path truncation showing distinctive parts
- Pulsating progress indicator with integrated timer
- Fixed dimensions to prevent layout shifts
- Hover tooltips for full paths

## Development Workflow

```bash
# Essential commands
cargo fmt && cargo clippy && cargo test

# Run with logging
RUST_LOG=debug cargo run -- server db.sqlite images/

# Browser end-to-end (Playwright) — drives the embedded React UI against
# a real `psf-guard server` instance with --allow-database-management.
# Requires a built release binary; specs live under static/e2e/.
cd static && npm run test:e2e

# Tauri desktop development
cargo tauri dev

# Frontend development  
cd static && npm run dev

# OpenCV setup (macOS)
brew install opencv
# For Command Line Tools:
export DYLD_FALLBACK_LIBRARY_PATH="/Library/Developer/CommandLineTools/usr/lib"
# For Xcode.app:
# export DYLD_FALLBACK_LIBRARY_PATH="/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib"
```

### Tauri Desktop Configuration
- **Settings Panel**: Configure database and image directories via native file dialogs
- **System Directory Structure**: Uses platform-appropriate directories for all data:
  - **Configuration**: 
    - macOS: `~/Library/Application Support/psf-guard/config.json`
    - Windows: `%APPDATA%\psf-guard\config.json`
    - Linux: `~/.config/psf-guard/config.json`
  - **Cache**: 
    - macOS: `~/Library/Caches/psf-guard/`
    - Windows: `%LOCALAPPDATA%\psf-guard\cache\`
    - Linux: `~/.cache/psf-guard/`
  - **Temp Database** (when no N.I.N.A. database found):
    - macOS: `~/Library/Application Support/psf-guard/temp.db`
    - Windows: `%APPDATA%\psf-guard\temp.db`
    - Linux: `~/.local/share/psf-guard/temp.db`
- **Smart Settings Modal**: Only appears on first launch or when configuration is invalid/missing
- **Configuration Updates**: Settings saved immediately, with user-friendly restart prompt to apply changes  
- **Automatic Loading**: Configuration loaded and validated on application startup
- **Directory Management**: All directories are automatically created as needed
- **Database Validation**: Checks that configured database file actually exists before considering config valid

### Development Notes
- **Important**: Remove `static/dist/` contents if Tauri detection fails - cached production assets may be served instead of dev server
- File picker commands are async to prevent UI freezing
- Application restart applies configuration changes cleanly without data loss

### Recent Fixes
- **Navigation**: Direct URL building eliminates timing issues
- **Cache Progress**: Real-time directory scanning with smart path display
- **Multi-Directory**: Priority-based file lookup with comprehensive caching

## Key Implementation Details

### Star Detection
- N.I.N.A. algorithm port with MTF stretching
- Optional OpenCV integration (`--features opencv`)
- PSF fitting: Gaussian/Moffat models

### Performance
- O(1) file lookups via directory tree cache
- Non-blocking server startup with background refresh
- Comprehensive cache key strategy prevents collisions