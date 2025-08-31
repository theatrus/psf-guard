# PSF Guard

[![CI](https://github.com/theatrus/psf-guard/actions/workflows/ci.yml/badge.svg)](https://github.com/theatrus/psf-guard/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

A comprehensive Rust utility for astronomical image analysis and grading, with N.I.N.A. Target Scheduler integration.

## Features

- **Target Scheduler Integration**: Query and analyze N.I.N.A. Target Scheduler SQLite databases
- **Star Detection**: N.I.N.A. algorithm port + HocusFocus detector with PSF fitting
- **Web Interface**: React-based UI for visual image grading with zoom/pan controls
- **Statistical Analysis**: Advanced outlier detection using HFR, star count, and cloud detection
- **FITS Processing**: Convert to PNG, annotate stars, visualize PSF residuals
- **File Organization**: Auto-move rejected images based on grading status

## Documentation

- [Statistical Grading Guide](STATISTICAL_GRADING.md) - Detailed statistical analysis features
- [Development Notes](CLAUDE.md) - Technical implementation details

## Quick Start

### Docker (Recommended)

```bash
# Pull from GitHub Container Registry
docker pull ghcr.io/theatrus/psf-guard:latest

# Run the web server
docker run -d \
  -p 3000:3000 \
  -v /path/to/database.sqlite:/data/database.sqlite:ro \
  -v /path/to/images:/images:ro \
  ghcr.io/theatrus/psf-guard:latest \
  server /data/database.sqlite /images

# Or use docker-compose (see docker-compose.yml)
docker-compose up -d
```

### Build from Source

```bash
# Clone and build
git clone https://github.com/theatrus/psf-guard.git
cd psf-guard
cargo build --release

# Optional: Install OpenCV for enhanced star detection
brew install opencv  # macOS
```

### Web Server (Recommended)

```bash
# Start server with embedded UI
psf-guard server schedulerdb.sqlite /path/to/images/

# Open browser to http://localhost:3000
```

### CLI Examples

```bash
# List projects and targets
psf-guard list-projects
psf-guard list-targets "Project Name"

# Analyze FITS files (no database needed)
psf-guard analyze-fits image.fits --compare-all
psf-guard annotate-stars image.fits --max-stars 100

# Filter rejected images (dry run first!)
psf-guard filter-rejected db.sqlite /images --dry-run
psf-guard filter-rejected db.sqlite /images --project "M31"
```

## Docker Usage

### Running with Docker

PSF Guard is available as a Docker image with all dependencies pre-installed, including OpenCV support.

```bash
# Pull the latest image
docker pull ghcr.io/theatrus/psf-guard:latest

# Run the web server (read-only mounts recommended)
docker run -d \
  --name psf-guard \
  -p 3000:3000 \
  -v /path/to/schedulerdb.sqlite:/data/database.sqlite:ro \
  -v /path/to/images:/images:ro \
  ghcr.io/theatrus/psf-guard:latest \
  server /data/database.sqlite /images

# Run CLI commands
docker run --rm \
  -v /path/to/schedulerdb.sqlite:/data/database.sqlite:ro \
  ghcr.io/theatrus/psf-guard:latest \
  list-projects -d /data/database.sqlite

# Analyze FITS files
docker run --rm \
  -v /path/to/images:/images:ro \
  ghcr.io/theatrus/psf-guard:latest \
  analyze-fits /images/M31/2025-08-30/LIGHT/M31_Ha_300s_001.fits
```

### Docker Compose

Create a `docker-compose.yml` file:

```yaml
version: '3.8'

services:
  psf-guard:
    image: ghcr.io/theatrus/psf-guard:latest
    container_name: psf-guard
    ports:
      - "3000:3000"
    volumes:
      - /path/to/schedulerdb.sqlite:/data/database.sqlite:ro
      - /path/to/images:/images:ro
    command: server /data/database.sqlite /images
    restart: unless-stopped
```

Then run:
```bash
docker-compose up -d
docker-compose logs -f  # View logs
docker-compose down     # Stop
```

### Building Your Own Image

```bash
# Build locally with podman or docker
podman build -t psf-guard:local .
docker build -t psf-guard:local .

# Run your local build
docker run -p 3000:3000 \
  -v ./database.sqlite:/data/database.sqlite:ro \
  -v ./images:/images:ro \
  psf-guard:local server /data/database.sqlite /images
```

### Volume Mounts

- `/data/database.sqlite` - N.I.N.A. Target Scheduler database (read-only recommended)
- `/images` - Directory containing your FITS images (read-only recommended)

Use `:ro` for read-only mounts to prevent accidental modifications.

### Container Tags

- `latest` - Latest stable release
- `main` - Latest development build
- `v1.0.0` - Specific version
- `v1.0` - Latest patch of v1.0
- `v1` - Latest minor/patch of v1

## Target Scheduler Database Location

**Windows:**
```
%LOCALAPPDATA%\NINA\SchedulerPlugin\schedulerdb.sqlite
```
(Usually `C:\Users\[Username]\AppData\Local\NINA\SchedulerPlugin\schedulerdb.sqlite`)

## Web Interface

### Key Features

- **Smart Image Loading**: Fast 2000px preview → full resolution on zoom
- **Comparison View**: Side-by-side with independent or synced zoom/pan
- **Batch Operations**: Multi-select with Shift+Click, Ctrl+Click
- **Undo/Redo**: Full history with Ctrl+Z/Ctrl+Y
- **Keyboard Navigation**: J/K for prev/next, A/R/U for grading

### Keyboard Shortcuts

| Key | Action | Key | Action |
|-----|--------|-----|--------|
| K/→ | Next image | A | Accept |
| J/← | Previous | X | Reject |
| C | Compare view | U | Unmark |
| S | Toggle stars | +/- | Zoom |
| P | Toggle PSF | F | Fit screen |
| Ctrl+Z | Undo | 1 | 100% zoom |

### REST API

```bash
# List images with filters
curl "localhost:3000/api/images?project_id=2&status=rejected"

# Update grading
curl -X PUT localhost:3000/api/images/123/grade \
  -H "Content-Type: application/json" \
  -d '{"status": "accepted"}'

# Get processed images
wget "localhost:3000/api/images/123/preview?size=large" -O preview.png
wget "localhost:3000/api/images/123/annotated" -O stars.png
```

## Command Reference

### Core Commands

#### `server` - Web interface with REST API
```bash
psf-guard server <database> <image-dir> [--port 3000] [--host 127.0.0.1]
```

#### `filter-rejected` - Move rejected files
```bash
psf-guard filter-rejected <database> <image-dir> [options]
  --dry-run                    Preview changes
  --project NAME               Filter by project
  --enable-statistical         Enable outlier detection
  --stat-hfr                   HFR analysis
  --stat-clouds                Cloud detection
```

#### `analyze-fits` - Star detection comparison
```bash
psf-guard analyze-fits <path> [options]
  --compare-all                Compare all detectors
  --detector nina|hocusfocus   Choose detector
  --psf-type gaussian|moffat   PSF fitting
```

#### `annotate-stars` - Create star maps
```bash
psf-guard annotate-stars <fits> [options]
  --max-stars 50               Number to annotate
  --color red|yellow|green     Annotation color
```

#### `visualize-psf-multi` - PSF analysis grid
```bash
psf-guard visualize-psf-multi <fits> [options]
  --num-stars 25               Stars to analyze
  --selection corners|regions  Selection strategy
```

### Database Commands

- `list-projects` - Show all projects
- `list-targets <project>` - Show project targets
- `dump-grading` - Export grading results
- `regrade` - Reapply statistical analysis

### FITS Processing

- `read-fits` - Display FITS metadata
- `stretch-to-png` - Convert with MTF stretch
- `benchmark-psf` - Performance testing

## Statistical Grading

Advanced outlier detection beyond database status:

- **HFR Analysis**: Focus quality outliers (per target/filter)
- **Star Count**: Abnormal detection counts
- **Cloud Detection**: Sequence analysis for weather events
- **Distribution Analysis**: MAD for skewed data

See [STATISTICAL_GRADING.md](STATISTICAL_GRADING.md) for details.

## Directory Structures

Automatically handles multiple layouts:

```
Standard:                    Alternate:
files/                       files/
└── 2025-08-25/             └── Target Name/
    └── Target Name/             └── 2025-08-25/
        └── 2025-08-25/              ├── LIGHT/
            ├── LIGHT/               └── LIGHT_REJECT/
            └── LIGHT_REJECT/
```

## Development

```bash
# Setup
cargo fmt && cargo clippy && cargo test

# Run with logging
RUST_LOG=debug cargo run -- server db.sqlite images/

# Frontend development
cd static && npm run dev
```

See [CLAUDE.md](CLAUDE.md) for architecture details.

## License

Apache License 2.0 - See [LICENSE](LICENSE)

## Contributing

Contributions welcome! Please submit pull requests.