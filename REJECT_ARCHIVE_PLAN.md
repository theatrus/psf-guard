# Out-of-Tree Reject Archive — Design & Implementation Plan

Status: **Draft / not started**
Owner: TBD
Last updated: 2026-05-20

## 1. Goal

Move user-rejected FITS files (DB `gradingStatus = 2`) out of the directory
tree that PixInsight loads in bulk, while keeping them findable per-project.
Today's `psf-guard filter-rejected` only renames `LIGHT/` → `LIGHT_REJECT/` as
a sibling directory under the same project root — PixInsight users who point
their session loader at the project still see the rejects unless they
manually exclude that folder. We want a true out-of-tree archive that's
discoverable per-project, configurable per-rig, and reversible.

### Concrete outcomes

- Rejected files (plus same-basename sidecars) land at:
  `<image_dir>/<P>/REJECT/<original-subpath>/<file>` by default, where
  `<P>` is the depth-1 segment under the configured image dir.
- The CLI is the only trigger surface (no UI button in v1). It's multi-DB
  aware via `--db <slug>`.
- The action is **idempotent**: re-running skips files already archived.
- Each move is recorded in a `psf_guard_archive` table in the same SQLite
  database, joined on `acquiredimage.guid`. A redundant JSON manifest lives
  at the REJECT root so disaster recovery (lost DB) is still possible.
- A future `psf-guard restore-rejects` command (out of scope for v1 but
  designed for) reverses the move using the recorded state.

### Non-goals (v1)
- No automatic move when the user clicks "reject" in the web UI — that
  stays a DB-only operation. Archiving is an explicit batch action.
- No HTTP API. The CLI is the only entry point.
- No deletion. We move files; we never `rm` them.
- No multi-base-dir conflict resolution beyond "each image_dir evaluated
  independently against the file's actual location."

---

## 2. Current state (survey, single-DB)

- **CLI**: `psf-guard filter-rejected <db> <base_dir> [--dry-run] [--project] [--target]` (`src/cli.rs:47-72`, `src/cli_main.rs:36-58`). Pre-multi-DB; takes a `&Connection` directly.
- **Core**: `src/commands/filter_rejected.rs` — queries DB for `gradingStatus = 2`, generates ~28 candidate paths per image via `get_possible_paths` (date ±1 day, several reject-folder naming conventions), falls back to `DirectoryTree::find_file_first`.
- **Move action**: `process_file_movement:258-266` — string-substitutes `/LIGHT/` → `/LIGHT_REJECT/` in the source path, `fs::rename` to that. Sibling folder, same tree.
- **No state tracking**: `gradingStatus = 2` means "user rejected"; no flag for "file has been physically moved." Re-runs hit not-found errors silently.
- **No sidecar handling**: only the primary FITS file moves. `.xisf`, plate-solve `.json`, mask files stay.
- **No undo**.
- **Zero integration tests**.
- **UI never invokes it**: web UI's reject button only sets `gradingStatus = 2`; users drop to a terminal afterward.

---

## 3. Target Scheduler schema research

Done; full report in conversation thread, summary here.

- `acquiredimage.metadata` is **TEXT NOT NULL** owned by the Target Scheduler
  plugin. The C# DTO (`ImageMetadata`) deserializes with Newtonsoft defaults
  (`MissingMemberHandling.Ignore`, no `[JsonExtensionData]`). Round-tripping
  through the `Metadata` property pair is **lossy by design**: unknown keys
  are silently dropped on read.
- **Today** no upstream code path round-trips: writes on existing rows
  (`Grading/ImageGrader.cs` `UpdateDatabase`) mutate `GradingStatus` and
  `RejectReason` only, and EF6 sends the unchanged `metadata` string back
  verbatim. So extra keys would survive — for now.
- **Future risk**: any new upstream PR that does
  `acquiredImage.Metadata = newMetadata` anywhere strips our annotations
  silently. Detecting that requires monitoring the upstream repo.
- **No annotation columns** (no `notes`, `comments`, `tags`, etc.) on
  `acquiredimage`. The sibling `imagedata` table is for thumbnail BLOBs.
- **`guid` column** (added in migration 22) is explicitly designed as a
  cross-tool identifier; it's stable across export/reimport whereas `Id`
  is auto-increment.

**Decision**: do NOT stamp `metadata`. Use a new sibling table
(`psf_guard_archive`) in the same SQLite file, keyed on `acquiredimage.guid`.
Plus a JSON manifest at the REJECT root for redundancy.

---

## 4. Design

### 4.1 Destination rule

Compute the archive path from `(image_dir, source_path)`:

```
relative   = source_path.strip_prefix(image_dir)
segments   = relative.path_components()
if segments.len() >= depth + 1:
    head, tail = segments[..depth], segments[depth..]
    archive  = image_dir / head.join(/) / segment_name / tail.join(/)
else:
    # File lives shallower than the configured depth. Fall back to
    # placing it directly under <image_dir>/<segment_name>/<file>.
    archive  = image_dir / segment_name / relative
```

Defaults:
- `segment_name = "REJECT"`
- `depth = 1` (insert below the project folder)

Example with depth=1, segment="REJECT":

| Source | Archive |
|---|---|
| `Targets/M31/2026-04-16/B/LIGHT/img.fits` | `Targets/M31/REJECT/2026-04-16/B/LIGHT/img.fits` |
| `Targets/M42/LIGHT/img.fits` | `Targets/M42/REJECT/LIGHT/img.fits` |
| `Targets/img.fits` (depth-0) | `Targets/REJECT/img.fits` |

### 4.2 Config precedence

CLI flag > per-DB registry override > global default.

- Global default: `{ segment_name: "REJECT", depth: 1, sidecar_exts: [".xisf", ".json", ".txt"] }`. Hardcoded in source.
- Per-DB registry override: optional `reject_archive` block in each `DbEntry`. New fields on the JSON shape (additive — old configs keep working):
  ```jsonc
  {
    "id": "imaging-rig",
    "name": "Imaging Rig",
    "db_path": "...",
    "image_dirs": [...],
    "reject_archive": {            // optional
      "segment_name": "REJECT",    // optional
      "depth": 1,                  // optional
      "sidecar_exts": [".xisf", ".json"]  // optional
    }
  }
  ```
- CLI: `--reject-segment NAME`, `--reject-depth N`, `--sidecar-exts ".xisf,.json"` override the per-DB / global values for this invocation only.

### 4.3 Sidecar handling

For each rejected file, after locating the primary on disk, collect siblings
in the same directory whose **stem matches** and whose **extension is in
the configured `sidecar_exts` list** (case-insensitive compare on the ext).
Move every match alongside the primary into the same archive directory.

Calibration masters (`Bias_*.fits`, `Dark_*.fits`, etc.) have a different
stem and are therefore never selected.

### 4.4 State tracking — sibling table on the TS database

The metadata-column stamping path is rejected (see §3). Use a sibling table:

```sql
CREATE TABLE IF NOT EXISTS psf_guard_archive (
  acquired_image_guid TEXT PRIMARY KEY,  -- joins to acquiredimage.guid
  acquired_image_id   INTEGER NOT NULL,  -- joins to acquiredimage.Id (current run convenience)
  moved_at            INTEGER NOT NULL,  -- unix seconds
  original_path       TEXT NOT NULL,
  archive_path        TEXT NOT NULL,
  segment_name        TEXT NOT NULL,
  archive_depth       INTEGER NOT NULL,
  sidecar_files       TEXT NOT NULL DEFAULT '[]',  -- JSON array of relative sidecar names
  source_db_slug      TEXT                          -- our registry slug, for cross-DB tooling later
);
CREATE INDEX IF NOT EXISTS idx_psf_guard_archive_image_id ON psf_guard_archive(acquired_image_id);
```

The table is **owned by psf-guard**. It doesn't shadow the upstream
`gradingStatus` field — `gradingStatus = 2` means "user marked rejected,"
and presence in `psf_guard_archive` means "file has been moved."

Idempotency: before moving a file, check if a row exists for that
`acquired_image_guid` (or `acquired_image_id` if guid is missing). If yes
and the archive path exists, skip. If yes and the archive path is missing,
log and skip (manual intervention; we don't auto-recover).

**Requires** Target Scheduler schema `user_version >= 22` for the `guid`
column. Older DBs are detected at startup of the command — emit a clear
error pointing the user at how to upgrade their plugin.

### 4.5 Manifest at the REJECT root

A redundant JSON file lives at `<image_dir>/<P>/REJECT/.psf-guard-manifest.json`
(one per archive root). Schema:

```jsonc
{
  "version": 1,
  "moves": [
    {
      "guid": "...",
      "image_id": 42,
      "moved_at": 1776470400,
      "original_path": "...",
      "archive_path": "...",
      "sidecar_files": ["img.xisf", "img.json"],
      "segment_name": "REJECT",
      "archive_depth": 1
    }
  ]
}
```

Appended to atomically (write to `.tmp`, rename). Used only for disaster
recovery if the DB is lost; the table is the authoritative source.

### 4.6 CLI surface

A new subcommand replaces the existing one (which becomes a deprecated
alias):

```
psf-guard move-rejects --db <slug> [--dry-run]
                       [--reject-segment NAME] [--reject-depth N]
                       [--sidecar-exts ".xisf,.json,.txt"]
                       [--registry <path>]
                       [--project NAME] [--target NAME]
```

- `--db <slug>` — required. Looks up the entry in the shared registry.
- Project/target filters narrow scope (same as today).
- `--dry-run` prints the plan (source → destination, sidecars) without
  touching disk or the DB.
- `--registry <path>` overrides the default registry location (matches the
  server flag from B3 of the multi-DB work, for dev/test isolation).

The legacy `filter-rejected <db> <base_dir>` stays as a thin alias that
prints a deprecation warning, looks up the DB in the registry by canonical
path of the supplied `<db>`, and calls into the new code with default
arguments. Removed in a future cycle once the docs and any scripts catch up.

### 4.7 Restore command (future, designed-for-but-not-built)

Out of scope for v1, but the shape is dictated by the state-tracking
design:

```
psf-guard restore-rejects --db <slug> [--image-id N] [--guid X] [--dry-run]
```

Reads `psf_guard_archive`, moves each archived file (plus sidecars) back to
`original_path`, and deletes the row. Errors on rows whose archive path no
longer exists.

---

## 5. Implementation phases

Each phase ends in a green build (`cargo build && cargo test`) and a
mergeable commit.

### Phase A1 — Schema bootstrap + read helpers
- `src/db.rs`: add `ensure_psf_guard_archive_table(&Connection)` that issues
  the `CREATE TABLE IF NOT EXISTS` + index. Called at start of any command
  that uses it.
- Read helper: `get_archive_record(guid_or_id) -> Option<ArchiveRecord>`.
- Schema-version check: `assert_target_scheduler_schema(>= 22)` reads
  `PRAGMA user_version`. Older DB → clear error with upgrade hint.
- Unit tests against an in-memory SQLite.

### Phase A2 — Per-DB registry field
- Extend `DbEntry` in `src/db_registry.rs` with optional `reject_archive`
  block. Default-construct when absent. Backwards-compatible: existing
  v2 configs load unchanged.
- Round-trip test (load → save → load).
- Update the Tauri settings types (`static/src/utils/tauri.ts`) so future
  settings UI can edit it. UI editor is *not* added in v1.

### Phase A3 — Destination computation
- Pure function `archive_path_for(image_dir, source_path, depth, segment)`
  in a new `src/commands/reject_archive.rs`. No I/O.
- Property tests over realistic NINA path shapes; explicit unit tests
  for the depth-0 edge case and multi-`image_dir` independence.

### Phase A4 — Sidecar discovery
- `find_sidecars(primary_path, exts) -> Vec<PathBuf>`. Tested against a
  tempdir layout including non-matching siblings and calibration masters.

### Phase A5 — `move-rejects` command
- New `Commands::MoveRejects` clap variant + handler in `src/cli_main.rs`.
- Reuses existing `query_images` + `get_possible_paths` + `DirectoryTree`
  fallback for source-on-disk discovery.
- For each rejected row:
  1. Check `psf_guard_archive` — skip if already moved.
  2. Compute destination + sidecars.
  3. Print plan (dry-run exits here).
  4. `fs::create_dir_all` destination parent.
  5. `fs::rename` primary, then each sidecar.
  6. INSERT row into `psf_guard_archive`.
  7. Append entry to the REJECT-root manifest.
- Transactional behavior: if step 4 or 5 fails, rollback any moves already
  made for this image (move them back) before failing the run. The DB
  insert (step 6) is the commit point.

### Phase A6 — Deprecation shim for `filter-rejected`
- Keep the existing command but emit a warning and call into the new path
  with default args. Update README + CLAUDE.md.

### Phase A7 — Integration test
- Synthesize a tiny SQLite (the test-fixture pattern from
  `tests/integration_sequence_analysis.rs::create_test_schema`) plus a
  tempdir with realistic NINA-style folder layout. Bump `user_version` to
  22 and populate a `guid` column.
- Spec: register 3 rejected images, run `move-rejects --dry-run`, assert
  the plan; run for real, assert files moved + sidecars moved + rows
  inserted + manifest written; re-run, assert no-op.

### Phase A8 (future) — `restore-rejects`
- Designed for in §4.7; not built in v1.

---

## 6. Open questions / deferred work

- **Multiple image_dirs configured for the same DB**: today each is
  scanned for the source file; archive lands relative to whichever dir
  the file was found under. Need to confirm this matches user
  expectation — if a file in dir B should always archive into dir A
  (preferred archive root), we need an explicit `archive_root` per DB
  rather than computing from `image_dir`.
- **Cross-DB archive search** (looking up "where was this file archived?"
  given just a filename): possible later via the `psf_guard_archive`
  table since `original_path` is recorded.
- **Restore command**: phase A8, after v1 ships and we have real moves
  in the wild.
- **Guid backfill**: if a user has a TS DB at `user_version >= 22` but
  the `guid` column was populated only for newly-acquired images
  (existing rows have NULL), we may need a backfill path. Need to
  check whether TS backfills on migration or only on INSERT going
  forward.
- **UI trigger**: not in v1, but eventually a "Move rejected files
  out-of-tree" button on each DB section in the merged Overview would
  close the workflow loop. Gated behind `--allow-database-management`
  (same gate as the existing CRUD endpoints).

---

## 7. Task tracker

`[ ]` pending, `[~]` in progress, `[x]` done.

### Research
- [x] Verify safety of stamping `acquiredimage.metadata` (verdict: unsafe long-term; pivot to sibling table on `acquiredimage.guid`)

### Implementation
- [x] **A1** Schema bootstrap (`psf_guard_archive` table) + version check
- [x] **A2** Per-DB `reject_archive` registry field (additive)
- [x] **A3** `archive_path_for` pure function + tests
- [x] **A4** `find_sidecars` discovery + tests
- [x] **A5** `psf-guard move-rejects` CLI command (multi-DB-aware) with transactional move + DB row + manifest entry
- [ ] **A6** Deprecation shim for `filter-rejected`
- [ ] **A7** Integration test against a synthesized NINA-style tempdir

### Docs / follow-up
- [ ] Update `README.md` (CLI section)
- [ ] Update `CLAUDE.md` (Architecture / Multi-database support note)
- [ ] Watch upstream `tcpalmer/nina.plugin.targetscheduler` for any new
      writes to `acquiredImage.Metadata` (would invalidate the residual
      "metadata stamping is safe today" claim — we're not relying on it,
      but a regression there could affect anyone else who is)

### Deferred
- [ ] **A8** `psf-guard restore-rejects` command
- [ ] UI trigger (button on Overview DB section)
- [ ] Cross-DB archive lookup helper
