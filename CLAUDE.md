# CLAUDE.md - Development Notes

## Project Overview

PSF Guard is a Rust CLI utility for analyzing N.I.N.A. Target Scheduler databases and managing astronomical image files. It includes a complete implementation of N.I.N.A.'s star detection algorithm, PSF fitting capabilities, and a React-based web interface for image grading.

## Quick Start

```bash
# Development setup
cargo fmt && cargo clippy && cargo test
cargo check --features opencv

# Run server (single directory)
cargo run -- server db.sqlite images/

# Run server (multiple directories with priority order)
cargo run -- server db.sqlite images1/ images2/ images3/

# Build for production (includes embedded frontend)
cargo build --release
```

## Architecture

### Core Components
- **CLI**: Command-pattern with clap-derive
- **Database**: SQLite via rusqlite with parameterized queries
- **Star Detection**: N.I.N.A. algorithm port + HocusFocus detector
- **Web Server**: Axum + embedded React frontend
- **File Cache**: Directory tree with O(1) lookups

### Key Algorithms

#### Star Detection Pipeline
1. Load FITS → Calculate statistics → Apply MTF stretch
2. Convert to 8-bit → Noise reduction → Resize for processing
3. Edge detection (Canny/NoBlur) → SIS threshold → Binary dilation
4. Blob detection → Circle analysis → Calculate HFR on original data

#### Statistical Grading
- Groups images by (target_id, filter_name)
- Z-score for normal distributions
- MAD for skewed distributions  
- Cloud detection with rolling baseline
- Sequence analysis for temporal patterns

## Recent Updates

### Multi-Directory Support (2025-08-31)
Added support for scanning multiple image directories with first-hit preference:
- CLI now accepts multiple directory arguments: `server db.sqlite dir1/ dir2/ dir3/`
- Directory tree cache scans all directories in priority order
- File lookup uses first-hit preference - files in earlier directories take precedence
- Maintains backward compatibility with single directory usage
- All directories validated at startup, cached together for O(1) lookups

### Image Comparison Zoom Fix (2025-08-31)
Fixed issue where zoomed images in comparison view would reset when switching images:
- Right image now always matches left image's resolution choice (original vs large)
- Removed zoom-based decision making for right image
- Added dedicated effect to sync resolution states
- Maintains zoom continuity when switching images at high zoom levels

### Directory Tree Caching (2025-08-31)
- Replaced recursive file finding with in-memory cache
- Single scan at startup, O(1) filename lookups
- 5-minute TTL with automatic refresh
- Integrated with project/target cache refresh

### Web UI Enhancements (2025-08-30)
- Smart dynamic image loading (large → original, one-way)
- Visual scale always represents actual size (100% = original)
- Non-blocking server startup with background cache refresh
- Comprehensive cache key improvements to prevent collisions

## Database Schema

Key tables and fields:
```sql
project (1:many) → target (1:many) → acquiredimage

acquiredimage:
- gradingStatus: 0=Pending, 1=Accepted, 2=Rejected
- metadata: JSON with FileName and imaging parameters
- rejectreason: Human-readable rejection reason
```

Column naming is inconsistent - use exact names:
- `Id`, `projectId`, `targetId` (not snake_case)
- `acquireddate`, `filtername` (not camelCase)

## Web Server

### API Endpoints
```
GET  /api/projects
GET  /api/projects/{id}/targets
GET  /api/images?project_id=X&target_id=Y
PUT  /api/images/{id}/grade
GET  /api/images/{id}/preview?size=screen|large|original
GET  /api/images/{id}/annotated
GET  /api/images/{id}/psf
GET  /api/images/{id}/stars
```

### Frontend Architecture
- React 18 + TypeScript + Vite
- TanStack Query for server state
- Custom hooks: useImageZoom, useGrading
- Smart image loading with state machine
- Embedded in binary for single-file deployment

### Key Features
- File existence checking with visual indicators
- Batch operations with multi-selection
- Undo/redo system (Ctrl+Z/Y)
- Side-by-side comparison with independent zoom
- Keyboard shortcuts throughout

## Star Detection Implementation

### N.I.N.A. Algorithm
Key discoveries from porting the C# code:
1. **MTF Stretching**: Applied before detection, original data for HFR
2. **MAD Calculation**: Histogram-based, not simple sorting
3. **Banker's Rounding**: .NET's "round half to even" strategy
4. **Edge Detection**: Normal uses blur, High/Highest uses NoBlur

### OpenCV Integration
- Optional via `--features opencv`
- Automatic fallback to pure Rust
- Enhances edge detection and morphology
- Better contour analysis

## PSF Fitting

- Gaussian and Moffat (β=4.0) models
- Levenberg-Marquardt optimization
- Sub-pixel bilinear interpolation
- R² and RMSE metrics
- FWHM and eccentricity calculations

## Performance Optimizations

- Directory tree caching eliminates recursive searches
- Image preview caching with comprehensive keys
- Non-blocking server startup
- Lazy loading and virtualization in frontend
- Batch database operations

## Development Workflow

### Essential Commands
```bash
# Before committing
cargo fmt && cargo clippy && cargo test

# OpenCV setup (macOS)
brew install opencv
export DYLD_FALLBACK_LIBRARY_PATH="$(xcode-select --print-path)/Toolchains/XcodeDefault.xctoolchain/usr/lib/"

# Run with logging
RUST_LOG=debug cargo run -- server db.sqlite images/

# Frontend development
cd static && npm run dev
```

### Logging
- `RUST_LOG=error|warn|info|debug|trace`
- Emoji prefixes for visual categorization
- Structured timing and metrics
- Clean output without module paths

## Known Issues

1. **Path Separators**: Mixed Windows/Unix paths may cause issues
2. **Large Metadata**: Very large JSON could cause memory issues
3. **Timezone Handling**: Dates stored as Unix timestamps

## Future Improvements

1. **Parallel Processing**: File operations
2. **Progress Bars**: Long operations
3. **Machine Learning**: Train on accepted/rejected images
4. **Real-time Monitoring**: Watch mode for live sessions
5. **Configuration File**: .psfguardrc support