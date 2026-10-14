# PSF Guard web interface

This React app is the shared PSF Guard interface. The desktop app embeds it,
and `psf-guard server` serves the same files to a browser.

## Development

### Prerequisites

- Node.js 18+ and npm
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
- **Set**: a filter or rule narrows what is shown. Mark it `.is-set` and use
  `--state-set-*`.
- **Focus**: `--state-focus-border` with `--state-focus-ring` on fields and
  boxed controls, or `outline: 2px solid var(--state-focus-border)`.

Write attribute selectors with single quotes, as in `[aria-pressed='true']`.
`src/components/toolbar/toolbar.css` shows the pattern: its states come from
one section and it writes no colour of its own.

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
