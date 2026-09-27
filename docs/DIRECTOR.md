# Director management

Director is experimental. This API manages global project, site and rig identities in a
separate meta database. It does not yet pair rigs, allocate work, or enable
acquisition. The NINA runtime preview and PSF Guard Sync remain separate.

## Enable on a test server

```text
psf-guard server --host 127.0.0.1 --registry test-registry.json --allow-database-management --director-meta director-meta.sqlite
```

The parent directory must exist. A missing file is created atomically; an
existing file must be a recognized Director meta store. Catalogs, empty files,
and unrelated SQLite databases are not adopted. Omitting `--director-meta`
leaves Director off and does not create a store. Tauri does not enable this API
yet. The normal server startup policy still requires accounts for database
management on a network bind unless the operator explicitly trusts anonymous
access. Prefer accounts; see [authentication](AUTHENTICATION.md).

Back up the meta store with its SQLite-aware storage API before an upgrade.
Do not copy a live SQLite main file without its WAL. Stop older coordinator
processes before opening an upgraded store. HTTP backup, restore and path
switching are not exposed. Catalog adoption requires the explicit workflow below.

## Management screen

When enabled, the **Director** navigation entry opens global **Projects**,
**Sites**, **Rigs**, and **Catalogs**. Editors can create and rename identities; readers
can inspect them. The selected view stays in the URL. Catalog selection does
not scope Director identities, and a catalog is not required to use this page.

Names do not establish identity. Each row includes its stable UUID and current
revision. Create retries retain the same UUID. A conflicting rename keeps the
draft without overwriting the other editor's change: cancel, refresh, then edit
the current record. Listings load in bounded pages with a refresh action.

This first management screen does not edit observing objectives, configuration
snapshots, assignments, or plugin credentials. Acquisition remains unavailable.

### Map a catalog

Open **Catalogs** and choose a configured catalog. Select source projects, then
choose or create a global project and a rig for each. Projects sharing a source
profile share one rig choice. Multiple source projects and rigs can contribute
to the same global project. Nothing infers rig identity from the database name.

Choose **Preview mappings**, check the source, global project and rig names and
UUIDs, then **Apply mappings**. Changed source evidence or destination revisions
discard the stale review; preview again. An interrupted Apply retains the exact
reviewed request for retry. Creating an identity is a separate operation and
does not map it until Apply succeeds.

The selected catalog stays in the `directorCatalog` URL parameter. Existing
links are read-only; a changed source profile is flagged instead of silently
reassigned. Missing, invalid or duplicate source identities cannot be selected.
Readers can inspect saved links but cannot create identities or apply mappings.
Refresh reloads source evidence and saved links. The view pages source rows and
loads at most 4096 records per inventory; each Apply accepts at most 256 projects.

## Protocol 1

Routes below start with `/api/director/v1`. Responses use the normal
`{success, data, error, status}` envelope. The API requires normal browser
authentication or a user API token when accounts are configured. The existing
trusted-loopback and explicit anonymous-access policies still apply when no
accounts exist. A database Sync key or pairing code does
not grant access. Read-only users may inspect identities but cannot mutate them.
All metadata routes also require the database-management gate.

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/status` | Reports `protocol_version`, `enabled`, `instance_id`, and `acquisition_available: false`. |
| GET | `/catalogs/{slug}/discovery` | Read project/profile evidence from one already registered catalog. |
| GET | `/catalogs/{slug}/mappings` | Optional `after` source-project UUID and `limit` 1-256; returns `catalog_identity`, `items` and `next_after`. |
| GET | `/projects` | Optional `after` UUID cursor and `limit` from 1 to 256 (default 64). |
| POST | `/projects` | `{"id":"<caller-generated UUID>","name":"M31"}` |
| GET | `/projects/{id}` | Exact project UUID. |
| PATCH | `/projects/{id}` | `{"expected_revision":1,"name":"Andromeda"}` |

The same identity operations and body/query shapes are available at `/sites`
and `/rigs`. Identities in those namespaces remain independent, even if their
names or UUIDs match. No route deletes an identity or grants a rig credentials.

A global project identity is not a Target Scheduler project or a catalog-local
integer ID. Same-name projects stay distinct. Keep the caller-generated UUID
across create retries. Retrying the same ID and name is idempotent; changed
content under an existing ID returns `409`. Renames require the current revision
and increment it when the name changes. Reload after a revision conflict.

Listings return `items` and `next_after`, ordered by stable UUID rather than
name. Each page is a fresh read, not a long-running snapshot; restart a full
listing to discover concurrent inserts before its cursor. Request bodies are
limited to 4 KiB for identity operations, and unknown fields are rejected.
There is no deletion endpoint.

## Catalog adoption

These operator-only POST routes preview and apply explicit catalog mappings.
They require database management and normal write access, not a Sync key. The
catalog slug must already exist in the registry; no client filesystem path is
accepted. Preview opens the catalog read-only. It uses a short rollback-only
metadata transaction to check the same constraints as Apply.

| Method | Route | Body |
| --- | --- | --- |
| POST | `/catalogs/{slug}/adoption/preview` | A plan containing `catalog_id` and `mappings`. |
| POST | `/catalogs/{slug}/adoption/apply` | `{"plan":<same plan>,"preview_digest":"<preview result>"}` |

Each mapping names `catalog_id`, `source_project_guid`, `source_profile_id`,
`project_id` and `rig_id`. Use exact source GUIDs/profile IDs from discovery and
existing Director project/rig UUIDs. Names and row numbers cannot establish a
mapping. All entries must use the plan's catalog UUID. Plans contain 1-256
distinct source projects and fit within 256 KiB. Missing/invalid/duplicate source
identities must be corrected before adoption; the API does not invent them.

Discovery returns `catalog_identity` when the catalog is already adopted. Keep
that UUID. For an unadopted catalog, generate a new catalog UUID and retain it
across preview, apply and retries. An unadopted file cannot claim a catalog UUID
already registered in the coordinator.

Preview returns the effective catalog identity, source and destination names,
destination revisions, mappings, `preview_digest`, and `applied: false`. Apply
revalidates the source in its write transaction and requires the exact preview
digest. Changed evidence, choices, destination names/revisions or registered
locator require a new preview. Success returns `applied: true`; identical retries
remain idempotent. The digest checks stale input; it is not authorization.

Apply adds only a PSF Guard-owned identity table to the catalog. It does not
change TS project GUIDs, image grades or history, import frames, infer historical
rig ownership, or authorize acquisition. Catalog registration and all mappings
commit together in the meta store. Catalog identity commits first while holding
the coordinator writer; if the final meta commit fails, retry the same plan and
digest to finish registration. Do not mint a replacement identity. A changed
preview still requires review before retrying.

The Catalogs view invokes these routes after explicit review. Its mapping
inventory endpoint is read-only, allows readers, and requires database
management. An unadopted catalog returns a null identity and no mappings without
creating anything. Reading the identity does not scan images or project history.

Catalog copies retain lineage; the same
catalog/project mapping is not duplicated for another path. Independent forks,
historical frame attribution and contribution accounting remain separate work.

## Configuration snapshots

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/sites/{site}/snapshots` | Optional `after` UUID and `limit` 1-256; returns `ids` and `next_after`. |
| POST | `/sites/{site}/snapshots` | Complete `SiteSnapshot`: `id`, `site_id`, `location`, `horizon`. |
| GET | `/sites/{site}/snapshots/{snapshot}` | Exact immutable snapshot owned by the named site. |
| GET | `/rigs/{rig}/setups` | Optional `after` UUID and `limit` 1-256; returns `ids` and `next_after`. |
| POST | `/rigs/{rig}/setups` | Complete `RigSetup`: `id`, `configuration`, `site_snapshot_id`, `minimum_altitude_degrees`, `maximum_altitude_degrees`, `meridian_exclusion`. |
| GET | `/rigs/{rig}/setups/{setup}` | Exact immutable setup owned by the named rig. |

Snapshot bodies allow 256 KiB, matching the shared-core bound, so native horizon
breakpoints are not thinned to fit an identity form. Send canonical horizon
content, never a local file path. The site/rig must already exist, and a setup
must reference a registered site snapshot. A POST whose owner differs from the
URL returns `400`; fetching an ID through another owner returns `404`.

The coordinator setup UUID is distinct from `configuration.id`, the native
equipment fingerprint. Keep both unchanged on retry. Identical retries return
the stored content; changed content under the same snapshot/setup ID returns
`409`. Create a new ID to change location, horizon, equipment or limits. These
routes do not select a latest/active setup, replace an in-flight assignment, or
make a registered rig eligible to acquire. See the typed
[configuration model](../crates/director-meta/src/configuration.rs).

These are operator APIs using ordinary user authentication. Dedicated rig
pairing, scoped check-in credentials, assignment issuance and snapshot editing remain
separate work. Do not put an operator API token in a plugin profile as a substitute
for pairing.

## Contention and recovery

Storage runs off the asynchronous HTTP worker. Only one operation is admitted
at a time; contention returns `503` with `Retry-After: 1`. A canceled HTTP
request may still commit its already admitted transaction. Use the same create
identity on retry, or GET after an ambiguous rename result. Errors do not return
filesystem paths or raw SQLite diagnostics; detailed failures are logged locally.

## Catalog discovery

`GET /api/director/v1/catalogs/{slug}/discovery` returns the registered catalog's
slug and display name, a `snapshot_digest`, and `evidence` containing projects,
profile IDs with project counts, and schema capabilities. It includes projects
with no images. A project reports its source row ID, parsed non-nil project GUID,
profile ID, name and `issues`. Missing GUID/profile columns in older TS schemas
are reported rather than treated as empty catalogs. Invalid fields and duplicate
project GUIDs are flagged; equivalent UUID spellings count as duplicates.

This is discovery, not adoption. It does not create rigs, link projects, infer
equipment or horizons, or change source tables. Profile IDs are source evidence,
not friendly rig names. A database can contain several profiles and one rig can
have several catalogs. Confirm mappings in the adoption workflow above rather
than treating a slug, source row ID, name or snapshot digest as global identity.
The digest detects changes to the returned evidence; it grants no write or
execution authority and is not an image/catalog-content checksum.

Discovery uses its own read-only SQLite connection and a short snapshot of the
project table; it reads no acquired-image records, thumbnails or image files.
One discovery read is admitted at a time, independently of metadata operations.
Contention returns retryable `503`; unsupported schemas or more than 4096
projects return `422` without a partial result. Text fields are limited to 512
UTF-8 bytes; malformed fields are flagged rather than used for mapping. The
normal operator authentication and database-management gate apply. This route
accepts only a registered slug, never an arbitrary file path.

Project objectives, site/rig enrollment, scoped assignments, feedback, and the
objective editor remain in the [phased Director plan](design/director.md).
