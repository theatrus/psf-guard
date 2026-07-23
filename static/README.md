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

## Features

- **Catalog overview** across every configured database.
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
