# Vendored Target Scheduler database schema

These SQL files are copied **verbatim** from the upstream N.I.N.A. Target
Scheduler plugin so that PSF Guard can create a brand-new, fully-faithful
scheduler database from scratch (rather than requiring an existing one).

## Provenance

- Upstream repo: <https://github.com/tcpalmer/nina.plugin.targetscheduler>
  (formerly `tcpalmer/nina.plugin.assistant`, which is now stale and stops at
  schema v16 — do **not** use it).
- Path in upstream: `NINA.Plugin.TargetScheduler/Database/`
  - `Initial/initial_schema.sql`  → `initial_schema.sql`
  - `Migrate/{1..23}.sql`          → `migrate/{1..23}.sql`
- Captured at upstream schema **user_version = 23** (2026-07).

## How Target Scheduler bootstraps a DB (replicate exactly)

From `Database/SchedulerDatabaseContext.cs`
(`CreateOrMigrateDatabaseInitializer.InitializeDatabase`):

1. If the DB has no tables, run `initial_schema.sql` in one transaction.
2. Read `PRAGMA user_version`.
3. For each `migrate/N.sql` with `N > user_version`, in ascending order, run it
   in its own transaction. Each script ends with `PRAGMA user_version = N;`.
4. Post-migration "repair & update" fixes that are **not** in the SQL:
   - Ensure every project has a `ruleweight` row for each scoring rule.
   - v5→v6: convert NINA 2 rotation to NINA 3 position angle.
   - v<17→17: remap `gradingStatus` enum, migrate override-exposure-order text
     into the `overrideexposureorderitem` table.
   - **v<22→22: generate a UUID (`Guid.NewGuid().ToString()`) for every row's
     new `guid` column** in `acquiredimage`, `exposureplan`, `exposuretemplate`,
     `profilepreference`, `project`, `target`.

Because PSF Guard creates a *fresh* DB, the simplest faithful path is: replay
`initial_schema.sql` + `migrate/1..23.sql` in order (landing at
`user_version = 23`), then populate rows ourselves — assigning a fresh UUIDv4 to
every `guid` column as we insert, exactly as step 4's v22 repair would.

## Notable schema evolution (why replaying migrations matters)

Migrations mutate the initial schema in ways a single consolidated CREATE cannot
capture without care:

- **1**: drops `project.startdate`, `project.enddate`.
- **17** (TS4→5): renames `acquiredimage.accepted` → `gradingStatus`, adds
  `acquiredimage.exposureId`; renames `target.overrideExposureOrder` →
  `unusedOEO`; creates `overrideexposureorderitem` and `filtercadenceitem`.
- **22**: adds the `guid` columns PSF Guard's newer features depend on
  (this is the "schema v22+" requirement referenced in CLAUDE.md).
- **23**: adds `profilepreference` API columns.

## `acquiredimage.metadata` JSON

PSF Guard reads `FileName` (and can populate quality metrics) from the
`metadata` TEXT column. Its shape is the `ImageMetadata` DTO in
`NINA.Plugin.TargetScheduler.Shared/Utility/ImageMetadata.cs` — a flat JSON
object with fields such as `FileName`, `FilterName`, `ExposureStartTime`,
`ExposureDuration`, `Gain`, `Offset`, `Binning`, `ReadoutMode`, `ROI`,
`DetectedStars`, `HFR`, `HFRStDev`, `FWHM`, `Eccentricity`, ADU stats, guiding
RMS, focuser/rotator/pier/camera/airmass. Most map 1:1 to FITS headers plus
PSF Guard's own star detection.

## License

The `.sql` files in this directory (and `migrate/`) are copied verbatim from
`tcpalmer/nina.plugin.targetscheduler` and remain under the **Mozilla Public
License 2.0** — see [`LICENSE`](./LICENSE) in this directory. MPL-2.0 is a
file-level copyleft: these files stay MPL-covered wherever they go (including
embedded via `include_str!` into release binaries), and their source is this
directory. Per MPL-2.0 §3.3 this is a permitted "Larger Work" combination —
the rest of PSF Guard is Apache-2.0 (see the repository root `LICENSE`) and
is unaffected.

Upstream source: <https://github.com/tcpalmer/nina.plugin.targetscheduler>

## Updating this snapshot

Re-fetch from the upstream repo when TS ships a new `user_version`, add the new
`migrate/N.sql`, and extend the replay list. Keep these files byte-for-byte
identical to upstream.
