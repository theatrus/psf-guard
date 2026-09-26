# Director metadata API

Director is experimental. This API manages global project identities in a
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
All project routes also require the database-management gate.

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/status` | Reports `protocol_version`, `enabled`, `instance_id`, and `acquisition_available: false`. |
| GET | `/projects` | Optional `after` UUID cursor and `limit` from 1 to 256 (default 64). |
| POST | `/projects` | `{"id":"<caller-generated UUID>","name":"M31"}` |
| GET | `/projects/{id}` | Exact project UUID. |
| PATCH | `/projects/{id}` | `{"expected_revision":1,"name":"Andromeda"}` |

A global project identity is not a Target Scheduler project or a catalog-local
integer ID. Same-name projects stay distinct. Keep the caller-generated UUID
across create retries. Retrying the same ID and name is idempotent; changed
content under an existing ID returns `409`. Renames require the current revision
and increment it when the name changes. Reload after a revision conflict.

Listings return `items` and `next_after`, ordered by stable UUID rather than
name. Each page is a fresh read, not a long-running snapshot; restart a full
listing to discover concurrent inserts before its cursor. Request bodies are
limited to 4 KiB, and unknown fields are rejected. There is no deletion endpoint.

Storage runs off the asynchronous HTTP worker. Only one operation is admitted
at a time; contention returns `503` with `Retry-After: 1`. A canceled HTTP
request may still commit its already admitted transaction. Use the same create
identity on retry, or GET after an ambiguous rename result. Errors do not return
filesystem paths or raw SQLite diagnostics; detailed failures are logged locally.

Project objectives, site/rig enrollment, scoped assignments, feedback, and the
project UI remain in the [phased Director plan](design/director.md).
