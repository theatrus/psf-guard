# Director management

Director is experimental. This API manages global project, site and rig identities in a
separate meta database. It does not yet pair rigs, allocate work, or enable
acquisition. The NINA runtime preview and PSF Guard Sync remain separate.

## Where the store lives

Director is on whenever the server may manage its databases. The meta store is
a separate SQLite file beside the database registry: `director-meta.sqlite`
next to `config.json`, or `<registry>.director-meta.sqlite` for a registry
with another name, so `--registry /tmp/psf-guard-test.json` gets its own
store. The desktop app uses the same default. A server without
`--allow-database-management` leaves Director off, because activating a plan
writes into rig databases.

```text
psf-guard server --host 127.0.0.1 --registry test-registry.json --allow-database-management
psf-guard server --host 127.0.0.1 --registry test-registry.json --allow-database-management --director-meta /srv/psf-guard/director-meta.sqlite
```

`--director-meta` keeps the store at another path. The parent directory must
exist. A missing file is created atomically; an existing file must be a
recognized Director meta store. Catalogs, empty files, and unrelated SQLite
databases are not adopted. The normal server startup policy still requires
accounts for database management on a network bind unless the operator
explicitly trusts anonymous access. Prefer accounts; see
[authentication](AUTHENTICATION.md).

Back up the meta store with its SQLite-aware storage API before an upgrade.
Do not copy a live SQLite main file without its WAL. Stop older coordinator
processes before opening an upgraded store. HTTP backup, restore and path
switching are not exposed. Catalog adoption requires the explicit workflow below.

## Management screen

Overview remains the project and collected-data workspace. Its existing
**Plan & coordinates** dialog opens **Rig planning**, reusing the same target,
coordinate and exposure-plan editor. Returning to Overview preserves the source
database, project and database filter. This edits existing TS-compatible project
metadata; it does not yet generate downstream projects or executable intent.

The **Director** entry has **Projects**, **Sites** and **Rigs**. Projects and
sites retain the prototype identity editor. **Rigs** lists the existing database
registry, not a second inventory to create manually. **Configure** opens that
database's settings. Profiles within a database are setup provenance, not
separate rigs. Shared project progress and the framing wizard remain planned.

Names do not establish identity. Each row includes its stable UUID and current
revision. Create retries retain the same UUID. A conflicting rename keeps the
draft without overwriting the other editor's change: cancel, refresh, then edit
the current record. Listings load in bounded pages with a refresh action.

This first management screen does not edit observing objectives, configuration
snapshots, assignments, or plugin credentials. Acquisition remains unavailable.

### Enable database planning

Open the database's **Project planning links** in Settings. Choose **Preview rig
setup**, then **Enable planning**. This binds the database's durable lineage to
one rig, including an empty database. A single unambiguous prototype rig is
retained so existing setup and intent references survive. Conflicting prototype
associations are reported without rewriting them; their reassignment workflow
is not implemented yet. Merely listing databases does not write an identity.

Select source projects, then choose or create their shared project. Every source
profile uses this database's rig. Projects in different databases can contribute
to the same shared project. Names never establish ownership or merge projects.

Choose **Preview mappings**, check the source, global project and rig names and
UUIDs, then **Apply mappings**. Changed source evidence or destination revisions
discard the stale review; preview again. An interrupted Apply retains the exact
reviewed request for retry. Creating an identity is a separate operation and
does not map it until Apply succeeds.

Legacy `directorView=catalogs&directorCatalog=<slug>` links open the selected
database's settings; there is no separate Director Catalogs tab. Existing
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
| GET | `/catalogs/{slug}/mappings` | Optional `after` source-project UUID and `limit` 1-256; returns `catalog_identity`, optional bound `rig`, `items` and `next_after`. |
| GET | `/projects` | Optional `after` UUID cursor and `limit` from 1 to 256 (default 64). |
| POST | `/projects` | `{"id":"<caller-generated UUID>","name":"M31"}` |
| GET | `/projects/{id}` | Exact project UUID. |
| PATCH | `/projects/{id}` | `{"expected_revision":1,"name":"Andromeda"}` |

The same identity operations and body/query shapes are available at `/sites`
and `/rigs`. The latter remains a compatibility API for prototype references;
normal clients should enable planning on a registered database instead. Identity
names never establish a binding. No route deletes an identity or grants a rig
credentials.

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

First bind the database to its rig using these operator-only routes:

| Method | Route | Body |
| --- | --- | --- |
| POST | `/catalogs/{slug}/rig/preview` | `{"catalog_id":"<durable or proposed UUID>"}` |
| POST | `/catalogs/{slug}/rig/apply` | `{"plan":<same plan>,"preview_digest":"<preview result>"}` |

The report contains `binding` (`catalog` and `rig`), `preview_digest`, and
`applied`. Schema 5 adds a one-to-one catalog/rig binding without automatically
rewriting schema-4 mappings. Multiple legacy rigs in one database, or one legacy
rig shared by independent databases, return `409` for explicit resolution.
Unambiguous adoption preserves old rig IDs. New rigs use the catalog UUID.
Once bound, project mappings must use that rig; another catalog cannot reuse it.
Copies retaining the same lineage retain the binding rather than creating rigs.
Preview/apply uses fresh evidence, a read-only source preview, bounded admission,
and the same identity-first retry protocol as project mappings below. The
database name/locator and existing identity revisions are part of the review.

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
existing project UUIDs and the database's bound rig UUID. Names and row numbers cannot establish a
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

Database settings invoke these routes after explicit review. The mapping
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
not friendly rig names. A database can contain several profiles; they share its
database-backed rig. Confirm mappings in the adoption workflow above rather
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
