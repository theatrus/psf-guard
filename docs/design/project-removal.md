# Project removal

Status: **Implemented**, for one catalog's project and for a plan with
nothing captured across its databases.
Last updated: 2026-10-22

A person can delete a project from a catalog: its targets, exposure plans
and frame rows, and what PSF Guard keeps about them. The image files stay
on disk. The core is in `src/commands/project_removal.rs` and the routes are
in `src/server/project_removal.rs`.

## Where to run it

In the Library, open a project's **Edit project** panel and click **Delete
project...**, or click **Delete** beside a project under **Archived
projects**. The dialog shows what goes and what stays, then **Delete
project** deletes it. When the project has frames, the dialog asks you to
type the project's name first. Deleting needs an editor's account and a
server started with `--allow-database-management`; without it, the dialog
explains why the project cannot be deleted.

| Method | Route | Request and response |
| --- | --- | --- |
| GET | `/api/db/{db}/projects/{id}/removal` | The preview: name, GUID, state, `targets`, `exposure_plans`, `frames` by grade, `protected_frames`, `linked_plan`, `synced_from`, `allowed` and `digest` |
| POST | `/api/db/{db}/projects/{id}/removal` | `{digest}`. Returns `{state: "removed", …}` with the counts and the plan that no longer links the database, or `{state: "changed", preview}` when the project changed since the preview. `403` without database management, `409` for frames reported to a collaboration or a plan from one, `404` for a project that is gone |
| GET | `/api/db/{db}/deleted-projects` | Deleted projects that syncs and imports leave out, newest first: `id`, `project_id`, `name`, `removed_at`, `frames`. A deletion without frames is listed once a sync has left the project out |
| POST | `/api/db/{db}/deleted-projects/{id}/allow` | Lets the next sync or import add the project back. `403` without database management |
| GET | `/api/director/v1/projects/{id}/deletion` | A plan's deletion preview: `name`, each linked database (`catalog_slug`, `catalog_name`, its `project` with `targets`, `exposure_plans`, `frames` and `digest`, or `problem` when it cannot be read), `collaboration` for a plan from a collaboration (as Leave previews it) and `refusal` |
| POST | `/api/director/v1/projects/{id}/deletion` | `{digests: {project GUID: digest}}`. Returns `{state: "deleted", name, databases, coordinator?}`, `{state: "changed", preview, deleted}` when the plan or a project changed, or `{state: "stopped", error, deleted}`. `422` with the refusal, `403` without database management, or for a plan from a collaboration without an interactive editor |

The preview reads on a read-only connection. Neither route runs as a job:
the deletion is one transaction of set-based statements, like a project
merge. Target Scheduler has no index on `imagedata.acquiredimageid`, so a
statement for each frame would scan the thumbnails once per frame; one
statement scans them once.

## Deleting a plan

Under **Plans with nothing captured yet**, each plan has a delete button
beside **Rename**, for editors on a server with database management. The
dialog lists the plan's project in each database it links, with its
targets and exposure plans. **Delete plan** deletes each project as the
Library's Delete does, one database at a time, then the plan. Each
deletion writes a tombstone with `origin` `local`, so later syncs leave the
project out. The projects have no frames, so none is listed under
**Deleted projects** until a sync leaves it out. The plan's deletion
removes its links,
preferences, framing and plan drafts, activation record and draft imports
(`MetaStore::delete_plan`).

The plan stays when:

- a linked database is not registered on this server, or cannot be read;
- the plan came from a collaboration and frames are in reports to it,
  queued or sent: leave the collaboration from the plan first (see
  [Leaving a collaboration](collaboration.md#leaving-a-collaboration)),
  then delete its projects from the Library, since a plan with reported
  frames has captured frames;
- a rig captured frames for it: delete that project from the Library
  first, where the frames are counted and the name is typed.

A plan from a collaboration with nothing reported leaves the collaboration
as it is deleted. The dialog names the collaboration's host and project and
lists what leaving removes. `MetaStore::delete_plan` removes the plan's
collaboration records in the same transaction that deletes the plan, as
`MetaStore::leave_collaboration` does, and the server then declines the
plan's tasks at the coordinator. The deleted projects take their
`psf_guard_collaboration_plan` rows with them, so no assignment needs ending.
Deleting such a plan needs an interactive editor, as joining a project does.
When a project changes part way, the plan stays in its collaboration and
forgets the projects already deleted, as for any plan.

The deletion holds, in this order, the collaboration gate that nightly
intake, automatic reports and every interactive collaboration operation
take (only for a plan from a collaboration), the turn at the rig databases
that discovery, plan listings and activation take, and the sync lock
activation holds. No intake, report or activation writes the plan's rigs,
links or collaboration records meanwhile. Nightly intake activates work
while it holds the gate, so the gate comes first. A plan of its own does
not wait for the gate; when an import linked it meanwhile, the deletion
answers `changed` with the new preview. The deletion runs in its own task,
so a browser that goes away cannot stop it part way. Every project needs the digest of the preview the person
saw before anything is deleted. `MetaStore::delete_plan` refuses a plan
that links a project the deletion did not handle, or one with frames
queued for or sent in a collaboration report.

When a project changed after the preview, the deletion stops there and
answers `changed` with a new preview. When deleting fails, it answers
`stopped` with the error. Either way the projects already deleted stay
deleted and are listed, and the plan forgets them, so it does not list
databases where its project is gone. Deleting the plan also takes it out of
every saved project order. A database the plan links but this server does
not have keeps the plan, and the refusal names its rig; so does a database
with two projects under the plan's GUID.

## Decisions

These were settled with the owner on 2026-10-22.

- **Delete, not hide.** Closing a project already moves it into the
  Library's archive; **Reopen** sets it Inactive again.
- **Rows go, files stay.** Frames, grades and PSF Guard's measurements of
  them leave the catalog. The FITS and XISF files stay where they are, as do
  calibrated copies PSF Guard made of them.
- **The plan drops the rig.** A plan that linked the project keeps its
  framing and its other databases, and drops the work it planned for this
  rig. A plan with no database left moves to Plans with nothing captured
  yet, where it can be deleted.

## What goes

One transaction, after checking the preview's digest (project name and
GUID, target, exposure plan and frame row Ids). The frames are the
project's and any recorded under another project on one of its targets.

1. Tombstones: a `psf_guard_removed_project` row (Id, GUID, name, date,
   frame count, `origin` `local`, and `left_out_at`, the first time a sync
   left the project out) and, under its Id, a
   `psf_guard_removed_project_frame` row for each frame's Id, GUID,
   lower-case file name and capture time (`acquired_at`, from
   `acquireddate`), and for each calibrated copy paired with a frame.
2. Each frame's `imagedata`, `psf_guard_remote_image_file`,
   `psf_guard_flat_light_angle`, `psf_guard_frame_derivative`,
   `psf_guard_archive`, `psf_guard_rejected_at`, `psf_guard_ts_sync_grade`,
   `psf_guard_ts_sync_pending` and `psf_guard_flat_light_choice` rows, then
   the `acquiredimage` rows.
3. Rejects removed from the project earlier keep their tombstones for the
   trash, without their target and exposure plan Ids, so a new target that
   takes an Id does not count them.
4. Each target's `overrideexposureorderitem`, `filtercadenceitem`,
   `flathistory` and `exposureplan` rows, and Planning's
   `psf_guard_director_target`, `psf_guard_director_plan`,
   `psf_guard_collaboration_plan` and `psf_guard_scheduler_flat_history`
   rows, then the targets.
5. `ruleweight`, `psf_guard_director_project`,
   `psf_guard_project_processing`, then the project.

Steps 2 to 5 are `delete_project_rows`, which a sync also uses (see
[Deleting in other catalogs and rigs](#deleting-in-other-catalogs-and-rigs)).
Target Scheduler declares no cascades, so each table is deleted from by
name; optional tables are probed first. Exposure templates belong to the
profile and stay. A GUID that another project in the catalog also has, in
a damaged catalog, identifies neither: the deletion then records no GUID
and leaves Planning alone.

After the commit the server deletes the frames' cache files (previews,
annotated views, star lists, statistics, plate solves, satellite
predictions), forgets their spatial evidence, drops the project's stack
indices and refreshes the Library's counts. Target Scheduler gives a
deleted row's Id to the next row, which must not inherit any of them. The
stack janitor deletes the stacks once no index lists them.

Planning then forgets the project: the link, the plan's contributions for
the database's rig, and the rig in the plan's activation record
(`MetaStore::forget_source_project`). This runs after the commit. Run first,
a refused deletion would leave a project the next plan listing adopts into
a new plan. When it fails, the answer carries `plan_error` and the plan
still lists the database, with no project there. A copy of a catalog file
that shares another file's identity changes nothing in Planning, which uses
the other file for that catalog. While another project of the same catalog
is still linked to the plan, the plan keeps the rig's work.

## How syncs and imports leave it out

Every deletion keeps its project out, with frames or without. A project
without frames is listed under **Deleted projects** only once a sync has
left it out (`left_out_at` is set): until then, keeping it out changes
nothing anyone would see. The list shows a deletion when
`allowed_back_at IS NULL AND (frames > 0 OR left_out_at IS NOT NULL)`. A
table created before `left_out_at` gains the column the next time PSF
Guard writes to it, and lists the deletions with frames meanwhile.
Deletions made before this change for projects without frames were marked
allowed back at once; they stay that way.

- **Sync.** A pull leaves out a source project whose GUID has a tombstone,
  with its targets, exposure plans and frames, as a project filter does:
  no skip line and no skipped count for its rows. A pull or transfer a
  person runs lists each project it left out once ("leave out project …:
  deleted in this catalog"); the automatic sync does not. This covers
  automatic Target Scheduler sync, a full pull and PSF Guard to PSF Guard
  sync. Leaving the project out sets the deletion's `left_out_at` the
  first time. A dry run does this inside the transaction it rolls back, so
  a preview changes nothing.
- **Frames that still arrive.** A pull into a catalog adds the left-out
  frames' GUIDs, lower-case file names (the basename of the metadata
  `FileName`) and capture times to the deletion's frame tombstones, when
  they are not there yet, inside the sync's transaction. An upload or
  import of those files is then skipped as removed. The deletion records
  the count of the source's frames for the project and a hash of their Ids
  and GUIDs (`left_out_source_frames`); a later sync reads the frames again
  only when they change, so the automatic sync does not parse them every
  time, and a second source with as many frames is still read.
- **Moved targets.** A target a person moved here from the deleted project
  to a project that stays (Move to, #971) keeps arriving, although the
  Target Scheduler source still lists it under the deleted project. The
  sync finds the source target's GUID here under a project that is not
  deleted, upserts the target with that project, and brings its exposure
  plans and frames under it. Its frames are not added to the deletion's
  frame tombstones.
- **Frames follow their target.** A frame whose own project is left out
  because it was deleted here, but whose target arrives, because the
  target's project arrives or the target was moved here, arrives with the
  target, under the target's project here. A frame of a project that a
  name filter left out stays out. This covers a rig whose target a planning sync moved while
  its earlier frames kept the old project's Id. No such frame, and no frame
  whose GUID this catalog already has, is added to the frame tombstones,
  so its upload is not refused.
- **Imports.** `reject_removal::removed_file_names` also returns the
  deleted project's file names, its lights' and their calibrated copies',
  so a folder import or autoimport of the files left on disk skips them;
  `removed_guids` returns their GUIDs.
- **Planning.** With the link and the rig side rows gone, activation does
  not create the project again, and the plan listing has no row to adopt.

A synced catalog's source keeps the project: PSF Guard opens it read-only.
The preview says so, and that N.I.N.A. still schedules the project there
while it is Active.

## Deleting in other catalogs and rigs

A sync applies the source catalog's deletions to its destination, in every
direction that writes project rows: a full pull, the automatic arrival
sync, a pull from a peer and a planning push (`apply_source_deletions` in
`src/commands/sync/pull.rs`). `sync_structure` follows the source's Allow
again first, so a project allowed back returns in the same sync, and
deletes last, after the targets and exposure plans are synced. A target
the source moved out of the deleted project into another one is then
already under its new project here, with its Id and every row that refers
to it: order and cadence items, flat history, and Planning's
`psf_guard_director_target`, `psf_guard_director_plan` and
`psf_guard_scheduler_flat_history` rows. Its `psf_guard_director_target`
row named the deleted project, so `delete_project_rows` deletes it, as Move
to does: mosaic layouts and collaboration activations read the project from
that row. While a target the sync writes is still under a project the
source deleted (the source has it under a project deleted here, and this
catalog moved it), that project stays: `keep project "Q": deleted in the
source catalog, but its target "T" still arrives from the source`. For
each GUID in the
source's `psf_guard_removed_project` that is not allowed back and that
the source has no project for:

- When the destination has a project with that GUID and nothing was
  captured for it there (no `acquiredimage` row with its project Id or on
  one of its targets, and no removed reject that Restore can bring back),
  the sync deletes it with `delete_project_rows` and reports `delete
  project "M31": deleted in the source catalog`. The project, target and
  exposure plan counts include it as `deleted`, and the sync preview shows
  how many projects a transfer deletes.
- When the destination has frames for it, the project stays and the sync
  reports `keep project "M31": deleted in the source catalog, but it has 3
  exposures here`. A removed reject keeps it too, with its own line.
- When this catalog allowed the project back after the source deleted it
  (an allowed-back row for the GUID dated after the source's
  `removed_at`), the project stays: that choice is newer. See
  [Allowing a project back](#allowing-a-project-back).
- When one of its targets here is a target the source has under another
  project, the sync could not move it (a name filter left that project
  out, for one), and the project stays: `keep project "M31": deleted in
  the source catalog, but its target "M31" could not move to its project
  in the source`.
- When the project belongs to a collaboration here, it stays: its plan
  came from a collaboration, or one of its frames or removed rejects was
  reported to one. The server passes those GUIDs to the sync
  (`SourceDeletionRules`, from `sync_deletion_rules`), as **Delete
  project** refuses such a project. A planning push into a `ts:<slug>`
  connection reads the collaborations of catalog `<slug>`, whose rig file it
  writes. Director file delivery passes none: its source is the activated
  project alone, with no tombstone table, so it deletes nothing.
- When the server could not read Planning to check for collaborations,
  the automatic Target Scheduler sync holds its deletions (`held`) and
  still imports captures; a later sync tries again. That sync reads
  Planning only when its source carries deletions.
- When the run was limited to the deletions its preview listed and this
  one was not among them, the project stays ("not in the preview; preview
  again"). A pull from a peer applies a fresh export rather than the one it
  previewed, so the page sends back the preview's `deleted_projects` with
  the apply.
- When two projects in the destination have the GUID, neither is deleted
  and the sync reports a skip line. A project filter limits deletions to
  destination projects whose names match it.

`sync_structure` takes the kind of destination explicitly
(`SyncDestination`):

- **Catalog**: the destination is a PSF Guard catalog: a full pull, the
  arrival sync, a pull from a peer, a merge or planning push a peer sends
  to this server, and a planning push from Settings into another
  registered catalog. The sync also records the deletion there as a
  tombstone with no frames and `origin` `sync`, so a later sync from a rig
  that still has the project leaves it out. The tombstone is dated with
  the source's `removed_at`, when the person deleted the project, so every
  catalog compares Allow again with that one date (now only when the
  source gave none). It is not listed until a sync leaves the project
  out.
- **Scheduler**: the destination is a Target Scheduler file: a planning
  push into a `ts:` connection, Director file delivery, and `psf-guard sync
  planning`. Only the rows go; the sync writes no PSF Guard table there.

`execute_scheduler_sync_paths` decides between them for a planning push:
a destination that is a registered catalog is **Catalog**, anything else
(a `ts:` connection) is **Scheduler**, and a Director delivery is always
**Scheduler**. A pull always writes a catalog.

Everything runs inside the caller's transaction, so a dry run deletes and
records nothing. The summary lists the GUIDs of the projects it deleted.
After the commit, each server path that writes a catalog (the Settings and
remote Sync routes through `execute_scheduler_sync_paths`, the automatic
Target Scheduler sync and a pull from a peer) has Planning forget them
through `forget_synced_deletions`, which calls the same
`forget_source_project` as **Delete project**. A failure is logged, and
the plan then still links a project that is gone.

### Peers

A remote PSF Guard reads deletions from the `psf_guard_removed_project`
table of a merge or planning bundle: `project_guid` (a UUID), `name`,
`removed_at` and `allowed_back_at`, for standing deletions and ones
allowed back. A peer that reads it advertises `project_tombstones_v1`; a
client pushes the table only to a peer that advertises it, and an export
includes it only when the request sets `include_project_tombstones`. A
bundle without the table deletes nothing. See
[Data transfer](data-transfer.md#deleted-projects).

### Uploads

An upload of a light that a project deletion took out is refused with
`410 Gone` and a message that names the project and explains how to allow
it back (**Deleted projects**, **Allow again**). An upload of a light
removed as a reject is refused with `410 Gone` too, with a message that
explains how to bring the frame back (**Purge rejects**, **Restore**). The
receive directory is not written.

A tombstone matches an upload by file name and by capture time within 2 s
(`CAPTURE_IDENTITY_TIME_TOLERANCE_SECS`): N.I.N.A. file patterns such as
`$$TARGETNAME$$\$$FRAMENR$$` reuse `0001.fits` across targets. A project
deletion's time is its frame tombstone's `acquired_at`; a reject's comes
from `psf_guard_removed_image.acquired_at`, filled at removal, copied from
the saved rows before a purge empties them, and backfilled from
`rows_json` when the column is added. A tombstone without a time, or an
upload without `DATE-OBS`, cannot show the upload is the removed frame, so
the upload imports. When only the name matches, the upload imports as a
new frame (`ImportOptions::admit_removed_names`). A folder import still
skips removed names by name alone. A 410 is not a failure worth retrying: the
N.I.N.A. plugin retries automatically only on no response, 408, 409, 429
and 5xx.

### The N.I.N.A. plugin

The plugin applies a planning or merge bundle to its rig's Target
Scheduler database itself, so PSF Guard's sync engine does not run there.
Plugin versions with project deletion support
([theatrus/psf-guard-nina-plugin#26](https://github.com/theatrus/psf-guard-nina-plugin/pull/26))
ask for `include_project_tombstones` when the server advertises
`project_tombstones_v1`, and delete a frameless project on the rig when
PSF Guard deleted it. They run the deletions after the target upserts, so
a target PSF Guard moved out of a deleted project is re-parented on the
rig, and they keep the old project while a target the source has under a
live project cannot move, as PSF Guard's own sync does. They hold an
upload refused with 410 (a deleted project's or a removed reject's file)
without retrying it: **Retry refused uploads** uploads it again after you
allow the project back or restore the frame, and **Forget refused
uploads** removes the held uploads from the queue while the image files
stay on disk. Older plugin
versions do not delete projects: their rig keeps the project, and the
next pull leaves it out of PSF Guard again. They show a refused upload as
a blocked job.

## Allowing a project back

The Library lists deleted projects under **Deleted projects**. **Allow
again** dates the deletion allowed back (`allowed_back_at`) and deletes its
frames' tombstones, so the next sync, or an import of the files, adds the
project as a new project. Grades and measurements do not come back with
it. The deletion's row stays, with its GUID and dates: Restore in reject
removal still refuses a reject removed from the old project, since Target
Scheduler may have given its Id to a new project, and syncs pass the
choice on. Every reader that means "deleted now" reads only rows with
`allowed_back_at IS NULL`.

Syncs carry Allow again between catalogs by comparing dates:

- A source's standing deletion is not applied here when this catalog
  allowed the project back after the source's `removed_at`. Otherwise a
  catalog that followed the deletion would undo Allow again on the next
  sync.
- A source's deletion that was allowed back allows back each standing
  deletion here of the same GUID that a sync recorded (`origin` `sync`)
  and dated before the source's `allowed_back_at`, with that date, and
  deletes its frame tombstones. The sync reports `allow project "M31" back:
  allowed back in the source catalog`. It deletes and creates no project
  itself; the project returns with the same sync or the next one, from
  whichever source still has it.
- A deletion a person made in this catalog (`origin` `local`) stands until
  **Allow again** here, whatever another catalog chose. So does any
  deletion dated after the source's Allow again.

For example, catalog A deletes P at t1 and C pulls from A, recording the
deletion dated t1. A allows P back at t3 and a rig brings P back to A. B
pulls from C and deletes P, dated t1. When B pulls from A, B's deletion
(t1, from a sync) is allowed back as of t3 and P returns to B. When A
pulls from B, P stays in A. Had B dated its deletion when it synced, after
t3, A would have deleted P again.

The plan listing adopts the returning project into a plan of its own, since
its old plan no longer links the database. To rejoin the old plan, attach
the new plan to it with **Attach a project from another database**.

## Refusals

- A project with a frame reported to a collaboration's coordinator cannot
  be deleted, as such a reject cannot be removed.
- Restore in reject removal refuses a frame whose project was deleted
  after the frame was removed.
- A plan imported from a collaboration keeps its projects. The refusal
  names the plan and says where to go next: choose **Leave collaboration**
  under **Collaboration assignments** in the plan, then delete the project.
  After the plan leaves, its reported frames are no longer protected, as
  no correction will be sent.
- A preview older than the project removes nothing and returns a new
  preview.

## Not covered

- A frame recorded under another project on one of the deleted project's
  targets is deleted with it, and its GUID is tombstoned. While the source
  still has the frame, each pull lists it as skipped.
- A sync that deletes a project clears no cache files: nothing was
  captured for the project in the destination.
- There is no restore of the deleted rows. The tombstones keep the frame
  Ids and names, not the rows; Allow again lets a sync or import add the
  project anew.
- When Planning cannot forget the project, nothing retries it. Deleting
  the plan removes the link.
