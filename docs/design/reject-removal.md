# Reject removal

Status: **In progress.** Rejection dates, removal, restore and emptying the
trash work from the CLI (`src/commands/reject_removal.rs`); the server API,
plan progress and the UI follow.

Rejected subframes that are truly bad cost disk space and clutter every view
that lists frames. Reject removal is an optional cleanup a person runs on
demand: it takes rejected frames out of a database's catalog, puts their
files in a trash folder, and keeps a tombstone that can bring both back. A
later purge empties tombstones for good.

It builds on the [reject archive](reject-archive.md), which moves rejected
files out of the light folders and never deletes anything.

## Decisions

These were settled with the owner on 2026-10-09.

- **On demand only.** Removal runs when someone asks for it, from Settings
  or the CLI, always as a preview and then an Apply. Nothing removes frames
  in the background.
- **A grace period.** Only frames rejected at least N days ago qualify, so a
  frame rejected by mistake can still be regraded. N is chosen for each run
  (7 by default; 0 takes every reject).
- **Trash, then delete.** Files move to a trash folder on the same disk.
  They are deleted only when someone empties the trash, and only for
  removals older than their retention period (14 days by default).
- **Tombstones, so frames can be restored.** A removed frame's Target
  Scheduler row, thumbnails and PSF Guard records are kept in a tombstone.
  Restore puts the row and the files back. Purging tombstones is a separate,
  later step.
- **This catalog only.** Removal changes this PSF Guard's database. A rig's
  own Target Scheduler copy keeps its rows; the tombstone stops Sync from
  bringing them back here.

## Rejection dates

`psf_guard_rejected_at(acquired_image_guid PRIMARY KEY, rejected_at)` holds
when each rejected frame was first seen rejected, in Unix seconds.
`db::record_rejection_times` runs beside `reconcile_accepted_counts` after
every grade change PSF Guard makes, and the removal preview runs it too.
Grade sync and catalog pulls update the dates only in a database that
already keeps them, so a rig's Target Scheduler copy never gains the table.
Each run:

- a rejected frame without a date gets the current time;
- a frame that is no longer rejected loses its date, so rejecting it again
  starts the wait over;
- frames without a GUID are left out, since tombstones key on it.

A reject made outside PSF Guard, such as by Target Scheduler's own grader
on a shared database, is dated when PSF Guard first notices it. Frames
rejected before this table existed are dated on first sight; choosing 0
days removes them at once.

## Removal

Removal is a preview and an Apply, like organization changes. Apply carries
the preview's digest and refuses with 409 when the candidates changed. It
needs the server's database-management permission.

A frame qualifies when it is rejected, has a GUID, was dated at least N
days ago, and lies in the chosen scope (database, project or target). The
preview names each frame left out and why:

- it appears in a collaboration report or capture (a later correction must
  include it);
- its file also backs another row;
- a file could not be moved to the trash.

For each frame Apply:

1. finds its files: the light (or its archived copy, with the archive's
   sidecars), and its calibrated and registered copies;
2. moves them to `<image dir>/.psf-guard-trash/<batch>/<relative path>`,
   never overwriting; a file outside every image directory goes to a
   `.psf-guard-trash` folder beside it;
3. in one transaction, writes the tombstone and deletes the
   `acquiredimage` row, its `imagedata` rows, and its
   `psf_guard_frame_derivative`, `psf_guard_remote_image_file`,
   `psf_guard_archive` and `psf_guard_rejected_at` rows; if the transaction
   fails, the files move back;
4. deletes the frame's per-image caches (previews, annotated previews,
   stars, PSF views, statistics, astrometry, satellites, spatial metrics),
   since Target Scheduler can reuse a deleted row's Id.

Afterwards it reconciles accepted counts, refreshes the directory tree and
file caches, and tells automatic stacks the database changed.

`exposureplan.acquired` is left alone: Target Scheduler owns it, and a pull
copies the rig's value back. Plan progress still counts removed rejects as
rejected, from the tombstones.

## Tombstones

`psf_guard_removed_image` holds one row per removed frame: its GUID, its
original row Id, target and exposure plan, when and in which batch it was
removed, its rejection date, the `acquiredimage` row and its side rows as
JSON (thumbnails base64-encoded), the files with their trash paths, when
the batch's trash may be emptied, and when its files were deleted.

The GUID suppresses the frame everywhere a row could come back:

- Sync pulls (CLI, local database sync, remote peers and the N.I.N.A. Sync
  plugin's Pull) skip a tombstoned GUID and count it as removed here;
- import, auto-import and remote uploads skip a file whose frame is
  tombstoned;
- folder scans and the directory tree skip `.psf-guard-trash`.

## Restore and the trash

Restore takes a batch or single frames. While the files are still in the
trash it moves them back (a taken name gets a `.restored` suffix, as
archive restore does), inserts the row (under its old Id when that is free)
and its side rows, restores its rejection date, and drops the tombstone.
Once the trash has been emptied, a frame cannot be restored.

Emptying the trash deletes the files of every batch past its retention and
marks those tombstones as having no files. It runs only on request.

## Later

- Purging tombstones: forget a tombstone's row data. Whether a slim GUID
  record should remain so Sync never brings the frame back is decided then.
- Reaching a rig's own Target Scheduler copy through the N.I.N.A. Sync
  plugin, with an acknowledged delete like flat history uses.
