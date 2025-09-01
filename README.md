# PSF Guard

[![CI](https://github.com/theatrus/psf-guard/actions/workflows/ci.yml/badge.svg)](https://github.com/theatrus/psf-guard/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

A Rust utility for astronomical image analysis and grading, with N.I.N.A. Target Scheduler integration.

## Features

- **N.I.N.A. Integration**: Query and analyze Target Scheduler SQLite databases
- **Star Detection**: N.I.N.A. algorithm port + HocusFocus detector with PSF fitting
- **Web Interface**: React-based UI for visual image grading with zoom/pan
- **Statistical Analysis**: Advanced outlier detection using HFR, star count, and cloud detection
- **FITS Processing**: Convert to PNG, annotate stars, visualize PSF residuals
- **Multi-Directory Support**: Scan multiple image directories with priority ordering

## Quick Start

### Docker (Recommended)

```bash
# Pull and run
docker pull ghcr.io/theatrus/psf-guard:latest

docker run -d -p 3000:3000 \
  -v /path/to/database.sqlite:/data/database.sqlite:ro \
  -v /path/to/images:/images:ro \
  ghcr.io/theatrus/psf-guard:latest \
  server /data/database.sqlite /images
```

### Build from Source

```bash
git clone https://github.com/theatrus/psf-guard.git
cd psf-guard
cargo build --release

# Run server
./target/release/psf-guard server schedulerdb.sqlite /path/to/images/
# Open http://localhost:3000
```

### Multi-Directory Usage

```bash
# Scan multiple directories in priority order (first-hit wins)
psf-guard server db.sqlite /primary/images/ /backup/images/ /archive/images/
```

## Docker Compose

Create `docker-compose.yml`:

```yaml
version: '3.8'
services:
  psf-guard:
    image: ghcr.io/theatrus/psf-guard:latest
    ports: ["3000:3000"]
    volumes:
      - /path/to/schedulerdb.sqlite:/data/database.sqlite:ro  
      - /path/to/images:/images:ro
    command: server /data/database.sqlite /images
    restart: unless-stopped
```

Run with: `docker-compose up -d`

## Database Location

**Windows N.I.N.A.:**
```
%LOCALAPPDATA%\NINA\SchedulerPlugin\schedulerdb.sqlite
```

## Web Interface

### Key Features
- **Smart Loading**: Fast preview → full resolution on zoom
- **Comparison View**: Side-by-side with independent zoom/pan  
- **Batch Operations**: Multi-select with Shift+Click, Ctrl+Click
- **Undo/Redo**: Full history with Ctrl+Z/Ctrl+Y
- **Cache Progress**: Real-time directory scanning with progress indicators

### Keyboard Shortcuts

| Key | Action | Key | Action |
|-----|--------|-----|--------|
| K/→ | Next image | A | Accept |
| J/← | Previous | X | Reject |  
| C | Compare | U | Unmark |
| S | Stars overlay | +/- | Zoom |
| Ctrl+Z | Undo | Ctrl+Y | Redo |

## CLI Commands

### Core Commands

```bash
# Web server
psf-guard server <database> <image-dirs...> [--port 3000]

# Move rejected images  
psf-guard filter-rejected <database> <image-dir> [--dry-run] [--project NAME]

# Star detection analysis
psf-guard analyze-fits image.fits [--compare-all] [--detector nina|hocusfocus]

# Create annotated images
psf-guard annotate-stars image.fits [--max-stars 50] [--color red|yellow]
```

### Database Queries

```bash
# List projects and targets
psf-guard list-projects -d database.sqlite
psf-guard list-targets "Project Name" -d database.sqlite

# Export grading data
psf-guard dump-grading -d database.sqlite [--project NAME]
```

### FITS Processing

```bash
# Convert with MTF stretch
psf-guard stretch-to-png image.fits output.png

# PSF analysis grid
psf-guard visualize-psf-multi image.fits [--num-stars 25]

# Metadata display
psf-guard read-fits image.fits
```

## Statistical Grading

Advanced outlier detection beyond database status:

- **HFR Analysis**: Focus quality per target/filter
- **Star Count**: Abnormal detection counts  
- **Cloud Detection**: Sequence analysis for weather
- **Distribution Analysis**: MAD for skewed data

Enable with `--enable-statistical` flag.

## REST API

```bash
# List images with filters
curl "localhost:3000/api/images?project_id=2&status=pending"

# Update grading
curl -X PUT localhost:3000/api/images/123/grade \
  -H "Content-Type: application/json" \
  -d '{"status": "accepted"}'

# Get processed images
curl "localhost:3000/api/images/123/preview?size=large" -o preview.png
curl "localhost:3000/api/images/123/annotated" -o stars.png
```

## Cache System

- **Auto-refresh**: Both file and directory caches refresh every 5 minutes
- **Manual refresh**: UI button for file cache, Shift+click for both
- **Real-time progress**: Live updates during directory scanning
- **Multi-directory**: Scans all directories with first-hit preference

## Development

```bash
# Setup
cargo fmt && cargo clippy && cargo test

# Run with logging
RUST_LOG=debug cargo run -- server db.sqlite images/

# Frontend development
cd static && npm run dev

# OpenCV (optional, enhanced star detection)
brew install opencv  # macOS
```

See [CLAUDE.md](CLAUDE.md) for architecture details.

## License

Apache License 2.0 - See [LICENSE](LICENSE)