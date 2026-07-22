# Create-and-Import Plan

Let users create a brand-new, fully-faithful Target Scheduler (TS) database and
populate it by importing folders of FITS images — **without** requiring an
existing N.I.N.A. scheduler DB. Available from the CLI and from the
DB-management UI. Import synthesizes projects / targets / exposure plans that the
user can then correct in the UI. Image-quality work is a separate database
maintenance action.

Status legend: ☐ todo · ◐ in progress · ☑ done

---

## 0. Schema foundation ☑

- ☑ Vendored the real TS schema at `src/ts_schema/` (`initial_schema.sql` +
  `migrate/1..23.sql`, byte-for-byte from
  `tcpalmer/nina.plugin.targetscheduler` @ `user_version = 23`). See
  `src/ts_schema/README.md` for provenance and the bootstrap algorithm.

Key facts driving the rest of the plan:

- A fresh DB = replay `initial_schema.sql`, then every `migrate/N.sql` in order,
  each in its own transaction (each ends with `PRAGMA user_version = N`).
  Landing state is `user_version = 23`.
- Migration 22 adds `guid TEXT` to `project`, `target`, `acquiredimage`,
  `exposureplan`, `exposuretemplate`, `profilepreference`. TS backfills these in
  **app code** (`Guid.NewGuid()`), not SQL — so **we assign a fresh UUIDv4 to
  every `guid` on insert**. This is what makes the DB "v22+" and unlocks
  `sync`, `move-rejects`, etc.
- `acquiredimage.metadata` is JSON shaped like the `ImageMetadata` DTO
  (`FileName`, `FilterName`, `ExposureStartTime`, `ExposureDuration`, `Gain`,
  `Offset`, `Binning`, `ReadoutMode`, `ROI`, ADU stats, star metrics, guiding
  RMS, focuser/rotator/pier/camera/airmass). PSF Guard reads `FileName` from it;
  everything else maps from FITS headers (+ star detection at backfill time).

---

## 1. Bootstrap module — `src/ts_schema.rs` ☑

- ☑ `create_fresh_db(path) -> Result<Connection>`: refuses existing non-empty
  files; replays initial + `1..=23`, each migration in its own transaction;
  verifies `user_version` advances to exactly N after each script.
- ☑ `TS_SCHEMA_VERSION = 23`, `new_guid()` (UUIDv4 via the `uuid` crate,
  lowercase-hyphenated like `Guid.NewGuid().ToString()`).
- ☑ `apply_schema` refuses to migrate a **populated pre-v22** DB (upstream
  pairs v17/v22 with app-code data repairs we don't implement); v22→v23 is
  pure SQL and upgrades in place.
- ☑ Golden column tests: all 11 tables, migration-17 rename
  (`accepted`→`gradingStatus`) verified, migration-1 drops verified, guid
  guards (`require_pull_capable` shape) pass on a fresh DB.

**Found during review**: `acquisition_context.rs` treated `epoch_code == 0`
as J2000. N.I.N.A.'s enum is JNOW=0, B1950=1, **J2000=2** — TS writes 2 for
every target it creates, so absolute offset grading silently abstained on
real DBs (and would have graded JNOW coordinates ~9′ off). Fixed to `== 2`
with tests for both directions.

---

## 2. Grouping model — `src/commands/import/grouping.rs` ☑

Per light FITS, extract (reuse existing header parsing): `OBJECT`, `RA/DEC`
(`OBJCTRA/OBJCTDEC`), `FILTER`, `GAIN`, `OFFSET`, `XBINNING`, `READOUTMODE`,
`FOCALLEN`, `INSTRUME`, `TELESCOP`, `DATE-OBS`, `EXPTIME`.

- **EquipmentSignature** = (`TELESCOP`/`INSTRUME` camera, `FOCALLEN`, binning).
  Frames with different signatures never share a project.
- **Project** = one distinct `OBJECT` by default, across all of its capture
  dates for one equipment signature. The project carries one target with the
  same name and median RA/Dec.
- **Mosaic exception** = panel-style target names with the same root (for
  example `North America Panel 1/2`), centers within 5°, and capture ranges
  no more than `--time-gap-days` apart share one project. Name, sky position,
  and time must all agree; nearby unrelated objects never merge by position
  alone.
- **ExposureTemplate** = distinct (`FILTER`, `GAIN`, `OFFSET`, binning,
  `READOUTMODE`); **ExposurePlan** = one per template on a target, `exposure` =
  `EXPTIME`, `desired`/`acquired`/`accepted` seeded from the frame counts.
- Emits a `Plan` describing every project/target/template/plan/frame so both the
  CLI and the API import paths (and a dry-run preview) share one deterministic
  result.

---

## 3. Import engine — `src/commands/import/mod.rs` ☑

Implemented as planned, plus: light-frame filter (IMAGETYP; darks/flats/bias
skipped), unreadable-file counting, `--profile-id` required when the DB has
several profiles, exposure-plan `desired`/`acquired` seeded from frame counts
(accepted stays 0 until grading), per-template `defaultexposure` from the most
common exposure. Metadata JSON deliberately omits star/ADU/guiding keys —
readers treat missing as None, zeros would read as measurements.

Shared core used by CLI and server.

- Scan dirs (reuse `find_fits_file` / directory walk); read headers in parallel
  via `concurrency::plan_workers` + `parallel_index`.
- Build the `Plan` (§2).
- Apply in **one transaction**: a synthetic `profilepreference` row (one
  `profileId` per DB, stored on the registry entry), then projects → targets →
  exposuretemplates → exposureplans → acquiredimages. `acquiredimage.metadata`
  from headers, `gradingStatus = 0`, `guid = new_guid()`. Default rule-weight
  rows added per project (mirror TS's post-migration "repair").
- **Idempotent**: skip a frame whose `FileName` already exists as an
  `acquiredimage` (safe re-import / incremental import of new subs).
- `imagedata` thumbnails: off by default (follow-up).
- Returns `ImportSummary` (counts + created project/target ids) for the UI and
  for kicking the backfill.

---

## 4. CLI ☑ (smoke-tested on real N.I.N.A. frames)

- `psf-guard create-db <new.sqlite> <dirs...> [--registry P] [--profile-id S]
  [--time-gap-days N] [--no-register] [--dry-run]` — bootstrap (§1) + import
  (§3) + register into the shared registry (like `server` does).
- `psf-guard import <db-slug-or-path> <dirs...> [--time-gap-days N] [--dry-run]`
  — import into an existing DB. `--dry-run` prints the grouping preview and
  writes nothing.

---

## 5. API — DB management endpoints ☑

All guarded by `require_database_management_allowed`.

- ☑ `POST /api/databases/create` `{ name, image_dirs[], db_path?, slug?,
  time_gap_days?, profile_id?, backfill? }` → bootstraps the fresh DB
  (default location `<registry dir>/databases/<name-slug>.sqlite`, uniquified),
  registers it, inserts the live `DatabaseContext`, starts the import job.
- ☑ `POST /api/db/{id}/import` (`image_dirs` defaults to the DB's configured
  dirs; `dry_run`, `backfill` flags) + ☑ `GET /api/db/{id}/import` progress.
- ☑ `src/server/import_job.rs`: singleton per-DB job
  (`DatabaseContext.import_job`), stages `scanning → importing →
  complete|error`. Import runs on a **dedicated connection** (never the shared
  request connection); scan-headers stage reports per-file progress. Panic in
  the blocking task is caught via the JoinError path so the singleton can't
  wedge.
- ☑ `POST/GET /api/db/{id}/analysis/quality-backfill` runs quality work as a
  separate database-wide job. `force=false` fills missing cache entries;
  `force=true` recomputes star counts, spatial/photometric metrics, and
  pointing evidence. Fresh star counts and HFR replace stale scheduler values
  in sequence grading. The job uses the background worker budget and yields
  between frames to interactive work.
- ☑ `tests/integration_import.rs`: 6 end-to-end tests with synthetic
  N.I.N.A.-style FITS (create→import→verify v23 rows, idempotent re-import,
  dry-run writes nothing, 403 without the management flag, missing-dir 400,
  concurrent import produces no duplicates).
- ☑ Import completion is independent from optional quality analysis; users can
  browse and correct the new structure as soon as the header transaction ends.

---

## 6. UI — DB-management panel (`static/src/components/TauriSettings.tsx`) ☑

- ☑ **"✨ New Database from Images"** form mode: name + image folders (native
  picker in Tauri, text-add in browser) → `createDatabaseFromImages` →
  tracked progress panel.
- ☑ **Per-DB "Import" button** → `startImport` with the DB's configured dirs
  (button shows "Importing…" while its job runs).
- ☑ `hooks/useImportJob.ts`: 1s poll while running; on running→finished
  invalidates `['databases']` + `['db', id]`; `describeImportProgress`
  renders one-line scan/import text and the dry-run/skipped outcome summary.
- ☑ Progress panel lists created projects (name — targets, frames) on
  completion.
- ☑ Each database row exposes **Analyze Missing Quality** and **Rescan All
  Quality**. The separate job restores its progress after the settings page is
  reopened.
- Follow-up: a Playwright e2e spec for the create flow (needs the release
  binary harness); time-gap-days option exposed in the form.

---

## 7. UI — correct the groupings ☑ (MVP)

Import guesses; the user fixes them.

- ☑ Endpoints (management-gated): `PUT /api/db/{id}/projects/{pid}` (rename),
  `PUT /api/db/{id}/targets/{tid}` (rename and/or `project_id` move — images
  follow the target in one transaction), `POST
  /api/db/{id}/projects/{pid}/merge` (targets+images move, source project +
  rule weights deleted). Cross-profile moves/merges are refused (exposure
  plans/templates are profile-scoped; upstream's copy-and-delete dance is out
  of scope).
- ☑ `Database::{rename_project, rename_target, move_target, merge_projects}`
  in `src/db.rs`; covered by `organize_rename_move_and_merge` in
  `tests/integration_import.rs`.
- ☑ Overview UI: ✏️ on each project/target row (only when management is
  allowed) opens an inline editor — rename input + "Merge into…" select on
  projects, rename + "Move to…" select on targets; merge confirms first;
  invalidates the DB's queries on save.
- Follow-up: reassign individual frames between targets; bulk select.

---

## 8. Safety: merge-aware import + preview-confirm (2026-07-22) ☑

Shipped after a real-world incident: the per-DB Import button ran a
ground-zero import against an existing scheduler database, synthesizing
duplicate projects/targets with no confirmation.

- ☑ **Merge phase**: after basename dedup, frames attach to EXISTING targets
  — exact (case-insensitive) OBJECT-name match first, else nearest target
  within `match_radius_deg` (default 0.5°). Attached frames reuse the
  target's project/profile, reuse the profile's exposure template, and reuse
  a matching exposure plan (bumping `acquired`) or add one. Only unmatched
  frames reach the ground-zero grouping. `--no-attach` restores the old
  behavior deliberately.
- ☑ A fully-attached import no longer resolves/creates an import profile —
  multi-profile databases import cleanly when everything matches.
- ☑ **UI preview-confirm**: the Import button now runs a dry-run first and
  shows exactly what would happen (attached per existing target with match
  kind, NEW projects, skip counts); nothing is written until the user
  clicks "Import N frame(s)". Cancel writes nothing.
- ☑ **Recovery**: `psf-guard remove-imported <db> [--dry-run]` deletes the
  projects an import created (recognized by the `Imported by PSF Guard`
  description marker) with their targets/plans/rule weights/images —
  attached frames in pre-existing projects are never touched.
- ☑ An opt-in import can queue changed targets in the general quality job;
  cached frames skip unless a full rescan was requested.

## Phasing

1. **P0** §1 bootstrap module + golden-schema test. ☑
2. **P1** §2–§4 grouping + import engine + CLI (header-only, all Pending). ☑
3. **P2** §5 create/import endpoints + independent quality-job wiring. ☑
4. **P3** §6 UI wizard + per-DB import button + progress. ☑
5. **P4** §7 UI grouping edits + supporting write endpoints. ☑ (MVP: rename /
   move-target / merge; frame-level reassign is a follow-up)

## Open defaults (proposed — flag any you want changed)

- Time-gap threshold: **14 days**.
- Synthetic `profileId`: fixed UUIDv4 stored on the registry entry (stable so
  re-imports and `sync` line up).
- New DB file location: `<config>/psf-guard/databases/<slug>.sqlite`.
- Equipment signature fields: `TELESCOP` + `INSTRUME` + `FOCALLEN` + binning.
