# PSF Guard web interface

This React app is the shared PSF Guard interface. The desktop app embeds it,
and `psf-guard server` serves the same files to a browser.

## Development

### Prerequisites

- Node.js 24.15+ (24.x LTS, as in CI) and npm
- The PSF Guard backend server running

### Setup

```bash
# Install dependencies
npm install

# Start development server
npm run dev
```

The development server runs on http://localhost:5173 and proxies API requests to the backend at http://localhost:3000.

### Running with Backend

1. Start the backend server:
   ```bash
   cargo run -- server catalog.sqlite /path/to/images
   ```

2. Start the frontend dev server:
   ```bash
   cd static
   npm run dev
   ```

## Production build

```bash
# From static/
npm run build
```

`cargo build` also runs this build when the frontend sources are newer than
`static/dist`, then embeds that directory in the binary.

To test the loose files instead of the embedded copy:

```bash
cargo run --release -- server catalog.sqlite /path/to/images \
  --static-dir static/dist
```

## Running e2e locally

Run Playwright through the guard script, not bare `npx playwright test`:

```bash
cargo build --release                            # the server the specs start
scripts/e2e-local.mjs e2e/navigation.spec.ts     # from the repository root
scripts/e2e-local.mjs --director                 # Director suite; needs npm run build
```

Arguments it does not know go to `playwright test`. The script:

- holds a machine-wide lock in `/tmp/psf-guard-e2e.lock`, so only one run
  goes at a time across worktrees. A second run prints
  `waiting for <worktree> (pid …, started …)` and waits; `--no-wait` exits
  with status 75 instead. A lock whose owner has died is broken.
- refuses to start while Playwright browsers sit in the exiting state (`E`
  in `ps`). macOS cannot kill them, even with `kill -9`, and new browser
  launches hang behind them. Reboot; do not retry or `pkill`.
- uses `PSF_GUARD_E2E_BINARY`, or else this worktree's
  `target/release/psf-guard`, and ports derived from the worktree path
  unless `PSF_GUARD_E2E_PORT` or `PSF_GUARD_DIRECTOR_E2E_PORT` is set. It
  refuses a port that is already in use rather than reuse that server.
- stops the run after `--max-time` (30 minutes), after `--idle` without
  output (5 minutes), or on Ctrl-C. It sends SIGTERM to the process groups
  the run created, browsers and servers included, then SIGKILL after
  `--grace` (10 seconds). It never signals anything else.
- ends with one line: passed, failed, timed out, watchdog, or interrupted.

Without the script, both Playwright configs still stop a local run after
30 minutes (`globalTimeout`); CI keeps no limit. `npm run test:scripts`
tests the script itself.

### Reshoot the import guide

From the repository root, run the capture spec through the local guard:

```bash
PSF_GUARD_E2E_BINARY=/path/to/psf-guard \
PSF_GUARD_E2E_TMP=/tmp/psf-guard-012-flow \
PSF_GUARD_CAPTURE_DIR=/absolute/path/to/captures \
scripts/e2e-local.mjs e2e/import-flow-captures.spec.ts --project=chromium
```

The spec creates disposable catalogs, imports a TS snapshot, saves sync settings,
previews and applies a transfer, and opens plugin pairing. It saves eight PNGs
and `import-flow-captures.json` with the build, fixture paths, viewport, UI state,
crop and checksum for each image. It skips the normal suite unless
`PSF_GUARD_CAPTURE_DIR` is set. Use a fresh capture directory and a disposable
`PSF_GUARD_E2E_TMP` path; global setup clears that fixture directory.

## Styling

Colours are tokens in the `:root` block of `src/App.css`. Stylesheets and
inline styles use `var(--…)`, not literal colours. Name a new token by what
it means, such as `--color-status-bad` or `--color-calibration-poor`, not by
its value. A chart with a palette of its own, such as the framing's rig
colours or the visibility chart, declares it as custom properties on the
chart's root element.

Interaction states take the `--state-*` tokens:

- **On**: a pressed toggle, an open menu, a chosen tab or row, or the current
  view. Style it from the ARIA attribute the control carries
  (`aria-pressed`, `aria-expanded`, `aria-selected`, `aria-checked` or
  `aria-current`) with `--state-on-border` and `--state-on-bg`. A tab uses
  its underline and `--state-on-fg`. Where a tint would not read, such as
  the header's current view or a toggle over an image, the control fills
  with `--state-on-fill`.
- **Set**: a control whose value is off its default, so the view differs
  from how it opens: a filter or rule that narrows what is shown, or a
  changed sort, such as View when Newest first is on. Mark it `.is-set` and
  use `--state-set-*`. A control that only opens a panel says it with
  `aria-expanded`, not `.is-set`.
- **Focus**: `--state-focus-border` with `--state-focus-ring` on fields and
  boxed controls, or `outline: 2px solid var(--state-focus-border)`.

Form fields need no colours of their own. A base rule in `src/index.css`
draws text, number, search and date inputs, selects and textareas from the
`--color-input-*` tokens, including focus, `aria-invalid='true'` and
disabled, at no specificity, so a component sets only the size and spacing
it needs. The theme's `:root` block sets `color-scheme: dark`, which the
browser uses for its own parts of a control: spinners, date pickers, a
select's menu and scrollbars. A theme added later sets its own
`color-scheme` and input tokens beside its other tokens;
`src/__tests__/formFields.test.ts` checks that every theme does.
Dropdowns use a shared inset arrow in `src/index.css`, with space reserved
for it. Native menus and keyboard controls still work. List boxes and
toolbar pills keep their own styles; forced-colors mode uses the system arrow.

Write attribute selectors with single quotes, as in `[aria-pressed='true']`.
`src/components/toolbar/toolbar.css` shows the pattern: its states come from
one section and it writes no colour of its own.

`src/__tests__/themeTokens.test.ts` holds this in place. It fails on a colour
literal outside a token definition, a colour in a component's source, a
class that only says a state, or a double-quoted attribute selector. The few
kept on purpose are listed in `src/__tests__/themeTokens.allowlist.ts`, which
may only shrink; regenerate it after removing one with
`UPDATE_THEME_ALLOWLIST=1 npx vitest run src/__tests__/themeTokens.test.ts`.

## Features

- **Catalog overview** across every configured database. It keeps the scope of
  the view you came from, so it marks and scrolls to the project you were
  working in, and Images or Sequence takes you straight back to it.
- **Grid and sequence review** with project, target, grade, filter, date,
  search, and grouping controls.
- **Keyboard-first grading**: arrows or `J`/`K` navigate, `Space` toggles
  selection, `A` accepts, `X` rejects, and `U` returns a frame to Pending.
- **Full image inspection** with pan, wheel/pinch zoom, fit, and 1:1 views.
- **Comparison, star, PSF, sky, and satellite overlays** from the detail view.
- **Quality scanning, stack previews, planning, import, export, and catalog
  management** through the same API as the desktop app.

## Architecture

- **React 19** with TypeScript
- **Vite** for fast development and optimized builds
- **React Query** for server state management
- **Axios** for API communication
- **react-hotkeys-hook** for keyboard shortcuts
- **Playwright** and **Vitest** for browser and component tests

## API Integration

The frontend expects the backend API at `/api`. Vite proxies this to
`http://localhost:3000` in development. In production, the Rust server serves
both the API and the embedded frontend.
