# Data transfer, database merge, and remote sync

Status: design approved in principle. The first implementation slice adds
local grade push and server-owned preview IDs with expiry and stale checks.

## Purpose

PSF Guard needs one safe way to:

- import FITS folders into a catalog;
- merge one Target Scheduler database into another;
- send planning changes or reviewed grades to another database;
- do the same work through a remote PSF Guard server; and
- later let a N.I.N.A. plugin provide the telescope-side endpoint.

The design must not require Syncthing, Dropbox, or another file copier to move
a live SQLite database. N.I.N.A. may write that file while an outside process
copies it, and a copied database gives neither side a useful conflict preview.

Original FITS files are outside the first remote-sync protocol. A transfer can
copy database rows and stored thumbnails, then report which image files resolve
on the destination. File transfer can be added later as a separate action.

## Current state

The repository already has most of the local merge rules:

- FITS import scans headers and runs a dry preview before the UI offers Apply.
- Per-database remote FITS ingest is opt-in, authenticated, and imports one
  verified light frame through the same target-resolution path.
- `sync pull` merges scheduler structure and captures by stable GUID.
- Pull keeps a reviewed destination grade and only fills a Pending grade.
- `sync planning` sends projects, targets, templates, plans, and rule weights
  without changing capture history, plan progress, or grades.
- `sync grades` sends grading state and reject reasons by image GUID.
- The management API and Settings UI expose local pull and planning push.

The missing pieces are:

- one clear transfer workspace instead of controls repeated under each catalog;
- durable job and preview state;
- local-file selection without first keeping a catalog in the registry;
- remote peers, authentication, and a versioned wire format; and
- an endpoint that a future N.I.N.A. plugin can implement.

## Terms

**Catalog endpoint**
: A readable or writable source of catalog records. It can be a registered
  local SQLite database, a file selected in the desktop app, a remote PSF Guard
  server, or a future N.I.N.A. plugin.

**Merge**
: A one-way, additive copy into a destination. It can insert and update rows
  under the rules below. Version 1 never deletes destination rows.

**Preview**
: A frozen source snapshot, selected options, destination preconditions, and
  the exact changes an Apply would make.

**Apply**
: One transactional write of an approved preview. Apply is not a fresh sync
  request.

This is not a three-way merge. Target Scheduler rows do not carry enough edit
history to prove that both sides changed the same field since a common base.
Direction and data class therefore define which side wins.

## User operations

### Import FITS folders

Source: one or more local folders.

Destination: one local writable catalog.

The importer reads FITS headers, attaches frames to a confirmed existing
target when possible, and creates structure for the rest. Pixel quality work
is a separate background action and defaults off.

### Merge catalog

Source: any catalog endpoint.

Destination: any writable catalog endpoint.

Copies projects, targets, exposure templates, exposure plans, rule weights,
captures, and optional stored thumbnails. Matching uses stable GUIDs.
Destination reviewed grades win. New captures retain the source grade.

### Send planning

Source planning fields win. The destination keeps capture history, plan
`acquired` and `accepted` counts, images, grades, and reject reasons.

### Send grades

Only `gradingStatus` and `rejectreason` change. Images match by stable GUID.
The source wins. The UI defaults to reviewed grades only, so a Pending source
row cannot erase a telescope decision by accident. Project, target, and grade
filters remain available.

## Merge rules

| Data | Match | Default rule |
| --- | --- | --- |
| Project, target, template, plan | Stable GUID | Directional source wins |
| Rule weight | Project GUID plus name | Directional source wins |
| Captured image | Stable GUID | Insert or update capture fields |
| Existing reviewed grade during merge | Image GUID | Destination wins |
| Existing Pending grade during merge | Image GUID | Fill from source |
| Explicit grade push | Image GUID | Source grade and reason win |
| Destination-only row | None | Keep |
| Duplicate or empty GUID | None | Skip and report |
| Non-null parent that cannot map | Parent GUID | Skip and report |
| Same-looking object with another GUID | Name and coordinates | Warn; never auto-merge |

The preview can offer an explicit target mapping for likely duplicates in a
later phase. Name or coordinate similarity must never silently replace GUID
identity.

## Interface

Add a full Data Transfer page reached from Settings. Do not add another item to
the already busy image toolbar.

```
Data Transfer

[ Import FITS folders ] [ Merge catalogs ] [ Send changes ]

Source                               Destination
[ Local / Remote ]  Review copy  -> [ Local / Remote ]  Telescope

Data
[x] Projects and targets       [x] Captures
[x] Plans and templates        [ ] Stored thumbnails
[ ] Reviewed grades

Scope
[ All projects ] [ Optional target ] [ Optional grade ]

                                      [ Preview changes ]
```

Before a preview, the page has no Apply action. The preview page contains:

- source, destination, direction, snapshot time, and expiry;
- inserted, updated, unchanged, and skipped counts per data class;
- grade transitions;
- detailed row changes;
- duplicate GUID, schema, and missing-parent warnings;
- likely duplicate targets that need a mapping choice;
- stored-thumbnail bytes;
- resolved and missing image-file counts; and
- a clear note that original FITS files are not transferred.

Only a valid preview shows **Apply this preview**.

The page also shows recent and running jobs. Reloading the page must recover
their state.

### Local files

The desktop app can choose a source or destination SQLite file with a native
picker. The backend opens a source read-only and an existing destination
read-write. A selected file does not need to remain in the normal catalog
registry.

Browser mode can use registered local catalogs. It must not accept arbitrary
server filesystem paths from an HTTP request.

### Remote peers

Settings stores a peer name, HTTPS base URL, server-held credential, allowed
remote catalog, optional path mappings, and granted capabilities. The browser
never receives the credential.

The connection test shows:

- peer and protocol version;
- remote product and Target Scheduler schema versions;
- readable and writable catalogs allowed by the credential;
- read, merge, planning-write, and grade-write capabilities; and
- the last successful connection and sync.

## Preview and apply

The local API now records each dry run under an opaque preview ID and Apply
accepts only that ID. Preview takes an online SQLite snapshot of the source.
Apply reads that snapshot, takes the destination write lock, and checks the
destination fingerprint in the same transaction as the write. Source edits
wait for the next transfer; destination edits return `409 Conflict`.
Preview IDs are one-use, and guarded applies run one at a time so two requests
cannot both pass the same precondition check.

Complete the model in phases:

1. Read the source into an immutable, versioned transfer bundle.
2. Plan that bundle against the destination without writing destination rows.
3. Save the bundle, options, summary, detailed changes, and destination
   preconditions under an opaque preview ID.
4. Return the preview ID and expiry to the UI.
5. Apply only by preview ID.
6. Recheck destination preconditions under the destination write lock.
7. Return `409 preview_stale` if relevant destination rows changed.
8. Back up the destination, apply in one transaction, and save an audit result.

If the source changes after preview, Apply still uses the frozen source bundle.
Those later source changes arrive in the next transfer. If the destination
changes, the preview becomes stale because its conflict results may no longer
hold.

Previews and jobs live below the server cache, not only in React state. The
default preview lifetime is 30 minutes. Completed job records can persist
longer with a bounded count and size.

## Local API

The UI talks only to its local PSF Guard server.

```
GET    /api/data-transfer/capabilities
GET    /api/data-transfer/endpoints
POST   /api/data-transfer/previews
GET    /api/data-transfer/previews/{preview_id}
DELETE /api/data-transfer/previews/{preview_id}
POST   /api/data-transfer/previews/{preview_id}/apply
GET    /api/data-transfer/jobs
GET    /api/data-transfer/jobs/{job_id}
```

`POST /previews` accepts endpoint references, an operation, data selection,
and filters. It never accepts `apply=true`. Apply has its own endpoint and
requires the opaque preview ID.

The current guarded local routes are:

```
POST /api/databases/{id}/sync/preview
POST /api/databases/{id}/sync/previews/{preview_id}/apply
```

The existing `/api/databases/{id}/sync` endpoint remains during migration.
Omitting `dry_run` means preview, never Apply. New UI code uses only the
guarded routes.

## Remote protocol

The coordinator uses a versioned protocol rather than copying SQLite:

```
GET  /api/sync/v1/capabilities
POST /api/sync/v1/exports
GET  /api/sync/v1/exports/{export_id}
POST /api/sync/v1/previews
GET  /api/sync/v1/previews/{preview_id}
POST /api/sync/v1/previews/{preview_id}/apply
GET  /api/sync/v1/jobs/{job_id}
```

For a pull, the coordinator requests a bundle from the remote source and plans
and applies it locally. For a push, it creates a local bundle and sends it to
the remote destination, which owns preview and Apply.

The bundle is compressed and contains:

- protocol and producer versions;
- source catalog identity and Target Scheduler schema facts;
- operation and filters;
- table schemas needed to preserve shared columns;
- rows keyed by stable GUID;
- optional stored-thumbnail chunks;
- source snapshot metadata; and
- a payload digest.

The protocol sets compressed and expanded size limits, row limits, timeouts,
and a bounded thumbnail budget. It rejects unknown required features rather
than guessing.

## N.I.N.A. plugin

A later N.I.N.A. plugin can implement the remote protocol at the telescope.
The plugin does not need to expose the SQLite file.

The plugin can:

- read a consistent Target Scheduler snapshot;
- expose schema and capability facts;
- export planning and capture history;
- preview and transactionally apply planning or grade changes;
- report new captures;
- expose configured image-history roots or manifests; and
- notify PSF Guard after a capture changes the catalog.

The first plugin release should remain manual and preview-first. Automatic
background sync can follow after the audit trail and conflict behavior have
real use.

### Remote image ingest

Image transfer is independent of Target Scheduler catalog sync. A capture
client can post a light frame directly to one opted-in PSF Guard database:

```http
POST /api/db/{db_id}/images/upload
Authorization: Bearer <per-database-upload-token>
X-PSF-Guard-Database-ID: <db_id>
X-Content-SHA256: <64 lowercase hexadecimal characters>
Content-Type: multipart/form-data

image=@capture.fits
```

The database settings select one of that database's registered image roots as
the receive directory. The server requires the URL slug and echoed database ID
to agree, authenticates with the selected database's salted token hash, streams
at most 512 MiB to a sibling temporary file, verifies SHA-256 and FITS headers,
and publishes without overwriting an existing basename.

The normal one-frame importer then resolves an existing target by object name
or coordinates and reuses its exposure plan. If no target matches, it builds
the project, target, template, and plan from FITS headers. This path therefore
works with an existing Target Scheduler catalog and with a fresh PSF Guard
catalog whose user never installed Target Scheduler.

Identical retries are idempotent. The response returns the resolved database,
project, target, and image IDs. A basename already registered elsewhere or an
existing receive file with different content returns `409 Conflict`.

## Security

Remote sync is disabled by default.

- Require HTTPS except for loopback development.
- Store peer credentials in the backend or desktop secret store.
- Store only salted token hashes on the receiving server.
- Scope tokens to catalog IDs and actions.
- Never expose arbitrary file paths or SQL through the protocol.
- Rate-limit preview and export creation.
- Log peer, catalog, operation, preview digest, actor, counts, and result.
- Redact credentials and local paths from logs returned to a remote caller.
- Keep database-management and sync permissions separate.

## Concurrency and recovery

Use one mutation coordinator per destination catalog. Import, merge, planning
push, grade push, and other database writes must not overlap on that catalog.
Reads and previews can run concurrently where SQLite permits.

Each job records queued, snapshotting, planning, waiting-for-approval,
applying, complete, cancelled, stale, or failed. A restart recovers durable
non-terminal jobs or marks an interrupted Apply for inspection. Apply remains
transactional, so it cannot leave half a merge committed.

## Delivery

### Phase 1: complete the existing local path

- [x] Add grade push to the management API and Settings UI.
- [x] Default grade push to reviewed rows only.
- [x] Make omitted `dry_run` mean preview.
- [x] Improve summaries and tests.

### Phase 2: immutable local previews

- [ ] Refactor sync cores into bundle, plan, and apply stages.
- [x] Add preview IDs, expiry, frozen source snapshots, and atomic destination
  stale checks.
- [ ] Add destination backups and audit jobs.
- [x] Add the Data Transfer workspace for registered local catalogs.
- [ ] Move FITS import onto the same preview and job model.

### Phase 3: desktop local files

- Add Tauri source and destination file pickers.
- Pass selected files through native commands, not arbitrary browser paths.
- Add optional path mappings and file-resolution reporting.

### Phase 4: remote PSF Guard peers

- Add scoped peer credentials and capability discovery.
- Add compressed export bundles and remote preview/apply.
- Test interrupted transfers, limits, stale previews, and retries.

### Phase 5: N.I.N.A. plugin

- Implement the same capability, export, preview, apply, and job contract.
- Add capture notifications and image-history manifests.

## Verification

Rust tests must cover:

- every merge policy and grade transition;
- dry preview making no row changes;
- source snapshot immutability;
- stale destination rejection;
- duplicate GUID and missing-parent handling;
- transaction rollback and backup failure;
- protocol version, authentication, and size limits; and
- restart recovery.

Frontend tests must cover:

- Apply absent before preview;
- changed options invalidating a preview;
- detailed counts and warnings;
- reload recovery;
- stale-preview handling;
- local file selection in Tauri;
- remote capability errors; and
- successful local and remote preview/apply flows in Playwright.
