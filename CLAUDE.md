# PSF Guard contributor guide

This is the canonical guide for people and coding agents working in this
repository. `AGENTS.md` is a symlink to this file so both entry points stay in
sync.

## Writing

### Write for the reader's task

Start with what the reader can do or needs to know. Give instructions in the
order the reader will follow them: where to go, what to choose, and what happens
next. State prerequisites before the step that needs them.

- Use direct actions: “Open Settings,” “Choose a rig,” “Click Save.”
- Use the reader as the subject of instructions. Use PSF Guard, the server,
  or the relevant tool as the subject when explaining automatic behavior.
- State the result: “Click Save. PSF Guard applies the settings to the next
  stack.” Avoid vague promises such as “Save handles the changes.”
- Use headings that help readers find a task or answer: “Add calibration
  frames,” “Check coverage,” or “How matching works.”
- Explain technical details when they affect a choice, a result, or recovery
  from an error. Keep implementation detail out of routine instructions.

An explanation need not be an instruction. “The server checks for new frames
at startup” is clear. Do not turn automatic behavior into a step the reader
must perform.

### Prefer natural, precise language

Use Orwell's rules as a test of clarity, not a demand for the shortest words:

1. Avoid stock metaphors and figures of speech.
2. Prefer a short, familiar word when it preserves the meaning.
3. Cut words that add nothing.
4. Prefer active voice when the actor matters.
5. Keep technical terms when they are more precise than an everyday substitute.
6. Break these rules before writing something awkward or misleading.

Short words can still make a sentence hard to read. Do not replace a clear
term such as “queue,” “settings,” “position,” or “progress” with a strained
phrase. Use “disabled” for a control that cannot be used, “off” for a setting
that is off, and “collapsed” for a section whose contents are hidden.

Give each sentence a clear subject and verb. Split sentences that combine
several actions, conditions, or exceptions. Repeat a noun when “it,” “one,”
“those,” or “the same” could refer to more than one thing.

### Say what the interface does

Do not make a page, row, label, or button sound like a person. Avoid using
“names,” “says,” “asks,” “takes,” or “holds” as substitutes for a precise verb.
These words are not banned: “Enter a name” and “The dialog asks you to confirm”
are natural. Choose the verb that describes the behavior:

- A page **shows** information or **lists** items.
- A message **explains** a problem or **identifies** the affected file.
- A button **opens**, **starts**, **saves**, or **stops** something.
- A control **is disabled** until its prerequisite is met.
- The server **stores**, **checks**, **matches**, or **writes** data.

| Avoid | Prefer |
|---|---|
| The row names what waits. | The row lists the queued jobs. |
| The error names the folder. | The error message shows which folder could not be opened. |
| The rigs hold the plan. | Each rig's settings match the saved plan. |
| The preview lands. | The preview is ready. |
| The button waits for it. | Activate stays disabled until the preview is ready. |
| The bar asks while the plan needs activating. | A banner explains why the plan needs activation. |
| Settings closes over unsaved edits. | Settings asks before discarding unsaved changes. |
| The address keeps the pick. | The URL stores the selected filter. |
| A run takes the frames its rig wants. | The run uses calibration frames that match the rig's settings. |
| The panel gives how far it has got. | The panel shows the job's progress. |

Rewrite the sentence, not just the verb. “The row identifies what waits” still
leaves the reader to work out what is waiting and why. Do not replace every
“names” with “shows” or every “holds” with “stores.” Check the meaning first.

### Explain changes in the reader's terms

For a change or deprecation, explain what happens to the reader's setup, when
it happens, what PSF Guard does automatically, and whether the reader needs
to act. State the benefit in concrete terms. Do not make readers translate
internal records or design goals into consequences for their work.

If existing and new setups need different steps, state what each should do.
Name the replacement and where to find it.

| Avoid | Prefer |
|---|---|
| Existing direct-open registrations remain supported through 0.12. | Your current Target Scheduler setup will keep working in 0.12. |
| A later release will migrate them automatically. | A later update will create a PSF Guard catalog for you and keep captures, plans and grades in sync with Target Scheduler. |
| Separate catalogs give each product control of its own data. | Separate catalogs let us add features to PSF Guard and support new Target Scheduler versions. |

Name what the reader keeps or gets: images, plans, grades, settings or a working
connection. Explain “migrate,” “adopt,” “retain” or “ownership” when their effect
would otherwise be unclear. These words are not banned; keep precise technical
terms in API and design documentation, and explain them when they affect a
user's choice. Never replace terms in bulk.

### Preserve technical meaning

- In documentation, copy UI labels exactly, including capitalization. When
  changing a UI label, update its accessible name, tests, and current guides
  together. Keep API keys and stored values unchanged.
- Keep commands, option names, API fields, units, limits and algorithm names
  exact. A prose edit must not change code or product behavior.
- Preserve distinctions such as queued versus waiting, planned versus
  measured, and warnings versus rejection. Do not imply that missing evidence
  proves a failure.
- State conditions explicitly. “When X, do Y” is clearer than a chain of
  “else,” “one,” and “it.” Use a list for several branches.
- Link with the destination's subject, such as “Flat group settings,” rather
  than “above,” “below,” or “here.” Sections may move to another page.

### Review UI text in context

These rules apply to labels, tooltips, help text, empty states, errors,
confirmation dialogs, accessibility text, documentation, commits and PRs.
Check server-supplied text as well as frontend components.

Read each passage as someone who has not seen the implementation. Make clear
who acts, what changes, when it happens, and what the user can do next. Name
what a limit measures, what a blank field means, and which settings override
which defaults. Use “delete” or “remove” when that is the action; do not hide
file deletion behind “go” or “clear.” Preserve exceptions and recovery steps.

Read awkward sentences aloud. Add words when they remove ambiguity. Do not
use a banned-word list or replace verbs in bulk: check each sentence's meaning.
Keep technical terms, such as queue, threshold and calibration, when they
make the result clearer.

Update the nearest focused document instead of adding another root-level plan.

## Product

PSF Guard is an astrophotography catalog, grader, and quality-analysis tool. It
can build a catalog from FITS folders or open an existing N.I.N.A. Target
Scheduler database. The same Rust application runs as a CLI, an Axum server,
or a Tauri desktop app. A React frontend supplies the grader and management UI.

Main capabilities include:

- multi-database FITS catalog management;
- fast image review, comparison, grading, and export;
- star detection, PSF fitting, spatial screening, and photometry;
- pixel-derived astrometry, sky overlays, and satellite prediction;
- calibrated mono stacks and RGB, LRGB, or narrowband previews;
- header-first imports, background quality backfill, and safe database sync;
- reversible reject archives and remote image intake.

## Toolchain and common commands

The repository uses the Rust toolchain pinned in `rust-toolchain.toml`, Rust
2024, Node 24 in CI, and Tauri 2.

```bash
# Rust checks used by CI
cargo fmt --all -- --check
cargo build
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

# Frontend checks
cd static
npm ci
npm run lint
npx vitest run
npm run build

# Browser end-to-end tests; requires a built psf-guard binary. Scroll specs
# also run in WebKit (Safari's engine and the macOS app's WebView).
npx playwright install chromium webkit
../scripts/e2e-local.mjs e2e/navigation.spec.ts   # locally; CI runs npx playwright test

# Run the server or desktop app
cargo run -- server db.sqlite /path/to/images
cargo tauri dev

# Build release forms
cargo build --release
cargo build --release --features tauri
cargo tauri build
```

Run local Playwright through `scripts/e2e-local.mjs`: one run at a time per
machine; never `pkill` by pattern; if browsers are stuck in the exiting
state, stop and reboot rather than retry. See
[Running e2e locally](static/README.md#running-e2e-locally).

Use `--registry /tmp/psf-guard-test.json` for ad-hoc server work that must not
touch the user's real database registry. Do not run import, sync, catalog
installation, quality scans, or database-management actions against a live
catalog unless the task grants that access.

## Repository map

| Path | Purpose |
|---|---|
| `src/cli_main.rs`, `src/commands/` | CLI commands and batch workflows |
| `src/db.rs`, `src/db_registry.rs` | Target Scheduler access and multi-database registry |
| `src/server/` | Axum API, jobs, caches, import, sync, previews, and remote intake |
| `src/sequence_analysis.rs` | Sequence scoring and issue classification |
| `src/spatial_analysis.rs`, `src/photometry.rs` | Cloud, obstruction, glow, and transparency evidence |
| `src/astrometry.rs`, `src/acquisition_context.rs` | Solving, target context, and pointing evidence |
| `src/satellites.rs` | Cached satellite prediction and trail alignment |
| `src/calibration.rs` | Calibration catalog, matching, and master provenance |
| `static/src/` | React application |
| `static/e2e/` | Playwright coverage and real FITS fixture manifest |
| `docs/` | Current user and engineering documentation |
| `docs/design/` | Design records that still guide active architecture |
| `packaging/` | RPM and platform packaging |
| `icons/` | Generated desktop icons; see `icons/README.md` |

The server caches `notice.json` for 24 hours for both browser and Tauri modes;
UI reloads read `/api/update-notice` and never fetch the public feeds. Only
Tauri loads the signed updater plugin. Both feeds try `updates.psf-guard.com`
before the GitHub release fallback. See [docs/UPDATES.md](docs/UPDATES.md).

## Core invariants

### Databases and grading

- Treat Target Scheduler compatibility as a public contract. Probe optional
  columns and tolerate supported schema differences.
- Target Scheduler stores right ascension in decimal hours. Convert it to ICRS
  degrees at the astrometry boundary.
- `gradingStatus` values are `0 = Pending`, `1 = Accepted`, and `2 = Rejected`.
  Keep `rejectreason` consistent with the grade.
- Match records across databases by stable GUID. The one exception is a
  destination row PSF Guard minted itself (an import or remote upload) for a
  frame whose telescope row then arrives: sync recognizes it by target, file
  name, and capture time and updates it in place rather than inserting a
  twin. Beyond that, do not substitute names or nearby coordinates for sync
  identity.
- Open a source database read-only during sync. Refuse the same source and
  destination path. Preview every UI transfer before Apply.
- The Library merges databases, but Grid, Detail, Comparison, and Sequence
  routes must keep the `db` slug in URL state. The Library carries the same
  scope so it can mark where the user was and hand it back, so anything that
  scopes to one database must read `useScopedDbId` and ignore the slug there.

Design records: [multi-database](docs/design/multi-database.md),
[data transfer](docs/design/data-transfer.md), and
[reject archive](docs/design/reject-archive.md).

### Image evidence and grading

- Keep catalog predictions separate from pixel evidence. A FITS header, target
  coordinate, or satellite ephemeris does not prove that the pixels match.
- Quality grading must renormalize when evidence is missing. Do not punish an
  image because an optional scan has not run.
- Only fresh pixel-derived solves support astrometry grading. Embedded WCS can
  support display but does not prove the current pixels.
- A lone no-solve is advisory. Automatic rejection needs corroborating
  pointing, cloud, obstruction, or tracking evidence.
- Cross-frame background and flux comparisons must use physical ADU, not the
  per-frame normalized storage values. A PixInsight XISF frame declaring
  `bounds="0:1"` has no ADU of its own; `src/image_io.rs` puts it on a 16-bit
  scale on the way in, so read every frame through there rather than reaching
  for `seiza_xisf` or `seiza_stacking::FitsFrame::open` directly.
- N.I.N.A. Fast supplies comparable star count and HFR values for imported
  catalogs. HocusFocus remains available for deeper analysis.

Current behavior: [quality screening](docs/SCREENING.md),
[astrometry grading](docs/ASTROMETRY_QUALITY.md), and
[statistical grading](docs/STATISTICAL_GRADING.md).

### Seiza and image processing

- PSF Guard consumes published Seiza crates. Do not copy Seiza algorithms into
  this repository when the crate owns them.
- `seiza-imgproc` replaces OpenCV. Its numeric behavior affects detector
  thresholds and star counts; validate changes on real frames as well as unit
  fixtures.
- Keep the order-sensitive HocusFocus reductions serial unless real-frame
  parity proves a replacement safe.
- Satellite element fetches belong to explicit user actions. Read-only sequence
  requests and ordinary regrading use cached elements.
- Keep satellite predictions labeled as predictions. Use
  `seiza_satellites::trail_alignment` for pixel evidence.

Current behavior: [satellites](docs/SATELLITES.md) and
[stack previews](docs/STACKING_PREVIEWS.md).

### Jobs, caches, and concurrency

- Scope derived artifacts below the database slug. Cache keys must include the
  source fingerprint and every algorithm or catalog input that can change the
  result.
- Write generated artifacts to a temporary file and rename them atomically.
- Preview requests return `202` while queued work runs. Keep generation out of
  the HTTP request path.
- Run long or heavy work as a tracked job with a row in the job line: one
  runner thread per job, no other threads, and no such work in a request
  handler. See [background jobs](docs/design/background-jobs.md).
- Give each job a `concurrency::Priority`: `Interactive` when a person
  started it, `Viewed` for automatic work a page shows, `Background` for the
  rest. Only `Interactive` holds the guard, from
  `AppState::begin_named_interactive_job`. The others yield through
  `AppState::wait_for_interactive_work`, which reports what they wait for.
- Lease workers with `plan_workers` and `AppState::lease_workers`, and run
  Seiza in a `ComputePool`. No fixed worker counts.
- Interactive work wins over background pre-generation and quality backfill.
- Keep the shared request database connection free of slow refresh work.
- A stale directory tree may be served while one background refresh runs. Do
  not make every request rescan storage.

### Imports, files, and calibration

- A frame is a FITS or an XISF file. Detect one with `src/image_io.rs` and read
  one through it; do not test extensions or call `seiza_fits` directly at a new
  site. Both containers decode to the same `seiza_fits::FitsImage`.
- Initial import is header-first. Expensive star, photometry, spatial, and
  astrometry work belongs to the background quality job.
- Import creates Target Scheduler-compatible lights and PSF Guard-owned
  calibration records. Never put calibration frames in `acquiredimage`.
- Preserve raw FITS files. Forgetting a calibration record must not delete its
  source file.
- Invalidate generated masters transitively when an input frame or upstream
  master changes.
- File moves must support a dry run, avoid overwrites, and retain enough state
  for recovery. `move-rejects` and `restore-rejects` follow this rule.

Current behavior: [importing](docs/IMPORTING.md) and
[calibration libraries](docs/CALIBRATION_LIBRARY.md).

## Frontend rules

- Keep project and database scope in URL state so reload, navigation, and
  shared links restore the same view.
- Preserve scroll position while previews load, cache status changes, or data
  refreshes. Safari and the macOS app run WebKit, which has no CSS scroll
  anchoring, so keep the place in code and cover it in
  `static/e2e/detail-scroll.spec.ts`, which also runs in WebKit.
- Grid and Sequence share arrow-key navigation and Space selection.
- Image zoom state is calibrated against the bitmap dimensions recorded by the
  zoom hook. Report every loaded bitmap before constraining pan or scale.
- Use the shared async preview poller; do not add one timer per image.
- Database mutations and catalog installation must remain behind the Tauri or
  server database-management gate.
- Colours and interaction states come from the theme tokens in
  `static/src/App.css`; follow the Styling section of `static/README.md`.
  Write no colour literals in CSS or inline styles; add a token named by its
  meaning instead. Style on and open from the control's ARIA attribute with
  the `--state-on-*` tokens, a control whose value is off its default (a
  narrowing filter or a changed sort) with `.is-set`, and focus
  with `--state-focus-*`. `src/__tests__/themeTokens.test.ts` fails on a new
  literal or a new state class.

## Change workflow

1. Read the focused document and nearby tests before editing.
2. Preserve unrelated worktree changes. Use a new worktree for a separate PR.
3. Add or update a regression test for changed behavior.
4. Run the narrow checks while iterating, then the relevant Rust, frontend, and
   browser layers before handoff.
5. Run `git diff --check` and review the final diff for stale links, generated
   files, and unrelated changes.
6. Add a line to `docs/releases/unreleased.md` when the change is visible to a
   user. Describe the behavior they get, not the commit. Skip it for internal
   refactors, test-only work, and dependency bumps.
7. For a PR, state exactly which checks ran. Do not treat hosted CI as a local
   test result.

## Documentation

Start at [docs/README.md](docs/README.md). Keep current behavior in the focused
user or engineering guide. Keep only architecture records that still explain a
live contract under `docs/design/`. Delete completed implementation plans once
tests and current docs cover them.
