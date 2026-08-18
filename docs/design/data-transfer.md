# Data transfer, database merge, and remote sync

Status: local and remote merge, planning, grade, and image-upload paths are
implemented. Native one-off file handles and the N.I.N.A. plugin remain
follow-up work.

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

Original FITS files stay outside the database-bundle protocol. A transfer can
copy database rows and stored thumbnails, then report which image files resolve
on the destination. The remote intake endpoint handles FITS upload as a
separate action.

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

The remaining pieces are:

- one clear transfer workspace instead of controls repeated under each catalog;
- durable job and preview state;
- local-file selection without first keeping a catalog in the registry;
- a N.I.N.A. plugin that calls the remote sync and intake endpoints.

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
captures, and optional stored thumbnails. Thumbnails are opt-in on a merge
export (`include_thumbnails`); they dominate a catalog's size and most
round trips do not need them. Matching uses stable GUIDs.
Destination reviewed grades win. New captures retain the source grade. Plan
`acquired` counts copy from the source; `accepted` counts are recomputed
from the merged grades, because local decisions can differ from the
source's counter.

### Send planning

Source planning fields win. The destination keeps capture history, plan
`acquired` and `accepted` counts, images, grades, and reject reasons.

### Send grades

Only `gradingStatus` and `rejectreason` change on images, and the
destination's `exposureplan.accepted` counters are recomputed from the
resulting grades so the scheduler plans against them. Images match by
stable GUID.
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
POST /api/sync/v1/previews/{preview_id}/refresh
GET  /api/sync/v1/jobs/{job_id}
```

Clients send `Prefer: respond-async` when a preview may outlive an HTTP proxy
timeout. The server returns `202 Accepted` with a job ID, builds the preview in
the background, and exposes the eventual preview or failure through the job
route. Omitting the preference keeps the original synchronous response for
older clients. Retrying the same `Idempotency-Key` returns the existing job
instead of building a second preview.

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
- an optional payload digest.

The protocol sets compressed and expanded size limits, row limits, timeouts,
and a bounded thumbnail budget. It rejects unknown required features rather
than guessing.

The digest is a courtesy checksum, not a credential. It carries no key, so it
says nothing about who built the bundle; the bearer token does that, and TLS
already covers truncation. The receiver therefore accepts a bundle whose
digest is absent or stale. Enforcing it would pin one canonical JSON encoding,
and reordering a single field would then reject every plugin already shipped.

A client that wants an integrity check on a pulled export must not
re-serialize the bundle and compare against `payload_sha256` — that check
only passes when the client reproduces the server's JSON writer byte for
byte, so it fails between implementations by construction. Instead, the
export endpoints stamp `X-Content-SHA256` on the response: the SHA-256 of
the exact body bytes sent. Hash the raw body before parsing and compare.
This mirrors the upload path, where the client sends the same header over
the raw image bytes.

A bundle must carry the tables its operation acts on — `project`, `target`,
and `acquiredimage` for grades, `project`/`target`/`exposureplan` for planning,
and both sets for a merge. Other tables in the operation's set may be omitted;
the receiver creates them empty from its own schema, and the merge finds
nothing to do. The current receiver accepts up to 512 MiB and one million rows
per bundle, and bounds the exports it builds by the same row limit.

A grade push carries `project` and `target` even though it writes neither,
because the receiver reads its source rows through the scheduler's own
project/target join. A bundle of bare `acquiredimage` rows is not a Target
Scheduler database and the read fails outright.

Each database opts into this protocol on its own. Holding a valid key is not
enough: the operator ticks **Accept remote scheduler sync** for that database,
separately from **Accept remote image uploads**.

### Speaking the protocol, not only answering it

`server/remote_sync.rs` answers `/api/sync/v1`; `sync_client.rs` speaks it.
Without both, a PSF Guard could only ever be the far end of somebody else's
sync — which left the two-instance arrangement this design is for unreachable
except through a script.

The client is one type over `reqwest`, and the directions sit on top of it in
`commands/sync/remote.rs`:

- **Pull**: fetch a bundle from the peer, stage it as a throwaway SQLite
  source, and hand it to the same merge engine a local `sync pull` runs. The
  write is local, so the preview is too.
- **Push**: build a bundle from the local database, send it for review, and
  apply the preview ID the peer returns. Nothing is written there until that
  ID is named, and a peer whose catalog moved under the preview refuses.

Two surfaces drive it. `psf-guard sync remote` for a terminal or a cron job,
and `POST /api/databases/{id}/sync/remote` for the Settings panel, which reads
its peers from `/api/peers`. Peers live in the registry beside the databases:
name, base URL, key, optional catalog. Unlike an incoming key, which is stored
as a digest because the server only needs to check it, an outgoing key must be
presented on every request and is therefore kept in the clear — the registry
file is a credential store, and the API never returns a key to a browser.

### Granting access without a Settings panel

The desktop app has Settings; a server run from the command line does not, and
the route Settings uses is gated behind `--allow-database-management` — far too
large a grant to demand for this, since it also lets a network caller name
server filesystem paths. A headless deployment therefore takes both grants from
`[[remote_sync]]` and `[[remote_upload]]` blocks in its `--config` file, each
naming a registry slug and a key (inline, or `token_file` for systemd
credentials and Docker secrets).

They apply to the in-memory database list at startup and are never written back
to the registry, so the config file stays the whole account of what a
deployment allows and rotating a key is a restart. A database has one key,
whichever grants it holds; two blocks naming it with different keys is refused
rather than letting the later one silently disable the earlier. So is a block
naming a database that is not registered — the alternative is a server that
answers every remote request with 403 and cannot say why.

### Keeping a preview across a refusal

Apply claims a preview once, so no two callers can apply the same one. But an
apply that refuses — the destination moved under the preview — or that breaks
on a locked file has written nothing, and its uploaded source data is still
good. The receiver therefore puts the preview back rather than dropping it.

The client's way forward is `POST .../refresh`: it re-runs the same stored
source against the destination as it now stands, keeps the preview ID, and
returns a fresh summary to review. Only a successful apply consumes the
preview. Without this, a stale destination would cost a remote client a
re-upload of the whole bundle.

### Audit log

Every remote action lands in `remote-sync-audit.jsonl` under the cache root,
one JSON object per line: when, which catalog, which action, which operation,
the outcome, and the row counts. Refusals are recorded too, including ones
that never named a catalog, because a run of rejected applies is what a stolen
token looks like from the server side. Entries carry no token, no filesystem
path, and no row contents. The file rolls at 8 MiB, keeping one previous
generation.

## N.I.N.A. plugin

A N.I.N.A. plugin can implement the remote protocol at the telescope without
exposing the SQLite file.

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
client can post a light or calibration frame directly to one opted-in PSF
Guard database:

```http
POST /api/db/{db_id}/images/upload
Authorization: Bearer <per-database-remote-api-key>
X-PSF-Guard-Database-ID: <db_id>
X-Content-SHA256: <64 lowercase hexadecimal characters>
Content-Type: multipart/form-data

image=@capture.fits
```

The database settings select one of that database's registered image roots as
the receive directory. The server requires the URL slug and echoed database ID
to agree, authenticates with the selected database's salted token hash, streams
at most 512 MiB to a sibling temporary file, verifies SHA-256 and the frame
headers, and publishes without overwriting an existing basename. The filename
must carry a frame extension: `.fits`, `.fit`, `.fts`, or `.xisf`.

For a light, the normal one-frame importer resolves an existing target by
object name or coordinates and reuses its exposure plan. If no target matches,
it builds the project, target, template, and plan from FITS headers. A bias,
dark, dark-flat, or flat instead enters PSF Guard's calibration library and
never creates a Target Scheduler `acquiredimage` row. This path therefore works
with an existing Target Scheduler catalog and with a fresh PSF Guard catalog
whose user never installed Target Scheduler.

Identical retries are idempotent. The response returns the resolved database,
frame kind, and either the project, target, and image IDs for a light or the
calibration frame and rig UUIDs. If scheduler sync registered a unique light
row first, the upload attaches the file to that row without importing a
duplicate. An ambiguous registered basename or an existing receive file with
different content returns `409 Conflict`.

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

- [x] Add scoped per-database credentials and capability discovery.
- [x] Add JSON export bundles and remote preview/apply.
- Test interrupted transfers, limits, stale previews, and retries.

### Phase 5: N.I.N.A. plugin

- [x] Implement the same capability, export, preview, apply, and job contract.
- [x] Add durable capture bundle notifications and direct FITS upload.
- Add image-history manifests.

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
