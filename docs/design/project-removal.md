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
| GET | `/api/db/{db}/deleted-projects` | Deleted projects that syncs and imports leave out, newest first: `id`, `project_id`, `name`, `removed_at`, `frames` |
| POST | `/api/db/{db}/deleted-projects/{id}/allow` | Lets the next sync or import add the project back. `403` without database management |
| GET | `/api/director/v1/projects/{id}/deletion` | A plan's deletion preview: `name`, each linked database (`catalog_slug`, `catalog_name`, its `project` with `targets`, `exposure_plans`, `frames` and `digest`, or `problem` when it cannot be read) and `refusal` |
| POST | `/api/director/v1/projects/{id}/deletion` | `{digests: {project GUID: digest}}`. Returns `{state: "deleted", name, databases}`, `{state: "changed", preview, deleted}` when the plan or a project changed, or `{state: "stopped", error, deleted}`. `422` with the refusal, `403` without database management |

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
Library's Delete does, one database at a time, then the plan. The projects
have no frames, so none is listed under **Deleted projects**, and a catalog
that syncs from Target Scheduler gets its project back on its next sync. The
plan's deletion removes its links,
preferences, framing and plan drafts, activation record and draft imports
(`MetaStore::delete_plan`).

The plan stays when:

- a rig captured frames for it: delete that project from the Library
  first, where the frames are counted and the name is typed;
- a linked database is not registered on this server, or cannot be read;
- the plan came from a collaboration.

The deletion holds the turn at the rig databases that discovery, plan
listings and activation take, and the sync lock activation holds, so no
activation writes the plan's rigs or links meanwhile. Every project needs
the digest of the preview the person saw before anything is deleted.
`MetaStore::delete_plan` refuses a plan that links a project the deletion
did not handle, or one that came from a collaboration.

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
   frame count) and, under its Id, a `psf_guard_removed_project_frame` row
   for each frame's Id, GUID and lower-case file name, and for each
   calibrated copy paired with a frame.
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

Only a project with frames is kept out. A project without frames has no
files an import could bring back, so its deletion writes no GUID tombstone
and is not listed under **Deleted projects**; its row in
`psf_guard_removed_project` is marked allowed back at once and stays only
for the Restore check. A catalog that syncs from Target Scheduler gets such
a project back on its next sync, and the dialog says so.

- **Sync.** A pull leaves out a source project whose GUID has a tombstone,
  with its targets, exposure plans and frames, as a project filter does:
  no skip line and no skipped count for its rows. A pull or transfer a
  person runs lists each project it left out once ("leave out project …:
  deleted in this catalog"); the automatic sync does not. This covers
  automatic Target Scheduler sync, a full pull and PSF Guard to PSF Guard
  sync.
- **Imports.** `reject_removal::removed_file_names` also returns the
  deleted project's file names, its lights' and their calibrated copies',
  so a folder import or autoimport of the files left on disk skips them;
  `removed_guids` returns their GUIDs.
- **Planning.** With the link and the rig side rows gone, activation does
  not create the project again, and the plan listing has no row to adopt.

A synced catalog's source keeps the project: PSF Guard opens it read-only.
The preview says so, and that N.I.N.A. still schedules the project there
while it is Active.

## Allowing a project back

The Library lists deleted projects under **Deleted projects**. **Allow
again** clears the deletion's GUID and its frames' tombstones, so the next
sync, or an import of the files, adds the project as a new project. Grades
and measurements do not come back with it. The deletion's row stays, with
its date, so Restore in reject removal still refuses a reject removed from
the old project: Target Scheduler may have given its Id to a new project.

The plan listing adopts the returning project into a plan of its own, since
its old plan no longer links the database. To rejoin the old plan, attach
the new plan to it with **Attach a project from another database**.

## Refusals

- A project with a frame reported to a collaboration's coordinator cannot
  be deleted, as such a reject cannot be removed.
- Restore in reject removal refuses a frame whose project was deleted
  after the frame was removed.
- A plan imported from a collaboration keeps its projects.
- A preview older than the project removes nothing and returns a new
  preview.

## Not covered

- A frame recorded under another project on one of the deleted project's
  targets is deleted with it, and its GUID is tombstoned. While the source
  still has the frame, each pull lists it as skipped.
- Remote image intake does not read the tombstones, as it does not read
  reject removal's. An upload of a deleted project's file arrives as a new
  frame.
- There is no restore of the deleted rows. The tombstones keep the frame
  Ids and names, not the rows; Allow again lets a sync or import add the
  project anew.
- When Planning cannot forget the project, nothing retries it. Deleting
  the plan removes the link.
