# PSF Guard

[![CI](https://github.com/theatrus/psf-guard/actions/workflows/ci.yml/badge.svg)](https://github.com/theatrus/psf-guard/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

A Rust utility for analyzing N.I.N.A. (Nighttime Imaging 'N' Astronomy) Target Scheduler plugin databases and managing astronomical image files. The database integration features are specifically designed for users of the [N.I.N.A. Target Scheduler plugin](https://tcdev.dk/nina-ts/). Many features (like FITS analysis and star detection) can be used independently without the Target Scheduler.

## Documentation

- [Statistical Grading Guide](STATISTICAL_GRADING.md) - Detailed documentation on statistical analysis features
- [Development Notes](CLAUDE.md) - Technical implementation details for developers

## Overview

PSF Guard provides tools to:
- **Target Scheduler Integration**: Query and analyze image grading results from N.I.N.A. Target Scheduler plugin SQLite databases
- **Project Management**: List projects and targets with their imaging statistics  
- **File Organization**: Filter and organize rejected image files based on database grading status
- **FITS Analysis**: Read and display metadata from FITS astronomical image files (works independently)
- **Star Detection**: Multiple star detection algorithms (NINA and HocusFocus) with comparison
- **PSF Fitting**: Gaussian and Moffat PSF fitting for accurate FWHM measurements
- **Image Visualization**: Convert FITS to PNG with MTF stretching and star annotations
- **Statistical Grading**: Advanced outlier detection using HFR, star count, and cloud detection algorithms
- **Multiple Formats**: Support for JSON, CSV, and table output formats
- **Directory Support**: Handle multiple directory structures for image organization

### Target Scheduler Database Location

If you're using the N.I.N.A. Target Scheduler plugin, the SQLite database file (`schedulerdb.sqlite`) is typically located at:

**Windows:**
```
%LOCALAPPDATA%\NINA\SchedulerPlugin\schedulerdb.sqlite
```
(Usually: `C:\Users\[YourUsername]\AppData\Local\NINA\SchedulerPlugin\schedulerdb.sqlite`)

You can copy this database file to your working directory or reference it directly in the commands.

## Key Features

### Star Detection
PSF Guard includes two advanced star detection algorithms:

1. **NINA Star Detection**: Faithful Rust implementation of N.I.N.A.'s star detection
   - MTF (Midtone Transfer Function) stretching
   - Multiple sensitivity levels (normal, high, highest)
   - Accurate HFR calculations matching N.I.N.A.'s results

2. **HocusFocus Detection**: Enhanced algorithm with PSF fitting
   - Optional Gaussian and Moffat PSF fitting
   - Sub-pixel accuracy with bilinear interpolation
   - Automatic OpenCV acceleration with fallback to pure Rust

### PSF Fitting
Advanced Point Spread Function analysis:
- Gaussian and Moffat (beta=4.0) models
- Levenberg-Marquardt optimization
- Goodness-of-fit metrics (R², RMSE)
- FWHM and eccentricity calculations

### Visualization Tools
- **FITS to PNG conversion** with customizable MTF stretching
- **Star annotation overlays** with HFR-based sizing
- **PSF residual visualization** showing observed vs fitted data
- **Multi-star grid displays** with various selection strategies

## Installation

### Prerequisites
- Rust 1.89.0 (see rust-toolchain.toml)
- SQLite3
- Optional: OpenCV 4.x for enhanced star detection (see [CLAUDE.md](CLAUDE.md) for setup)

### Building from Source
```bash
git clone https://github.com/theatrus/psf-guard.git
cd psf-guard
cargo build --release
```

The compiled binary will be available at `target/release/psf-guard`.

## Usage

PSF Guard can be used with or without a Target Scheduler database:
- **With database**: Access grading results, project/target information, and sync file operations
- **Without database**: FITS analysis, star detection, PSF fitting, and image visualization work independently

### Basic Commands (Target Scheduler Integration)

#### List all projects
```bash
psf-guard list-projects
```

#### List targets for a specific project
```bash
psf-guard list-targets "Project Name"
```

#### Dump grading results
```bash
# Show all images
psf-guard dump-grading

# Filter by status (pending, accepted, rejected)
psf-guard dump-grading --status rejected

# Filter by project
psf-guard dump-grading --project "Cygnus Wall"

# Filter by target
psf-guard dump-grading --target "North American"

# Output formats (table, json, csv)
psf-guard dump-grading --format json
```

### Standalone Features (No Database Required)

These commands work with FITS files directly and don't require a Target Scheduler database:

#### Read FITS File Metadata

Display metadata from FITS astronomical image files:

```bash
# Read a single FITS file
psf-guard read-fits image.fits

# Read all FITS files in a directory (recursive)
psf-guard read-fits /path/to/fits/directory

# Show all header keywords (verbose mode)
psf-guard read-fits --verbose image.fits

# Output formats (table, json, csv)
psf-guard read-fits --format json image.fits
psf-guard read-fits --format csv /path/to/fits/directory
```

#### Analyze FITS with Star Detection

Compare star detection algorithms and analyze image quality:

```bash
# Basic analysis with HocusFocus detector
psf-guard analyze-fits image.fits

# Compare all detection algorithms
psf-guard analyze-fits image.fits --compare-all

# Use NINA detector with high sensitivity
psf-guard analyze-fits image.fits --detector nina --sensitivity high

# Add PSF fitting for more accurate measurements
psf-guard analyze-fits image.fits --detector hocusfocus --psf-type moffat
```

#### Convert FITS to PNG

Create stretched PNG images from FITS files:

```bash
# Basic conversion with default MTF stretch
psf-guard stretch-to-png image.fits

# Custom stretch parameters
psf-guard stretch-to-png image.fits -o output.png --midtone 0.3 --shadow 0.002

# Logarithmic stretch with inversion
psf-guard stretch-to-png image.fits --logarithmic --invert
```

#### Annotate Stars

Create PNG images with detected stars marked:

```bash
# Basic star annotation
psf-guard annotate-stars image.fits

# Annotate top 100 stars with yellow circles
psf-guard annotate-stars image.fits --max-stars 100 --color yellow

# Use NINA detector with verbose output
psf-guard annotate-stars image.fits --detector nina --verbose
```

#### Visualize PSF Fitting

Generate detailed PSF analysis visualizations:

```bash
# Visualize PSF for multiple stars
psf-guard visualize-psf-multi image.fits --num-stars 25

# Show corner stars (9-point grid)
psf-guard visualize-psf-multi image.fits --selection corners

# Analyze stars from different image regions
psf-guard visualize-psf-multi image.fits --selection regions --num-stars 20
```

### Filter Rejected Files (Requires Database)

The `filter-rejected` command moves rejected image files to a `LIGHT_REJECT` directory based on database grading status.

**IMPORTANT: Always use `--dry-run` first to preview what will be moved!**

```bash
# Dry run (recommended first step)
psf-guard filter-rejected schedulerdb.sqlite /path/to/images --dry-run

# Filter by project
psf-guard filter-rejected schedulerdb.sqlite /path/to/images --dry-run --project "Double Dragon"

# Actually move files (use with caution!)
psf-guard filter-rejected schedulerdb.sqlite /path/to/images --project "Double Dragon"
```

### Supported Directory Structures

The utility automatically detects and handles two common directory structures:

1. **Standard Structure**: `date/target_name/date/LIGHT/filename.fits`
   ```
   files/
   └── 2025-08-25/
       └── Sh2 157/
           └── 2025-08-25/
               ├── LIGHT/
               │   └── image.fits
               └── LIGHT_REJECT/  (created by utility)
                   └── image.fits
   ```

2. **Alternate Structure**: `target_name/date/LIGHT/filename.fits`
   ```
   files2/
   └── Bubble Nebula/
       └── 2025-08-17/
           ├── LIGHT/
           │   └── image.fits
           └── LIGHT_REJECT/  (created by utility)
               └── image.fits
   ```

The utility also handles files already in `LIGHT/rejected/` subdirectories and moves them to the appropriate `LIGHT_REJECT/` directory.

### Web Server and Frontend

PSF Guard includes a built-in web server with a React-based frontend for visual image grading and analysis. The server provides both a REST API and a complete web interface.

#### Starting the Web Server

```bash
# Start server with embedded frontend (production)
psf-guard server schedulerdb.sqlite /path/to/images/

# Custom port and host
psf-guard server schedulerdb.sqlite /path/to/images/ --port 8080 --host 0.0.0.0

# Development mode (serve from filesystem for hot reload)
psf-guard server schedulerdb.sqlite /path/to/images/ --static-dir ./static/dist
```

Once started, open your browser to `http://localhost:3000` (or your configured port) to access the web interface.

#### Web Interface Features

**Project & Target Navigation**:
- Dropdown selectors for projects and targets
- Live statistics showing total, accepted, and rejected image counts
- Filter images by status, date range, or filter name

**Image Browser**:
- Grid view with lazy loading and virtualization for performance
- Grouping by filter name, date, or both
- Adjustable thumbnail sizes with slider control
- Batch operations with shift+click and ctrl+click selection
- Visual status indicators (accepted=green, rejected=red, pending=orange)
- HFR and star count statistics on image cards

**Image Detail View**:
- Full-resolution FITS preview with MTF stretching
- Comprehensive zoom and pan controls (mouse wheel, click-drag, keyboard shortcuts)
- Star detection overlay toggle (S key)
- PSF residual visualization toggle (P key)
- Complete metadata display (exposure, temperature, camera, statistics)
- One-click grading with keyboard shortcuts (A=accept, R=reject, U=unmark)
- Navigation between images with J/K or arrow keys
- **Undo/Redo System**: Full undo/redo support for all grading actions with Ctrl+Z/Ctrl+Y

**Keyboard Shortcuts**:
- `?` - Show help modal with all shortcuts
- `K / →` - Next image
- `J / ←` - Previous image
- `A` - Accept image
- `R` - Reject image
- `U` - Unmark (set to pending)
- `Ctrl+Z / ⌘Z` - Undo last grading action
- `Ctrl+Y / ⌘Y` - Redo last grading action
- `S` - Toggle star overlay
- `P` - Toggle PSF visualization
- `Z` - Toggle image size
- `+/-` - Zoom in/out
- `F` - Fit to screen
- `1` - 100% zoom
- `0` - Reset zoom
- Mouse wheel - Zoom toward cursor
- Click & drag - Pan when zoomed
- `Esc` - Close modals/views

#### REST API Endpoints

The server provides a complete REST API for programmatic access:

**Projects and Targets**:
- `GET /api/projects` - List all projects
- `GET /api/projects/{id}/targets` - List targets for a project

**Images**:
- `GET /api/images?project_id=X&target_id=Y&status=pending&limit=100&offset=0` - List images with filters
- `GET /api/images/{id}` - Get detailed image information
- `PUT /api/images/{id}/grade` - Update grading status
  ```json
  {"status": "accepted|rejected|pending", "reason": "optional reason"}
  ```

**Image Data**:
- `GET /api/images/{id}/preview?size=screen&stretch=true&midtone=0.2&shadow=-2.8` - Stretched FITS preview (PNG)
- `GET /api/images/{id}/annotated?size=large` - Star-annotated image (PNG)
- `GET /api/images/{id}/psf?num_stars=9&psf_type=moffat&sort_by=r2` - PSF visualization (PNG)
- `GET /api/images/{id}/stars` - Star detection results (JSON)

**Response Format**:
```json
{
  "success": true,
  "data": { /* response data */ },
  "error": null
}
```

#### Caching System

The server includes intelligent caching for performance:
- **Preview Images**: Cached PNG files with different stretch parameters
- **Star Detection**: Cached JSON results for HocusFocus analysis
- **PSF Visualizations**: Cached multi-star PSF residual images
- **Statistics**: Cached FITS metadata and image statistics
- **Cache Management**: Automatic cleanup and configurable cache directory

#### Frontend Technology Stack

- **React 18** with TypeScript
- **Vite** for fast development and building
- **TanStack Query** for API state management and caching
- **React Hotkeys Hook** for keyboard shortcuts
- **Custom hooks** for image zoom/pan functionality
- **CSS Grid/Flexbox** for responsive layouts
- **Single Page Application** with client-side routing

#### Deployment Options

**Production (Single Binary)**:
The frontend is automatically built and embedded into the Rust binary during compilation, creating a self-contained executable:
```bash
cargo build --release
./target/release/psf-guard server database.db images/
# No separate web server needed - everything is embedded!
```

**Development**:
For frontend development with hot reload:
```bash
# Terminal 1: Start frontend dev server
cd static
npm run dev

# Terminal 2: Start backend with filesystem serving
cargo run -- server database.db images/ --static-dir ./static/dist
```

#### API Integration Examples

```bash
# Get all projects
curl http://localhost:3000/api/projects

# Get images for a specific target
curl "http://localhost:3000/api/images?target_id=5&status=pending&limit=10"

# Accept an image
curl -X PUT http://localhost:3000/api/images/123/grade \
  -H "Content-Type: application/json" \
  -d '{"status": "accepted"}'

# Get star detection data
curl http://localhost:3000/api/images/123/stars

# Download annotated image
wget http://localhost:3000/api/images/123/annotated?size=large -O annotated.png
```

### Statistical Grading

Beyond the database grading status, PSF Guard can perform statistical analysis to identify additional outliers:

- **HFR Analysis**: Detects images with Half Flux Radius (focus quality) significantly different from the target's distribution
- **Star Count Analysis**: Identifies images with abnormal star detection counts per target
- **Distribution Analysis**: Uses Median Absolute Deviation (MAD) for skewed distributions where median differs significantly from mean
- **Cloud Detection**: Monitors sequences for sudden rises in HFR or drops in star count that indicate clouds, then waits for stable baseline before accepting images again

Statistical grading analyzes all images per target and filter combination to establish baselines, then identifies outliers that may have been missed by the initial grading process. The analysis is target-specific to account for different imaging conditions across the sky.

For detailed information about statistical grading features, algorithms, and best practices, see [STATISTICAL_GRADING.md](STATISTICAL_GRADING.md).

## Database Schema

The utility expects a SQLite database with the following key tables:
- `project`: Contains project information
- `target`: Contains observation targets
- `acquiredimage`: Contains image metadata and grading status

Grading status values:
- 0 = Pending
- 1 = Accepted
- 2 = Rejected

## Command Reference

### Global Options
- `-d, --database <DATABASE>`: Target Scheduler database file (default: schedulerdb.sqlite)
  - Only used by commands that require database access: `list-projects`, `list-targets`, `dump-grading`, `show-images`, `update-grade`
  - Standalone FITS analysis commands do not use this option

### Commands

#### dump-grading
Dump grading results for all images

Options:
- `-s, --status <STATUS>`: Filter by grading status (pending, accepted, rejected)
- `-p, --project <PROJECT>`: Filter by project name (partial match)
- `-t, --target <TARGET>`: Filter by target name (partial match)
- `-f, --format <FORMAT>`: Output format (table, json, csv) [default: table]

#### list-projects
List all projects in the database

#### list-targets
List all targets for a specific project

Arguments:
- `<PROJECT>`: Project ID or name

#### filter-rejected
Filter rejected files and move them to LIGHT_REJECT folders

Arguments:
- `<DATABASE>`: Database file to use
- `<BASE_DIR>`: Base directory containing the image files

Options:
- `--dry-run`: Perform a dry run (show what would be moved without actually moving)
- `-p, --project <PROJECT>`: Filter by project name
- `-t, --target <TARGET>`: Filter by target name
- `--enable-statistical`: Enable statistical analysis for additional rejections
- `--stat-hfr`: Enable HFR outlier detection
- `--hfr-stddev <STDDEV>`: Standard deviations for HFR outlier detection (default: 2.0)
- `--stat-stars`: Enable star count outlier detection  
- `--star-stddev <STDDEV>`: Standard deviations for star count outlier detection (default: 2.0)
- `--stat-distribution`: Enable distribution analysis (median/mean shift detection)
- `--median-shift-threshold <THRESHOLD>`: Percentage threshold for median shift from mean (default: 0.1)
- `--stat-clouds`: Enable cloud detection (sudden rises in HFR or drops in star count)
- `--cloud-threshold <THRESHOLD>`: Percentage threshold for cloud detection (default: 0.2 = 20% change)
- `--cloud-baseline-count <COUNT>`: Number of images needed to establish baseline after cloud event (default: 5)

#### read-fits
Read and display metadata from FITS files

Arguments:
- `<PATH>`: Path to FITS file or directory containing FITS files

Options:
- `-v, --verbose`: Show verbose output with all headers
- `-f, --format <FORMAT>`: Output format (table, json, csv) [default: table]

#### analyze-fits
Analyze FITS file with star detection and comparison

Arguments:
- `<PATH>`: Path to FITS file or directory

Options:
- `-p, --project <PROJECT>`: Filter by project name
- `-t, --target <TARGET>`: Filter by target name
- `-f, --format <FORMAT>`: Output format (table, json, csv) [default: table]
- `--detector <DETECTOR>`: Star detector to use (nina, hocusfocus) [default: hocusfocus]
- `--sensitivity <SENSITIVITY>`: Detection sensitivity (normal, high, highest) [default: normal]
- `--apply-stretch`: Apply MTF stretch before detection
- `--compare-all`: Compare all detector configurations
- `--psf-type <TYPE>`: PSF model (none, gaussian, moffat) [default: none]
- `-v, --verbose`: Show verbose output

#### stretch-to-png
Convert FITS file to PNG with stretching

Arguments:
- `<FITS_PATH>`: Path to FITS file

Options:
- `-o, --output <OUTPUT>`: Output PNG file path
- `--midtone <FACTOR>`: Midtone transfer function factor [default: 0.5]
- `--shadow <CLIPPING>`: Shadow clipping value [default: 0.001]
- `--logarithmic`: Use logarithmic stretch instead of MTF
- `--invert`: Invert the image (white stars on black background)

#### annotate-stars
Create annotated PNG image showing detected stars

Arguments:
- `<FITS_PATH>`: Path to FITS file

Options:
- `-o, --output <OUTPUT>`: Output PNG file path
- `--max-stars <N>`: Maximum number of stars to annotate [default: 50]
- `--detector <DETECTOR>`: Star detector (nina, hocusfocus) [default: hocusfocus]
- `--sensitivity <SENSITIVITY>`: Detection sensitivity [default: normal]
- `--midtone <FACTOR>`: Midtone factor for stretching [default: 0.5]
- `--shadow <CLIPPING>`: Shadow clipping [default: 0.001]
- `--color <COLOR>`: Annotation color (red, green, blue, yellow, cyan, magenta, white) [default: red]
- `--psf-type <TYPE>`: PSF model for HocusFocus [default: none]
- `-v, --verbose`: Show verbose output

#### visualize-psf
Visualize PSF fitting residuals for a single star

Arguments:
- `<FITS_PATH>`: Path to FITS file

Options:
- `-o, --output <OUTPUT>`: Output PNG file path
- `--star-index <INDEX>`: Index of star to visualize
- `--psf-type <TYPE>`: PSF model (gaussian, moffat) [default: moffat]
- `--max-stars <N>`: Number of stars to show [default: 1]
- `--selection <MODE>`: Selection mode (top, regions, quality, corners) [default: top]
- `--sort-by <METRIC>`: Sort metric (hfr, r2, brightness) [default: r2]
- `-v, --verbose`: Show verbose output

#### visualize-psf-multi
Visualize PSF fitting residuals for multiple stars in a grid

Arguments:
- `<FITS_PATH>`: Path to FITS file

Options:
- `-o, --output <OUTPUT>`: Output PNG file path
- `--num-stars <N>`: Number of stars to visualize [default: 9]
- `--psf-type <TYPE>`: PSF model (gaussian, moffat) [default: moffat]
- `--sort-by <METRIC>`: Sort metric (hfr, r2, brightness) [default: r2]
- `--grid-cols <N>`: Number of grid columns (0 for auto) [default: 0]
- `--selection <MODE>`: Selection mode (top, regions, quality, corners) [default: top]
- `-v, --verbose`: Show verbose output

#### benchmark-psf
Benchmark PSF fitting performance

Arguments:
- `<FITS_PATH>`: Path to FITS file

Options:
- `--runs <N>`: Number of benchmark runs [default: 3]
- `-v, --verbose`: Show detailed analysis

#### server
Start the web server with embedded React frontend

Arguments:
- `<DATABASE>`: Database file to use
- `<IMAGE_DIR>`: Base directory containing the image files

Options:
- `-p, --port <PORT>`: Port to listen on [default: 3000]
- `--host <HOST>`: Host to bind to [default: 127.0.0.1]
- `--static-dir <DIR>`: Directory to serve static files from (optional - uses embedded files if not provided)
- `--cache-dir <DIR>`: Cache directory for processed images [default: ./cache]

#### regrade
Regrade images in the database based on statistical analysis

Arguments:
- `<DATABASE>`: Database file to use

Options:
- `--dry-run`: Perform a dry run (show what would be changed without actually updating)
- `-p, --project <PROJECT>`: Filter by project name
- `-t, --target <TARGET>`: Filter by target name
- `--days <DAYS>`: Number of days to look back (default: 90)
- `--reset <MODE>`: Reset mode: none, automatic, or all (default: none)
  - `none`: Do not reset any existing grades
  - `automatic`: Reset only automatically graded images (preserves manual grades)
  - `all`: Reset all images to pending status
- Statistical analysis options (same as filter-rejected command)

## Examples

### Web Server Examples

```bash
# Start web server on default port (3000)
psf-guard server schedulerdb.sqlite /path/to/images/

# Start on custom port and allow external access
psf-guard server schedulerdb.sqlite /path/to/images/ --port 8080 --host 0.0.0.0

# Development mode with filesystem serving
psf-guard server schedulerdb.sqlite /path/to/images/ --static-dir ./static/dist

# Custom cache directory
psf-guard server schedulerdb.sqlite /path/to/images/ --cache-dir /tmp/psf-cache
```

### API Usage Examples

```bash
# List all projects
curl http://localhost:3000/api/projects | jq '.data'

# Get images for project 2, target 5, only rejected ones
curl "http://localhost:3000/api/images?project_id=2&target_id=5&status=rejected" | jq '.data'

# Get detailed info for image 123
curl http://localhost:3000/api/images/123 | jq '.data'

# Accept image 123
curl -X PUT http://localhost:3000/api/images/123/grade \
  -H "Content-Type: application/json" \
  -d '{"status": "accepted", "reason": "Good focus and tracking"}'

# Reject image 456 with reason
curl -X PUT http://localhost:3000/api/images/456/grade \
  -H "Content-Type: application/json" \
  -d '{"status": "rejected", "reason": "Poor focus"}'

# Download stretched preview
wget "http://localhost:3000/api/images/123/preview?size=large&midtone=0.3" -O preview.png

# Download star-annotated image
wget "http://localhost:3000/api/images/123/annotated?size=screen" -O stars.png

# Download PSF visualization
wget "http://localhost:3000/api/images/123/psf?num_stars=25&psf_type=moffat" -O psf.png

# Get star detection results
curl http://localhost:3000/api/images/123/stars | jq '.data.stars[0:5]'
```

### CLI Examples

```bash
# Check what rejected files exist for a project
psf-guard dump-grading --status rejected --project "Double Dragon" --format csv > rejected_files.csv

# Preview file moves for a specific project
psf-guard filter-rejected mydb.sqlite ./images --dry-run --project "Cygnus Wall"

# Move all rejected files for a target
psf-guard filter-rejected mydb.sqlite ./images --target "LDN 1228"

# Get JSON output for integration with other tools
psf-guard dump-grading --status accepted --format json | jq '.[] | select(.filter_name == "HA")'

# Use statistical grading to find outliers beyond database rejections
psf-guard filter-rejected mydb.sqlite ./images --dry-run --enable-statistical --stat-hfr --stat-stars

# Fine-tune statistical thresholds
psf-guard filter-rejected mydb.sqlite ./images --dry-run --enable-statistical --stat-hfr --hfr-stddev 1.5 --stat-distribution --median-shift-threshold 0.15

# Enable cloud detection with custom thresholds
psf-guard filter-rejected mydb.sqlite ./images --dry-run --enable-statistical --stat-clouds --cloud-threshold 0.15 --cloud-baseline-count 3

# Regrade images in database (last 30 days)
psf-guard regrade mydb.sqlite --dry-run --days 30 --enable-statistical --stat-hfr --stat-stars

# Reset automatic grades and reapply statistical analysis
psf-guard regrade mydb.sqlite --dry-run --reset automatic --enable-statistical --stat-hfr --stat-stars --stat-clouds

# Reset all grades for a specific target
psf-guard regrade mydb.sqlite --dry-run --reset all --target "M31" --days 7

# Analyze FITS file metadata
psf-guard read-fits "image.fits"

# Check all FITS files in a directory
psf-guard read-fits "/path/to/fits/files/"

# Show all header keywords for debugging
psf-guard read-fits --verbose "image.fits"

# Export FITS metadata to JSON or CSV for analysis
psf-guard read-fits --format json "/path/to/fits/files/" > metadata.json
psf-guard read-fits --format csv "/path/to/fits/files/" > metadata.csv

# Analyze FITS file with star detection
psf-guard analyze-fits "image.fits"

# Compare all star detectors
psf-guard analyze-fits "image.fits" --compare-all

# Use specific detector with PSF fitting
psf-guard analyze-fits "image.fits" --detector hocusfocus --psf-type moffat

# Convert FITS to PNG with custom stretch
psf-guard stretch-to-png "image.fits" --midtone 0.3 --shadow 0.002

# Create annotated star map
psf-guard annotate-stars "image.fits" --max-stars 100 --color yellow

# Visualize PSF residuals for multiple stars
psf-guard visualize-psf-multi "image.fits" --num-stars 25 --psf-type gaussian

# Show corner stars (9-point grid)
psf-guard visualize-psf-multi "image.fits" --selection corners

# Benchmark PSF fitting performance
psf-guard benchmark-psf "image.fits" --runs 10 --verbose
```

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
