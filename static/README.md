# PSF Guard Web Interface

This is the React-based web interface for PSF Guard, providing an efficient image grading workflow for astronomical images.

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
   cargo run -- server schedulerdb.sqlite /path/to/images
   ```

2. Start the frontend dev server:
   ```bash
   cd static
   npm run dev
   ```

## Production Build

### Using Make (Recommended)

From the project root:

```bash
# Build everything (frontend + backend)
make build

# Build only frontend
make build-frontend

# Clean and rebuild
make clean build
```

### Manual Build

```bash
# From the static directory
npm run build

# Copy to dist
mkdir -p ../dist/static
cp -r dist/* ../dist/static/
```

### Serving Production Build

After building, run the server with the static directory:

```bash
cargo run --release -- server schedulerdb.sqlite /path/to/images --static-dir dist/static
```

## Features

- **Keyboard-First Navigation**: Optimized for efficient image grading
  - `j`/`k` or arrow keys: Navigate images
  - `a`: Accept image
  - `r`: Reject image
  - `u`: Unmark (set to pending)
  - `s`: Toggle star overlay
  - `?`: Show help

- **Image Grouping**: Images are automatically grouped by filter type
- **Adjustable Thumbnails**: Slider to resize images from 150px to 1200px
- **Image Preloading**: Smooth navigation with automatic preloading
- **Full Image View**: Double-click or Enter to see full details
- **Statistics**: HFR and star count displayed for each image

## Architecture

- **React 19** with TypeScript
- **Vite** for fast development and optimized builds
- **React Query** for server state management
- **Axios** for API communication
- **react-hotkeys-hook** for keyboard shortcuts

## API Integration

The frontend expects the backend API to be available at `/api`. In development, this is proxied to `http://localhost:3000`. In production, the backend serves both the API and static files.