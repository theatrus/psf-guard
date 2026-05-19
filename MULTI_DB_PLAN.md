# Multi-Database Support — Design & Implementation Plan

Status: **Draft / not started**
Owner: TBD
Last updated: 2026-05-18

## 1. Goal

Allow the **server** to operate on multiple N.I.N.A. scheduler SQLite databases simultaneously in a single session — regardless of whether it's launched from the Tauri app or via `psf-guard server`. Each database has its own set of image base directories. Users can navigate between databases; every read and write is scoped to the database the user selected.

One-off CLI subcommands (`analyze`, batch operations, etc.) remain single-DB — they take a DB path on the command line and do their work without going through the server. Multi-DB only applies to the long-running interactive server surface.

### Concrete user-visible outcomes
- Settings panel lists N configured databases. Each has: a friendly name, a `.sqlite` path, and an ordered list of image directories.
- **No "active database" mode.** The Overview merges projects and targets across all configured databases into one list, visually grouped/sectioned by DB. Users see all their telescope work at once, just labeled with which DB each item lives in.
- Scoped views (grid, detail, comparison) read `db` from the URL alongside `project`/`target`/`image`. There is no fallback — every scoped link is fully qualified.
- Cache refresh is per-DB and shows per-DB progress.
- Grading writes go to the DB the image came from. The frontend always has the `db_id` in scope (from URL or the project's row), so writes are unambiguous.
- URL state encodes the DB so links/bookmarks/back-button work.

### Why merge instead of an active-DB switcher
Project IDs in N.I.N.A. databases are just `i32` and trivially collide across files. A merged-list UI is **safer** (every navigation carries an explicit `db_id`, no ambiguity) AND **better UX** (no mode-switching cognitive load — users just see their projects). The cost is a small fan-out of API requests on the overview pages, which is fine since results cache.

### Non-goals (v1)
- Cross-DB joins, merged project overview, "all projects from all DBs" aggregations.
- Migrating projects/images between DBs.
- Editing DB schemas. Each DB is treated as read/grade-write only, as today.
- Multi-DB support for one-off CLI subcommands (`analyze`, batch grading, etc.) — those stay single-DB and operate without the server.

---

## 2. Current state (single-DB, summary)

Detailed mapping from exploration:

- **Startup**: `src/cli_main.rs:217-305` and `src/tauri_main.rs:41-110` build a single `AppState` from one DB path + N image dirs.
- **State**: `src/server/state.rs:23-39` — `AppState` holds `database_path: String`, `image_dirs: Vec<String>`, a single `Arc<Mutex<Connection>>`, a single `FileCheckCache`, a single `DirectoryTree`, a single refresh mutex, and a single `RefreshProgress`.
- **DB layer**: `src/db.rs:43-52` — `Database<'a>` wraps a borrowed `&Connection`. No pool; one Mutex.
- **Cache**: `FileCheckCache` keys on `project_id: i32` and `target_id: i32` only — these IDs are DB-local, so two DBs with overlapping IDs would collide.
- **API**: `src/server/mod.rs:154-194` — routes like `/api/projects`, `/api/images?project_id=X`, `/api/images/{id}/grade` have no DB qualifier.
- **Frontend**: `static/src/hooks/useUrlState.ts:70-93` — URL state is `?project=5&target=3`. No DB dimension.
- **Tauri config**: `src/tauri_main.rs:8-21` — `TauriConfig { database_path, image_directories }`. Single DB.
- **Project ID scope**: DB-local `i32`. **Will collide across DBs.**
- **Filename uniqueness**: assumed unique within image dirs of one DB. With per-DB trees this stays a per-DB assumption.

---

## 3. Design choices (with rationale)

### 3.1 Database identity: user-editable slug, seeded from a path hash
Each configured DB has a `slug` field — a short, URL-safe string that is the canonical id everywhere (API paths, cache dirs, URL state). Properties:

- **Auto-derived on add**: when a DB is first added, the slug defaults to `db-<8-hex-chars>` where the hex is the first 32 bits of `sha256(canonical(db_path))`. Example: `/Users/me/scheduler.sqlite` → `db-a3f4b2c1`. Deterministic, so re-adding the same path yields the same default — no spurious cache invalidation.
- **User-editable**: settings panel lets the user rename `db-a3f4b2c1` to `imaging-rig` or anything matching `^[a-z0-9][a-z0-9-]{0,63}$`. Display name is a separate field that can be anything (`"Imaging Rig"`).
- **Decoupled from `db_path` after creation**: once the slug is in `config.json`, moving the `.sqlite` file does not change it. (The hash is only the *default seed*, not a live derivation.)
- **Unique within config**: collisions on add get suffixed `-2`, `-3`, …
- **On user rename**: cache directory `<cache_root>/<old-slug>/` is renamed to `<new-slug>/` so cached previews survive. URL state from the old slug stops resolving — settings UI shows a clear "existing links to this DB will break" warning before the rename commits.

Used as:
- Cache subdirectory: `~/Library/Caches/psf-guard/<slug>/...`
- API path component: `/api/db/<slug>/...`
- URL param on frontend: `?db=<slug>&project=...`

**Why this over a UUID?**
- `?db=imaging-rig&project=5` is shareable and debuggable in a way `?db=9a2b1c4d-...` isn't.
- Server logs become readable: `"refreshing cache for db=imaging-rig"`.
- Hand-editing `config.json` stays sane.
- Same URL-stability properties as a UUID — the slug is persisted, not re-derived.

**Why seed from a path hash instead of a random/incremental id?**
- Deterministic default means re-adding the same DB after an accidental removal lands on the same slug, which avoids orphaning cache and URLs.
- Avoids needing to track an "id counter" or generate randomness.

Internally we still call this `db_id` in code/routes (it *is* an id, just human-friendly). The `slug` term is for user-facing copy.

### 3.2 API shape: **path-nested**, not query-param
All per-DB endpoints move under `/api/db/{db_id}/...`. Global endpoints (`/api/info`, `/api/databases`) stay at top level.

**Why path nesting over `?db=`?**
- Forces every handler signature to take a `db_id`; can't accidentally forget it and silently hit "the default DB."
- Makes server-side routing concerns (load `DatabaseContext`, gate by validity) trivial via a route layer / `FromRequestParts` extractor.
- SSE progress streams become per-DB URLs, which is clean.
- One-time refactor cost is bounded (~15 handlers).

Trade-off accepted: every per-DB frontend URL must be built with a `db_id` prefix. This is what we want — explicit.

### 3.3 State refactor: `AppState` → `AppState` of `Arc<DatabaseContext>`
Split today's `AppState` into:

- **`AppState`** (global, one per server process):
  - `databases: RwLock<HashMap<DbId, Arc<DatabaseContext>>>` — mutable so DBs can be added/removed at runtime without restart.
  - `cache_dir_root: PathBuf` (the parent; per-DB sub-dirs live under this)
  - `pregeneration_config: PregenerationConfig` (shared)
  - Anything else process-global.

- **`DatabaseContext`** (one per configured DB, all today's per-DB state):
  - `id: DbId`
  - `name: String`
  - `database_path: String`
  - `image_dirs: Vec<String>` + parsed `Vec<PathBuf>`
  - `cache_dir: PathBuf` (under `cache_dir_root/<id>/`)
  - `db_connection: Arc<Mutex<Connection>>`
  - `file_check_cache: Arc<RwLock<FileCheckCache>>`
  - `directory_tree_cache: Arc<RwLock<Option<DirectoryTree>>>`
  - `refresh_mutex: Arc<TokioMutex<()>>`
  - The cache progress lives **inside** `FileCheckCache` already, so it travels with the context.

**`DbId`** = newtype around `String` (the UUID). Hashable, cloneable. Reject IDs that aren't well-formed UUIDs at the boundary.

### 3.4 Live config: hot-reload vs restart
Adding/removing a DB should work **without an app restart** when feasible. Concretely:
- Add DB: persist to `config.json`, open the connection, insert a new `DatabaseContext` into `AppState.databases`, kick off background cache refresh. No restart.
- Remove DB: drop the context (closes the connection, frees caches), persist config. No restart.
- Edit DB (change path or image dirs): replace the context atomically. No restart.

This is reachable because there's no global "default DB" baked into startup — handlers always look up by `db_id`.

### 3.5 Cache namespacing
The on-disk image preview cache (`~/Library/Caches/psf-guard/`) gets a per-DB subdir: `<cache_root>/<slug>/<image_id>/<size>.jpg` etc. Prevents collision when two DBs assign the same `image_id` to different files. On slug rename: directory is moved (`rename(2)`) so caches survive.

### 3.6 Server launch surfaces (CLI vs Tauri)
There is **one server**. It is always multi-DB-capable at the API level. CLI and Tauri are two ways to *launch* it; they differ only in how the initial DB list is assembled.

**Single source of truth: the config file.**
`~/Library/Application Support/psf-guard/config.json` (and platform equivalents). Both launch paths read from and write to it. The CRUD endpoints (`POST /api/databases`, etc.) always work and always persist.

**Tauri launch:**
- Load config, build one `DatabaseContext` per entry, start server. Same as before.

**CLI launch (`psf-guard server [db] [dirs...]`):**
- **No positional args**: load config, start server. Identical to Tauri startup. If the config is empty, the server runs but the UI shows an empty-state screen prompting the user to add a database via settings.
- **With positional args**: derive the slug from the DB path (`db-<8-hex>`), check if an entry with that slug (or that canonical path) already exists in the config. If not, **add it** to the config and persist. Then start the server with the full configured set loaded. This makes `psf-guard server new.sqlite imgs/` mean "register this DB if I haven't already, then start" — copy-pasted docs commands still work, but they extend rather than replace the user's setup.
- **`--config <path>` flag** (new): override the config file location. Useful for dev/test isolation so a CLI server invocation doesn't pollute the user's real config. When set, both reads and writes target this file.

**Slug derivation** is shared by the Tauri add-DB flow and the CLI "ensure registered" path via a single helper:
```
compute_default_slug(db_path) = "db-" + hex(sha256(canonicalize(db_path))[..4])
```
Deterministic, so the same `.sqlite` file always seeds the same default slug. Bookmarks and on-disk caches carry across sessions and across launch surfaces.

**No `AppState.mode` field.** There is no behavioral fork between "CLI mode" and "Tauri mode" inside the server. The only difference is config-file seeding at startup, which happens before `AppState` is constructed.

**One-off CLI subcommands** (`analyze`, batch grade, etc.) remain single-DB. They take a positional DB path and operate on that file directly. They don't touch the config and don't go through `AppState`. Out of scope for this plan.

**Implementation surface:**
- `src/cli_main.rs` server command: replace single-DB construction with "load config (merging CLI positional args if any), build a `DatabaseContext` per entry."
- New `--config <path>` flag on the server subcommand.
- New `compute_default_slug` helper, shared.
- Config load/save code refactored into a shared module used by both `tauri_main.rs` and `cli_main.rs` (today it lives only in `tauri_main.rs:335-360`).

The web UI talks to `/api/db/<that-id>/...` exactly like multi-DB mode. The UI just renders one DB in the switcher. No special "single-DB code path" on the server.

For the Tauri/UI side under CLI mode: the frontend calls `GET /api/databases` and discovers the one available DB.

---

## 4. Data model

### 4.1 New config schema (v2)

```jsonc
{
  "schema_version": 2,
  "databases": [
    {
      "id": "imaging-rig",                        // slug, user-editable, URL-safe
      "name": "Imaging Rig",                      // display name, can be anything
      "db_path": "/Users/me/.../schedulerdb.sqlite",
      "image_dirs": ["/Volumes/Astro/Images"]
    },
    {
      "id": "db-a3f4b2c1",                        // default slug from path hash; never renamed
      "name": "Remote Scope",
      "db_path": "/Users/me/.../remote.sqlite",
      "image_dirs": ["/Volumes/Remote/Images"]
    }
  ],
  "active_db_id": "imaging-rig"                   // last-selected; UI starts here
}
```

### 4.2 Migration from v1 (`{database_path, image_directories}`)
On load, if the file has no `schema_version`:
1. Derive a slug from the legacy `database_path` using the path-hash default (`db-<8-hex>`).
2. Wrap the legacy fields into a single `databases[0]` entry with `name` derived from the file stem (e.g. `schedulerdb.sqlite` → `"Schedulerdb"`).
3. Set `schema_version: 2` and `active_db_id` to that slug.
4. Atomically write back (write to temp + rename) so a crash mid-migration doesn't leave a half-baked file.
5. Keep a `.bak` copy of the v1 file on first migration.

Single migration step only — no need to support multiple historical versions yet.

### 4.3 Slug stability rules
- The slug is **persisted** in `config.json`. It is the canonical id from that point on.
- Initial value when adding a DB: `db-<8-hex>` from the path. The user is shown this in the add-DB dialog and can override it on the spot.
- Editing `db_path` or the display `name` does **not** change the slug.
- Editing the slug is allowed but disruptive: existing URL state breaks. The cache directory is renamed alongside, and the settings UI shows a confirmation. Recommend treating slug rename as rare.
- If the user removes a DB and later re-adds the same path: default slug matches what it was before. If the user previously customized it (e.g. `imaging-rig`), they'd need to re-customize — that's acceptable since they explicitly removed it.
- On collision when adding (slug already exists in config): suffix `-2`, `-3`, … and report the final value to the UI so the user can override.

---

## 5. API surface

### Global (no DB scope)
- `GET /api/info` — server info (unchanged shape, but no `database_path` field anymore).
- `GET /api/databases` — list of `{id, name, db_path, image_dirs, status}`. `status` is `"ok" | "unreachable" | "loading"`.
- `POST /api/databases` — add a new DB. Body `{name, db_path, image_dirs}`. Server generates `id`, persists config, opens context.
- `PUT /api/databases/{db_id}` — edit name / db_path / image_dirs.
- `DELETE /api/databases/{db_id}` — remove.

### Per-DB (all under `/api/db/{db_id}`)
Existing routes move verbatim, just nested:
- `GET  /api/db/{db_id}/projects`
- `GET  /api/db/{db_id}/projects/overview`
- `GET  /api/db/{db_id}/targets/overview`
- `GET  /api/db/{db_id}/stats/overall`
- `GET  /api/db/{db_id}/projects/{project_id}/targets`
- `GET  /api/db/{db_id}/images`
- `GET  /api/db/{db_id}/images/{image_id}`
- `GET  /api/db/{db_id}/images/{image_id}/preview`
- `GET  /api/db/{db_id}/images/{image_id}/stars`
- `GET  /api/db/{db_id}/images/{image_id}/annotated`
- `GET  /api/db/{db_id}/images/{image_id}/psf`
- `PUT  /api/db/{db_id}/images/{image_id}/grade`
- `GET  /api/db/{db_id}/analysis/sequence`
- `GET  /api/db/{db_id}/analysis/image/{image_id}`
- `PUT  /api/db/{db_id}/refresh-cache`
- `PUT  /api/db/{db_id}/refresh-directory-cache`
- `GET  /api/db/{db_id}/cache-progress`

### Server-side helpers
- Custom Axum extractor `DbContext` that resolves `{db_id}` from path → `Arc<DatabaseContext>`. Returns 404 on unknown ID.
- Handlers stop taking `State<Arc<AppState>>` for per-DB work; they take `DbContext` instead (and `State<Arc<AppState>>` only when they truly need global state).

### Tauri commands (separate from HTTP API)
Replace single-DB commands:
- `pick_database_file()` — still picks a file path (unchanged).
- `pick_image_directory()` — unchanged.
- `save_configuration()` — now takes the v2 shape.
- `get_current_configuration()` — returns v2 shape.
- New: `add_database(name, db_path, image_dirs)`, `remove_database(db_id)`, `update_database(db_id, ...)`. These call the server's HTTP API (or shared library code) so behavior matches the web client exactly.
- `is_configuration_valid()` — true if **at least one** DB is configured and reachable.

---

## 6. Frontend changes

The frontend never has a notion of a "currently selected database." Lists are merged across DBs; scoped views read `db_id` from URL state. There is no DB switcher in the navigation.

### 6.1 URL state
The `db` param appears alongside `project`/`target`/`image` whenever a view is scoped to one database:
- `/grid?db=imaging-rig&project=5&target=3` — image grid for a specific project's images
- `/detail?db=imaging-rig&image=42` — single-image detail view
- `/overview` — **no `db` param**; renders the merged cross-DB overview

`useProjectTarget()` becomes `useDbProjectTarget()` returning `{ dbId, projectId, targetId, setDbProjectTarget }`. There is no setter for `db` alone — you always set the triple `(dbId, projectId, targetId)` atomically when navigating into a scoped view.

**Missing `?db=` on a scoped view is an error**, not something to fall back from. Show "Database not found, this link may be stale" and a button back to overview. This catches bugs early and protects against accidental cross-DB writes.

### 6.2 API client
`static/src/api/client.ts` gains a `dbId` parameter on every per-DB method and embeds it in the path:
```ts
getProjects(dbId)                     → GET /api/db/{dbId}/projects
getProjectsOverview(dbId)             → GET /api/db/{dbId}/projects/overview
getImages(dbId, { projectId, ... })   → GET /api/db/{dbId}/images?...
updateGrade(dbId, imageId, status)    → PUT /api/db/{dbId}/images/{imageId}/grade
```

React Query keys gain `dbId` as the first segment: `['db', dbId, 'projects']`. This keeps caches per-DB without manual invalidation, and makes per-DB cache eviction trivial.

Global methods (no DB scope): `getServerInfo()`, `getDatabases()`. Used by the merged-overview hooks below.

### 6.3 Merged-overview hooks
Cross-DB list views are built via hooks that fan out per-DB queries and concatenate the results, stamping each row with its source `db_id` and `db_name`.

- `useAllDatabases()` — returns the list of configured DBs (single React Query keyed on `['databases']`).
- `useMergedProjectsOverview()` — fetches `getProjectsOverview(db.id)` for every configured DB in parallel (via `useQueries`), returns a flat list of `(ProjectOverview & { db_id, db_name })`. Loading/error states are aggregated.
- `useMergedTargetsOverview()` — same pattern.
- `useMergedOverallStats()` — sums totals across the per-DB `OverallStats` results client-side.

Overview component renders sections grouped by `db_name`, with a collapsible header per section. Clicking a project navigates to `/grid?db={db_id}&project={project.id}`.

### 6.4 Settings panel
Today it's a single-DB form. Rebuild as a list with add/edit/remove rows. Each row:
- Editable display name
- Editable slug (with warning that changing it breaks existing links)
- DB path with file-picker button
- Image directory list with add/remove/reorder buttons
- "Refresh caches" button (per-DB)
- "Remove" button (with confirm)

"Add database" button at the bottom. This is the only place users think about "which DBs do I have" — the rest of the UI just merges them.

### 6.5 SSE / cache progress
Each `DatabaseContext` has its own progress. On the overview, an aggregated indicator shows "N of M databases refreshing." Each DB section header can expand to show its individual progress. Tapping a per-DB "Refresh caches" button kicks that DB's refresh; the SSE stream is per-DB.

---

## 7. Backend implementation phases

Each phase ends in a working build (`cargo build --features tauri && cargo test`). Each phase is mergeable to main on its own.

### Phase B1: Introduce `DatabaseContext` (no API change yet)
- Add `src/server/database_context.rs` with `DatabaseContext` (fields per §3.3). Move per-DB state off `AppState` into this struct.
- `AppState` keeps a `databases: RwLock<HashMap<String, Arc<DatabaseContext>>>` but at this phase always has exactly one entry. Existing handlers fetch that one entry.
- All cache/refresh code moves to methods on `DatabaseContext`. The current `AppState` ones become thin shims.
- **Tests**: cargo test passes; manual smoke test of single-DB mode unchanged.

### Phase B2: Path-nested API + extractor
- Add `DbContext` Axum extractor that reads `{db_id}` path param and returns `Arc<DatabaseContext>` or 404.
- Move all per-DB routes under `/api/db/{db_id}/...`. Rewrite handler signatures to use `DbContext`.
- Add new global endpoints: `GET /api/databases`, `GET /api/info` (updated to omit `database_path`).
- Frontend isn't touched yet — server has a temporary compatibility shim: `/api/projects` → 301 redirect or 410 Gone? **Recommend: no shim, frontend updated in same PR.** Compatibility shims rot.
- **Tests**: extractor unit tests; integration test that 404s on bad `db_id`.

### Phase B3: Multi-DB-aware config + Tauri commands
- Define v2 `TauriConfig`: `{ schema_version, databases: Vec<DatabaseEntry>, active_db_id }`.
- Write migration from v1.
- Tauri commands `add_database`, `remove_database`, `update_database`, updated `save_configuration` and `get_current_configuration`.
- At startup: load v2 config, build one `DatabaseContext` per entry, populate `AppState.databases`. Trigger background cache refresh per DB.
- `is_configuration_valid()` → ≥1 reachable DB.
- **Tests**: v1→v2 migration unit test on synthetic config files (round-trip).

### Phase B4: HTTP CRUD for databases
- `POST /api/databases`, `PUT /api/databases/{id}`, `DELETE /api/databases/{id}`.
- These persist to config AND mutate `AppState.databases` atomically.
- On add: validate file exists & opens; refuse if not.
- **Tests**: integration test that adds → uses → removes a DB in one process.

### Phase B5: Cache directory namespacing
- Anywhere we write to `cache_dir`, prefix with `<db_id>/`.
- One-time cleanup pass on startup: nothing to do for fresh installs; for upgraded installs, optionally migrate existing cached files into the legacy DB's namespace.

---

## 8. Frontend implementation phases

### Phase F1: API client + URL state + merged hooks
- Update `static/src/api/client.ts` so every per-DB method takes `dbId` and routes to `/api/db/{dbId}/...`. Add `getDatabases()` global.
- Update `useUrlState.ts`: add `useDbProjectTarget` returning `{ dbId, projectId, targetId, setDbProjectTarget }` for navigating into scoped views.
- Add `useAllDatabases()`, `useMergedProjectsOverview()`, `useMergedTargetsOverview()`, `useMergedOverallStats()`.
- Update every existing call site to pass `dbId` (sourced from URL for scoped views, from row metadata for list items in the merged hooks).
- React Query keys: `['db', dbId, ...]` for per-DB queries; `['databases']` for the global list.

### Phase F2: Merged Overview component
- Rewrite Overview to use the merged hooks.
- Group projects/targets by DB, each in a collapsible section with the DB's display name as the header.
- Project click → `/grid?db={db_id}&project={project.id}`.
- Target click → `/grid?db={db_id}&project={project_id}&target={target.id}`.
- Empty state when no DBs configured: "Add a database in Settings."

### Phase F3: Settings panel rewrite
- Multi-row DB list with add/remove/edit.
- Per-row: name, slug (with rename warning), DB path file picker, image dir list with add/remove/reorder, "Refresh caches" button.
- File pickers via existing Tauri commands.
- "Add database" button.

### Phase F4: Per-DB SSE progress + aggregated indicator
- Each DB's `/api/db/{db_id}/cache-progress` opens its own SSE.
- Aggregated indicator in nav showing "N of M databases refreshing." Expand to see per-DB status.

---

## 9. Test plan

### Backend
- Unit: config v1→v2 migration (golden files), `DbContext` extractor (valid/invalid/unknown id), per-DB cache isolation (two contexts don't share state).
- Integration: full add-DB → list-projects → grade-image → remove-DB cycle via HTTP.
- Property: two DBs with overlapping project_ids return distinct project listings.

### Frontend
- Component test for DB switcher (changes URL + invalidates queries).
- Unit test for `useDbProjectTarget`.
- E2E (manual for v1, automated later): open two DBs, grade an image in DB-A, verify DB-B's images are untouched.

### Manual smoke test (must pass before merge)
1. Fresh install, single DB → unchanged behavior.
2. Upgrade install from v1 config → DB shows up with old name, all data accessible.
3. Add second DB via settings → switcher shows both → switching works → caches refresh independently.
4. Grade image in DB-A, switch to DB-B, switch back → grade persisted in DB-A only.
5. Restart app → both DBs still loaded, last active DB selected.
6. Remove DB-A → switcher shows only DB-B → URL state from DB-A no longer resolves (graceful empty state, not crash).

---

## 10. Deferred / open questions

- **Cross-DB overview** ("show me all projects from all DBs in one list"): not in v1. Easy to add as a separate `/api/aggregate/projects` endpoint once the per-DB model is stable.
- **Concurrent grade writes** from two browser tabs targeting the same DB: same risk as today (last-write-wins). Not addressed here.
- **DB locking when N.I.N.A. has the file open**: today's behavior unchanged (we open read-only-ish via rusqlite's bundled SQLite). Document that opening a DB N.I.N.A. is actively writing to may yield stale reads.
- **Image filename collisions across DBs that share an image directory**: legal, each DB has its own directory tree. The same filename in the same dir resolves to the same physical file for both — fine. Different files with the same basename in different dirs would be ambiguous *within one DB* but that's a pre-existing concern.
- **CLI `--databases` flag for batch-registering multiple DBs at startup?**: deferred. The `psf-guard server <db> <dirs>` form handles the common case of "ensure this DB is registered, then start." Users wanting many at once can edit `config.json` directly or use the settings panel.

---

## 11. Task tracker

Update inline as work progresses. `[ ]` pending, `[~]` in progress, `[x]` done.

### Backend
- [x] **B1** Introduce `DatabaseContext`, refactor `AppState` to a map (single entry today)
- [x] **B2** Path-nested per-DB API + `DbContext` extractor; `/api/databases` list endpoint
- [x] **B3** v2 config + migration; updated Tauri commands; build N contexts at startup
- [x] **B4** HTTP CRUD for databases (add/edit/remove at runtime)
- [x] **B5** Cache directory namespacing under `<cache_root>/<db_id>/`

### Frontend
- [x] **F1** API client + URL state + merged-overview hooks (call sites updated to pass `dbId`)
- [x] **F2** Merged Overview view (cross-DB list grouped per-DB)
- [x] **F3** Settings panel rewritten for multi-DB
- [ ] **F4** Per-DB SSE cache progress + aggregated indicator

### Cross-cutting
- [ ] Manual smoke test pass against checklist in §9
- [ ] Update `CLAUDE.md` notes for multi-DB architecture
- [ ] Update `README.md` user-facing docs
- [ ] **Document new CLI persistence behavior** in both `README.md` and `CLAUDE.md`: `psf-guard server <db> <dirs>` now registers the DB into the shared config on first run. For ad-hoc/scratch sessions that should not pollute the user's real config, use `psf-guard server --config /tmp/scratch.json <db> <dirs>`. Include a short "Migration from previous behavior" callout for users who relied on the old "one-shot, no config touched" semantics.

---

## 12. Key file references (current code, single-DB)

For reviewers tracking the diff:

- `src/server/state.rs:23-39` — `AppState` (refactor target)
- `src/server/state.rs:41-50` — `FileCheckCache` (moves into `DatabaseContext`)
- `src/server/mod.rs:154-194` — Route table (nested under `/api/db/{db_id}`)
- `src/server/handlers.rs` — All handlers (signatures change to take `DbContext`)
- `src/db.rs:43-52` — `Database<'a>` (unchanged; one instance per `DatabaseContext`)
- `src/tauri_main.rs:8-21` — `TauriConfig` (v2 schema)
- `src/tauri_main.rs:335-360` — config load/save (migration)
- `src/cli_main.rs:217-305` — Server command (wraps single DB into a map)
- `static/src/hooks/useUrlState.ts:70-93` — `useProjectTarget` → `useDbProjectTarget`
- `static/src/api/client.ts` — every method takes `dbId`
- `static/src/components/ProjectTargetSelector.tsx` — keyed on (db, project, target)
