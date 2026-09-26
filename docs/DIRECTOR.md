# Director metadata API

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
processes before opening an upgraded store. HTTP backup, restore, path switching,
and catalog adoption are deliberately not exposed.

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
pairing, scoped check-in credentials, assignment issuance and the UI remain
separate work. Do not put an operator API token in a plugin profile as a substitute
for pairing.

## Contention and recovery

Storage runs off the asynchronous HTTP worker. Only one operation is admitted
at a time; contention returns `503` with `Retry-After: 1`. A canceled HTTP
request may still commit its already admitted transaction. Use the same create
identity on retry, or GET after an ambiguous rename result. Errors do not return
filesystem paths or raw SQLite diagnostics; detailed failures are logged locally.

Project objectives, site/rig enrollment, scoped assignments, feedback, and the
project UI remain in the [phased Director plan](design/director.md).
