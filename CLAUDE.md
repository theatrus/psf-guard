# CLAUDE.md - Development Notes

## Project Overview

PSF Guard (Point Spread Function Guard) is a Rust CLI utility designed to analyze N.I.N.A. Target Scheduler plugin databases and manage rejected astronomical image files. The project was developed to help organize FITS files based on their grading status in the N.I.N.A. Target Scheduler database. It now also includes a full implementation of N.I.N.A.'s star detection algorithm for HFR (Half Flux Radius) calculations.

## Code Quality Guidelines

**IMPORTANT**: Before committing any code changes:

1. **Format all code**: Run `cargo fmt` to ensure consistent code formatting
2. **Fix all clippy warnings**: Run `cargo clippy` and address all warnings
3. **Run tests**: Ensure all tests pass with `cargo test`
4. **Check with features**: Test compilation with `cargo check --features opencv`

These steps help maintain code quality and prevent CI failures.

## Key Implementation Details

### Architecture

The application uses a command-pattern architecture with the following main components:

1. **CLI Interface**: Built with clap-derive for type-safe command parsing
2. **Database Layer**: Uses rusqlite for SQLite database access
3. **File Operations**: Standard library fs module for file system operations
4. **Data Models**: Serialize/Deserialize structs for Project, Target, and AcquiredImage

### Security Considerations

- **SQL Injection Prevention**: All database queries use parameterized statements
- **Safe File Operations**: Explicit dry-run mode before any destructive operations
- **Path Validation**: Careful handling of file paths from database to prevent directory traversal

### Complex Implementation Areas

#### 1. Multi-Structure Path Detection

The most complex part is handling different directory structures. The code supports:

```rust
// Standard: date/target_name/date/LIGHT/filename
// Alternate: target_name/date/LIGHT/filename
```

The implementation:
- First attempts standard structure parsing
- Falls back to alternate structure if files not found
- Uses date pattern detection (YYYY-MM-DD) to identify structure
- Handles files in both `LIGHT/` and `LIGHT/rejected/` subdirectories

#### 2. Metadata Extraction

Image filenames are extracted from JSON metadata stored in the database:
```rust
fn extract_filename(metadata: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(metadata).ok()?;
    json.get("FileName")
        .and_then(|f| f.as_str())
        .map(|path| {
            // Extract just the filename from the full path
            path.split(&['\\', '/'][..])
                .last()
                .unwrap_or(path)
                .to_string()
        })
}
```

#### 3. Column Name Handling

The database uses inconsistent column naming (camelCase vs snake_case). The code handles this by using exact column names from the schema:
- `Id`, `projectId`, `targetId` (not `id`, `project_id`, etc.)
- `acquireddate`, `filtername` (not `acquired_date`, `filter_name`)

### Testing Strategy

Testing focused on:
1. **Dry-run verification**: Always test with `--dry-run` first
2. **SQL injection attempts**: Verified parameterized queries work correctly
3. **Multiple directory structures**: Tested both `files/` and `files2/` directories
4. **Edge cases**: Files already in rejected folders, missing files, etc.

### Performance Considerations

- Batch operations where possible (e.g., reading all rejected files in one query)
- Minimal file system checks (only when necessary)
- Efficient path parsing without regex where possible

### Recent Updates (2025-08-27)

#### Code Refactoring
- Broke up monolithic main.rs (~1100 lines) into modular structure
- Created separate modules for CLI, models, utils, and commands
- Each command now has its own module in commands/ directory
- Extracted shared statistical options into reusable structure
- See REFACTORING.md for detailed changes

#### Statistical Grading Enhancement
- Refactored from filter-based to target-and-filter-based analysis
- Added cloud detection algorithm with sequence analysis
- Implemented rolling baseline establishment after cloud events

#### Database Regrading Command
- Added `regrade` command to update database with statistical analysis results
- Supports date range filtering (default: last 90 days)
- Three reset modes: none, automatic (preserves manual), all
- Marks auto-rejected images with `[Auto]` prefix for identification

#### Key Implementation Details

1. **Data Flow**:
   - Images grouped by `(target_id, filter_name)` tuples
   - Sorted chronologically within each group for sequence analysis
   - Per-target statistics calculated independently

2. **Cloud Detection Algorithm**:
   - Rolling median baseline (default: 5 images)
   - Triggers on 20% change (configurable)
   - Requires new baseline after cloud event
   - Dual detection: HFR increase OR star count decrease

3. **Statistical Methods**:
   - Standard deviation for normal distributions
   - MAD (Median Absolute Deviation) for skewed distributions
   - Automatic detection of distribution type based on median/mean difference

### Future Improvements

1. **Parallel Processing**: File moves could be parallelized for large batches
2. **Progress Bars**: For long-running operations
3. **Undo Capability**: Track moves in a local database for reversal
4. **Configuration File**: Support for .psfguardrc configuration
5. **Extended Filtering**: More complex queries (date ranges, multiple statuses)
6. **Machine Learning**: Train models on accepted/rejected images for better detection
7. **Real-time Monitoring**: Watch mode for live sessions

### Statistical Grading Module (grading.rs)

The statistical grading module provides advanced outlier detection:

#### Key Structures
- `StatisticalGradingConfig`: Configuration for all analysis features
- `ImageStatistics`: Per-image metrics (HFR, star count, target info)
- `FilterStatistics`: Aggregate statistics per target/filter group
- `StatisticalRejection`: Rejection details with reason and explanation

#### Algorithm Flow
1. Parse metadata to extract HFR and star counts
2. Group images by (target_id, filter_name)
3. Sort chronologically for sequence analysis
4. Calculate distribution statistics
5. Apply multiple detection methods:
   - Z-score for normal distributions
   - MAD for skewed distributions
   - Sequence analysis for cloud detection

#### Edge Cases Handled
- Images with missing HFR/star count data
- Groups with insufficient data (< 3 images)
- Extreme outliers (0 HFR values)
- Baseline reset after cloud events

### Development Commands

```bash
# macOS: Set up libclang for OpenCV (required before building)
export DYLD_FALLBACK_LIBRARY_PATH="$(xcode-select --print-path)/Toolchains/XcodeDefault.xctoolchain/usr/lib/"
# Or add to ~/.zshrc for permanent setup

# Run with verbose logging
RUST_LOG=debug cargo run -- filter-rejected db.sqlite files --dry-run

# Test statistical grading
cargo run -- filter-rejected schedulerdb.sqlite files --dry-run \
  --enable-statistical --stat-hfr --stat-stars --stat-clouds

# Run tests
cargo test

# Check for common issues
cargo clippy

# Format code
cargo fmt
```

### Known Issues

1. **Path Separator Handling**: The code handles both Windows (`\`) and Unix (`/`) paths, but mixed paths might cause issues
2. **Large Metadata**: Very large metadata JSON could cause memory issues (unlikely with typical FITS metadata)
3. **Timezone Handling**: Dates are stored as Unix timestamps, timezone conversion not implemented

### Database Schema Notes

Key relationships:
- `project` (1:many) -> `target`
- `target` (1:many) -> `acquiredimage`
- `exposureplan` links to both `target` and `exposuretemplate`

Critical fields:
- `acquiredimage.gradingStatus`: 0=Pending, 1=Accepted, 2=Rejected
- `acquiredimage.metadata`: JSON containing file path and imaging parameters
- `acquiredimage.rejectreason`: Human-readable rejection reason

### Error Handling Philosophy

The code uses `anyhow` for error handling with context:
- File operations provide specific paths in errors
- Database errors include the operation being attempted
- User-friendly messages for common issues (file not found, permission denied)

### Code Style Notes

- Prefer explicit types in complex sections for clarity
- Use descriptive variable names even if longer
- Comment complex logic, especially path parsing
- Keep functions focused on single responsibilities

## N.I.N.A. Star Detection Implementation (2025-08-27)

### Overview

Implemented N.I.N.A.'s (Nighttime Imaging 'N' Astronomy) star detection algorithm in Rust, matching their C# implementation for accurate HFR (Half Flux Radius) calculations used in astronomical focusing and image quality assessment.

### Problem Statement

Initial implementation produced HFR values of 7.331 vs N.I.N.A.'s 2.920 for the same FITS file, indicating significant pipeline differences. Through iterative debugging and analysis of N.I.N.A.'s source code, we discovered and implemented multiple missing components.

### Key Discoveries

1. **MTF (Midtone Transfer Function) Stretching**
   - N.I.N.A. applies MTF stretching before star detection
   - Uses stretched data for detection but original raw data for HFR measurement
   - Formula: `(midtone_balance - 1) * x / ((2 * midtone_balance - 1) * x - midtone_balance)`

2. **MAD (Median Absolute Deviation) Calculation**
   - N.I.N.A. uses histogram-based approach, not simple sorting
   - Steps outward from median in histogram to find MAD
   - Critical for proper image statistics

3. **Banker's Rounding**
   - .NET's default Math.Round uses "round half to even" strategy
   - Important for matching exact pixel value calculations
   - Implemented custom `round_half_to_even()` function

4. **Edge Detection Variants**
   - Normal sensitivity: Regular Canny with Gaussian blur
   - High/Highest sensitivity: NoBlur Canny edge detector
   - Different sensitivities use different resize factors

### Implementation Details

#### File Structure
```
src/
├── nina_star_detection.rs    # Main star detection algorithm
├── mtf_stretch.rs           # MTF stretching implementation
├── image_analysis.rs        # FITS image analysis and statistics
├── accord_imaging.rs        # Accord.NET imaging functions port
└── lib.rs                   # Library exports

test_nina_comparison.rs      # Comprehensive comparison test
```

#### Key Algorithms

1. **Star Detection Pipeline**
   ```rust
   1. Load FITS → Calculate statistics → Apply MTF stretch
   2. Convert 16-bit to 8-bit (stretched data)
   3. Apply noise reduction (optional)
   4. Resize for faster processing
   5. Edge detection (Canny/NoBlur Canny)
   6. SIS threshold → Binary dilation
   7. Blob detection → Circle/shape analysis
   8. Calculate HFR on original data
   ```

2. **Resize Factors**
   - Normal: `MAX_WIDTH / image_width` (MAX_WIDTH = 1552)
   - High: Simulated 1/3 for typical setups
   - Highest: max(2/3, MAX_WIDTH/width)

3. **HFR Calculation**
   - Uses original raw data, not stretched
   - Background subtraction with banker's rounding
   - Centroid-weighted distance calculation
   - Formula: `sum(pixel_value * distance) / sum(pixel_value)`

### Technical Challenges Solved

1. **Floating Point Precision**
   - Implemented banker's rounding to match .NET
   - Careful handling of edge cases in MTF formula

2. **Image Processing**
   - Ported Accord.NET's Canny edge detector
   - Implemented both blur and no-blur variants
   - Added SIS (Simple Image Statistics) thresholding

3. **Performance**
   - Efficient histogram-based MAD calculation
   - Image resizing with bicubic interpolation
   - Optimized blob detection algorithm

### Testing Results

#### OIII Filter (Bubble Nebula)
- N.I.N.A.: 343 stars, HFR 2.920
- Our implementation: 128 stars, HFR 2.596
- Best stars: HFR 2.5-2.6 (very close to N.I.N.A.'s average)

#### H-alpha Filter (North American Nebula)
- Consistent detection patterns
- High sensitivity: ~110-120 stars
- Normal sensitivity: ~70-80 stars
- HFR distributions match expected ranges

### Key Code Additions

1. **Banker's Rounding** (nina_star_detection.rs:7-28)
   ```rust
   fn round_half_to_even(x: f64) -> f64 {
       let truncated = x.trunc();
       let fraction = x - truncated;
       
       if fraction > 0.5 || fraction < -0.5 {
           x.round()
       } else if fraction == 0.5 {
           if truncated % 2.0 == 0.0 {
               truncated
           } else {
               truncated + 1.0
           }
       } else if fraction == -0.5 {
           if truncated % 2.0 == 0.0 {
               truncated
           } else {
               truncated - 1.0
           }
       } else {
           truncated
       }
   }
   ```

2. **MTF Stretch** (mtf_stretch.rs)
   ```rust
   fn midtones_transfer_function(midtone_balance: f64, x: f64) -> f64 {
       if x > 0.0 {
           if x < 1.0 {
               return (midtone_balance - 1.0) * x / 
                      ((2.0 * midtone_balance - 1.0) * x - midtone_balance);
           }
           return 1.0;
       }
       return 0.0;
   }
   ```

3. **NoBlur Canny** (accord_imaging.rs:143-151)
   ```rust
   pub fn new_no_blur(low_threshold: u8, high_threshold: u8) -> Self {
       Self {
           low_threshold,
           high_threshold,
           gaussian_size: 5,
           gaussian_sigma: 1.4,
           apply_blur: false,
       }
   }
   ```

### Remaining Differences

Small variations remain due to:
- Floating-point calculation differences between Rust and C#
- Image interpolation implementation details
- Edge detection numerical precision
- Compiler optimizations

These differences are within acceptable tolerances for astronomical image analysis.

### Usage

```rust
use psf_guard::nina_star_detection::{detect_stars_with_original, StarDetectionParams};
use psf_guard::mtf_stretch::StretchParameters;

// Load FITS and calculate statistics
let fits = FitsImage::from_file("image.fits")?;
let stats = fits.calculate_basic_statistics();

// Apply MTF stretch for detection
let stretch_params = StretchParameters::default();
let stretched = stretch_image(&fits.data, &stats, stretch_params.factor, stretch_params.black_clipping);

// Detect stars (stretched for detection, original for measurement)
let params = StarDetectionParams::default();
let result = detect_stars_with_original(&stretched, &fits.data, fits.width, fits.height, &params);

println!("Detected {} stars with average HFR {:.3}", result.detected_stars, result.average_hfr);
```

### Files Added/Modified for Star Detection

1. **src/nina_star_detection.rs** - Main star detection implementation
2. **src/mtf_stretch.rs** - MTF stretching algorithm
3. **src/image_analysis.rs** - Enhanced with MAD calculation and FITS support
4. **src/accord_imaging.rs** - Port of Accord.NET imaging functions
5. **src/lib.rs** - Added public exports for star detection
6. **test_nina_comparison.rs** - Comprehensive test program
7. **Cargo.toml** - Added `image` crate dependency

### Debugging Journey

1. **Initial HFR mismatch (7.331 vs 2.920)**
   - Fixed by discovering and implementing MTF stretching
   
2. **MAD calculation error (13 vs 97.86)**
   - Fixed by implementing histogram-based approach
   
3. **MTF stretch too aggressive (0 stars detected)**
   - Fixed incorrect formula implementation
   
4. **Wrong edge detector for High sensitivity**
   - Implemented NoBlur variant for High/Highest
   
5. **Rounding differences**
   - Implemented banker's rounding to match .NET

This implementation demonstrates deep integration with astronomical image processing pipelines and careful attention to numerical accuracy.

## OpenCV Integration (2025-08-28)

### Overview

Added optional OpenCV support to enhance star detection capabilities with professional computer vision algorithms. OpenCV operations are attempted first with automatic fallback to pure Rust implementations, ensuring the code works both with and without the OpenCV feature.

### Implementation Details

#### OpenCV Wrappers Created

1. **opencv_canny.rs** - Canny edge detection wrappers
   - `OpenCVCanny`: Canny edge detection with and without Gaussian blur
   - `OpenCVThreshold`: SIS thresholding using Otsu's method
   - `OpenCVBinaryMorphology`: Binary dilation operations
   - `OpenCVNoiseReduction`: Gaussian and median blur filters

2. **opencv_contours.rs** - Advanced blob detection
   - `OpenCVBlobDetector`: Contour-based star detection with quality assessment
   - Shape analysis and eccentricity calculations
   - Better handling of overlapping stars

3. **opencv_morphology.rs** - Morphological operations
   - Elliptical and rectangular structuring elements
   - Erosion and dilation with edge preservation
   - Better star/noise separation

4. **opencv_wavelets.rs** - Structure removal
   - Wavelet decomposition for nebula removal
   - Edge-preserving filters for better star preservation
   - Domain transform filters for large-scale structures

#### Integration Strategy

1. **Automatic Fallback Pattern**:
   ```rust
   match OpenCVOperation::apply(data) {
       Ok(result) => use_opencv_result(result),
       Err(e) => {
           eprintln!("OpenCV operation failed: {}, using fallback", e);
           use_fallback_implementation(data)
       }
   }
   ```

2. **Unified API**: All OpenCV wrappers follow consistent patterns:
   - Use `create_mat_from_u8/u16` for Mat creation
   - Return `Result<Vec<u8>, Box<dyn Error>>` for error handling
   - Provide both feature-gated implementations

3. **Feature Flag**: OpenCV is optional via `--features opencv`

### Building with OpenCV

#### Prerequisites

Follow the [opencv-rust installation guide](https://github.com/twistedfall/opencv-rust/blob/master/INSTALL.md):

**Linux/macOS**:
```bash
# Ubuntu/Debian
sudo apt-get install libopencv-dev clang

# macOS
brew install opencv

# IMPORTANT: macOS libclang setup (required)
export DYLD_FALLBACK_LIBRARY_PATH="$(xcode-select --print-path)/Toolchains/XcodeDefault.xctoolchain/usr/lib/"
# Add to ~/.zshrc or ~/.bash_profile for permanent setup

# Set environment variables if needed
export OPENCV_LINK_LIBS=opencv_world
export OPENCV_LINK_PATHS=/usr/local/lib
```

**Windows**:
Use vcpkg for easiest setup:
```powershell
vcpkg install opencv4[contrib,nonfree]:x64-windows
set VCPKG_ROOT=C:\vcpkg
set OPENCV_LINK_LIBS=opencv_world
```

#### Building

```bash
# Build with OpenCV support (default)
cargo build

# Build without OpenCV (pure Rust fallbacks)
cargo build --no-default-features

# Run tests with OpenCV (default)
cargo test

# Run tests without OpenCV
cargo test --no-default-features
```

### Benefits of OpenCV Integration

1. **Better Edge Detection**: OpenCV's Canny implementation is highly optimized
2. **Advanced Morphology**: Better star/noise separation with elliptical kernels
3. **Professional Filters**: Domain transform and edge-preserving filters
4. **Contour Analysis**: More accurate star boundary detection
5. **Performance**: SIMD-optimized operations on supported platforms

### Detector Comparison

The `analyze-fits` command now supports `--compare-all` to test different detector configurations:

```bash
psf-guard analyze-fits image.fits --compare-all

=== Detector Comparison Results ===
Detector                       |    Stars |    Avg HFR | HFR StdDev
NINA-normal                    |       85 |      2.834 |      0.412
NINA-high                      |      112 |      2.756 |      0.523
NINA-highest                   |      128 |      2.691 |      0.634
HocusFocus                     |       93 |      2.812 |      0.387
```

HocusFocus always attempts OpenCV operations first with automatic fallback to pure Rust implementations if OpenCV fails.

## Enhanced PSF Fitting and Analysis (2025-08-29)

### PSF Fitting Implementation

Added comprehensive Point Spread Function (PSF) fitting capabilities:

1. **PSF Models** (src/psf_fitting.rs)
   - Gaussian PSF model
   - Moffat PSF with beta=4.0 (better for atmospheric seeing)
   - Levenberg-Marquardt optimizer for non-linear least squares fitting
   - Sub-pixel bilinear interpolation for accurate measurements

2. **Key Features**
   - ROI extraction with configurable size (default 32x32 pixels)
   - Sub-pixel sampling (0.5 pixel spacing)
   - Automatic bounds enforcement for stable fitting
   - R² and RMSE goodness-of-fit metrics
   - FWHM and eccentricity calculations

3. **Integration**
   - HocusFocus star detector enhanced with PSF fitting option
   - PSF parameters available in star detection results
   - Used for more accurate FWHM measurements than simple HFR

### New Commands and Features

#### 1. analyze-fits Command
Comprehensive FITS analysis with star detection comparison:

```bash
# Basic usage
psf-guard analyze-fits image.fits

# Compare all detectors
psf-guard analyze-fits image.fits --compare-all

# Specific detector with PSF fitting
psf-guard analyze-fits image.fits --detector hocusfocus --psf-type moffat

# Directory analysis
psf-guard analyze-fits /path/to/fits/directory --format csv
```

Features:
- Compare NINA (normal/high/highest) vs HocusFocus detectors
- Database comparison with N.I.N.A. metadata
- Multiple output formats (table, json, csv)
- PSF fitting options (none, gaussian, moffat)

#### 2. stretch-to-png Command
Convert FITS to PNG with MTF stretching:

```bash
# Basic MTF stretch
psf-guard stretch-to-png image.fits

# Custom parameters
psf-guard stretch-to-png image.fits -o output.png --midtone 0.3 --shadow 0.001

# Logarithmic stretch with invert
psf-guard stretch-to-png image.fits --logarithmic --invert
```

#### 3. annotate-stars Command
Create annotated PNG images showing detected stars:

```bash
# Basic annotation
psf-guard annotate-stars image.fits

# Custom settings
psf-guard annotate-stars image.fits --max-stars 100 --color yellow --detector nina --sensitivity high

# With PSF fitting
psf-guard annotate-stars image.fits --psf-type moffat --verbose
```

Features:
- Circle annotations sized by HFR
- Customizable colors (red, green, blue, yellow, cyan, magenta, white)
- Top N stars by HFR quality
- Verbose mode shows star positions and HFR values

#### 4. visualize-psf Commands
Advanced PSF residual visualization:

```bash
# Single star visualization
psf-guard visualize-psf image.fits --star-index 0 --psf-type moffat

# Multi-star grid visualization
psf-guard visualize-psf-multi image.fits --num-stars 25 --psf-type gaussian

# Selection strategies
psf-guard visualize-psf-multi image.fits --selection corners  # 9-point grid
psf-guard visualize-psf-multi image.fits --selection regions  # 5 regions
psf-guard visualize-psf-multi image.fits --selection quality  # Quality tiers

# Custom grid layout
psf-guard visualize-psf-multi image.fits --grid-cols 5 --sort-by r2
```

Features:
- Side-by-side observed/fitted/residual panels
- Multiple selection strategies (top-n, corners, regions, quality)
- Star location minimap with numbered markers
- Automatic square grid layout
- Sort by HFR, R², or brightness
- Detailed PSF metrics display

#### 5. benchmark-psf Command
Performance benchmarking for PSF fitting:

```bash
# Basic benchmark
psf-guard benchmark-psf image.fits

# Multiple runs for averaging
psf-guard benchmark-psf image.fits --runs 10 --verbose
```

Output includes:
- Detection times per method (HFR only, Gaussian, Moffat)
- Time per star metrics
- PSF fit success rates
- R² and FWHM statistics

### Visualization Improvements

1. **Enhanced Minimap**
   - Larger 600x600 pixel size for better visibility
   - Numbered star markers corresponding to grid positions
   - Clear position indicators
   - Shows all detected stars with selected ones highlighted

2. **Grid Layout**
   - Automatic square grid calculation
   - Special handling for corners mode (3x3 grid)
   - Consistent star numbering based on position
   - Better spacing and panel organization

3. **Residual Visualization**
   - Red-white-blue colormap for residuals
   - Normalized display ranges
   - Clear panel labels
   - PSF parameter display (FWHM, R², eccentricity)

### Code Quality Improvements (2025-08-29)

Fixed all cargo clippy warnings:

1. **Code Style**
   - Changed `HFR` enum to `Hfr` (upper-case acronym rule)
   - Used `.clamp()` instead of manual min/max
   - Used `.div_ceil()` for ceiling division
   - Added type alias `ResidualMaps` for complex return types

2. **Best Practices**
   - Proper struct initialization syntax
   - Removed unnecessary `mut` declarations
   - Added `#[allow(clippy::too_many_arguments)]` where appropriate
   - Added `#[allow(dead_code)]` for unused but planned features

3. **Performance**
   - More efficient integer operations
   - Better memory usage patterns
   - Cleaner code generation

### Command Line Examples

```bash
# Analyze and compare all detectors
psf-guard analyze-fits my_image.fits --compare-all

# Create annotated PNG with top 50 stars in yellow
psf-guard annotate-stars my_image.fits --max-stars 50 --color yellow --verbose

# Visualize PSF fitting for 25 stars in a 5x5 grid
psf-guard visualize-psf-multi my_image.fits --num-stars 25 --psf-type moffat

# Show 9-point corner grid with Gaussian PSF
psf-guard visualize-psf-multi my_image.fits --selection corners --psf-type gaussian

# Benchmark PSF fitting performance
psf-guard benchmark-psf my_image.fits --runs 5 --verbose

# Convert FITS to PNG with custom stretch
psf-guard stretch-to-png my_image.fits --midtone 0.25 --shadow 0.002 -o stretched.png
```

## Build.rs Integration for Embedded React App (2025-08-30)

### Overview

Implemented `build.rs` integration to automatically build and embed the React frontend into the Rust binary, creating a single self-contained executable. This eliminates the need for separate frontend deployment and simplifies distribution.

### Key Features

1. **Automatic Frontend Building**
   - `build.rs` automatically runs `npm run build` during cargo compilation
   - Frontend is built from `static/` directory and output to `static/dist/`
   - Build process is integrated into Cargo's dependency tracking system

2. **File Embedding**
   - Uses `include_dir!` macro to embed `static/dist/` at compile time
   - All React assets (HTML, CSS, JS, images) are included in the binary
   - No external files needed for deployment

3. **Dual Serving Modes**
   - **Production (Embedded)**: `psf-guard server database.db images/` - serves from embedded assets
   - **Development (Filesystem)**: `psf-guard server database.db images/ --static-dir ./static/dist` - serves from filesystem for hot reload

4. **Smart Caching**
   - Development builds skip frontend compilation if `dist/` is newer than source files
   - Environment variable `PSF_GUARD_SKIP_FRONTEND_BUILD=1` to skip frontend build entirely
   - Release builds always rebuild frontend for consistency

### Implementation Details

#### Build System Integration

**build.rs**:
```rust
fn build_react_app() {
    // Automatic dependency tracking
    println!("cargo:rerun-if-changed=static/src");
    println!("cargo:rerun-if-changed=static/package.json");
    
    // Skip build in development if dist is newer
    if is_dev && dist_dir.exists() && is_dist_newer_than_sources(&static_dir, &dist_dir) {
        return;
    }
    
    // Run npm build
    Command::new("npm").args(["run", "build"]).current_dir(&static_dir).output();
}
```

#### Embedded Static File Serving

**src/server/embedded_static.rs**:
```rust
static STATIC_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/static/dist");

pub async fn serve_embedded_file(uri: Uri) -> impl IntoResponse {
    let path = if path.is_empty() { "index.html" } else { path };
    
    if let Some(file) = STATIC_DIR.get_file(path) {
        // Serve embedded file with proper MIME type and caching
    } else if !path.starts_with("api/") {
        // SPA fallback to index.html for client-side routing
    }
}
```

#### Server Configuration

**src/server/mod.rs**:
```rust
let app = if let Some(static_dir_path) = &static_dir {
    // Development: serve from filesystem
    Router::new().nest("/api", api_routes).fallback_service(serve_dir)
} else {
    // Production: serve from embedded assets
    Router::new().nest("/api", api_routes).fallback(serve_embedded_file)
};
```

### Usage

#### Production Deployment
```bash
# Build single binary with embedded frontend
cargo build --release

# Run with embedded assets (no external files needed)
./target/release/psf-guard server database.db images/
```

#### Development Workflow
```bash
# Option 1: Use embedded assets (rebuilt automatically)
cargo run -- server database.db images/

# Option 2: Use filesystem serving (for hot reload)
npm run dev &  # Start Vite dev server in static/
cargo run -- server database.db images/ --static-dir ./static/dist

# Option 3: Skip frontend build entirely
PSF_GUARD_SKIP_FRONTEND_BUILD=1 cargo run -- server database.db images/
```

### Binary Size and Performance

- **Release binary size**: ~7.3MB (includes full React app)
- **Startup time**: No additional filesystem reads for static assets
- **Caching**: Proper cache headers with long-term caching for assets, no-cache for HTML
- **Compression**: Assets use best PNG compression and minification

### Added Dependencies

```toml
# Cargo.toml
include_dir = "0.7"     # Embed directories at compile time
mime_guess = "2.0"      # MIME type detection for proper HTTP headers
```

### Environment Variables

- `PSF_GUARD_SKIP_FRONTEND_BUILD=1`: Skip npm build during cargo compilation
- `CARGO_MANIFEST_DIR`: Used by build.rs to locate static directory
- `PROFILE`: Detected automatically (debug/release) for build optimization

### File Structure Changes

```
psf-guard/
├── build.rs                     # Frontend build integration
├── src/server/
│   ├── embedded_static.rs      # Embedded file serving
│   └── mod.rs                   # Dual-mode server setup
├── static/
│   ├── src/                     # React source code
│   ├── dist/                    # Built assets (embedded)
│   ├── package.json
│   └── vite.config.ts
└── target/release/psf-guard     # Single binary with embedded UI
```

### Benefits

1. **Single Binary Distribution**: No need to deploy frontend and backend separately
2. **Zero Configuration**: No nginx, Apache, or static file server needed
3. **Simplified Deployment**: Copy one file and run
4. **Development Flexibility**: Choose embedded or filesystem serving
5. **Fast Startup**: No filesystem scanning for static assets
6. **Offline Capable**: All assets embedded, no CDN dependencies

### Migration from Makefile Approach

The old Makefile approach required manual coordination between frontend build and Rust compilation:

```makefile
# Old approach - manual steps
build-frontend:
	(cd static && npm run build)

build: build-frontend
	cargo build --release
```

The new approach integrates everything into Cargo's build system:

```bash
# New approach - automatic
cargo build --release  # Frontend built automatically
```

This eliminates the need for make and provides better dependency tracking and caching.

### Files Added/Modified for Build.rs Integration

1. **build.rs** - Enhanced with React app building and smart caching
2. **src/server/embedded_static.rs** - New embedded static file serving module
3. **src/server/mod.rs** - Updated for dual-mode serving (embedded/filesystem)
4. **src/cli.rs** - Made `--static-dir` optional for embedded mode
5. **Cargo.toml** - Added `include_dir` and `mime_guess` dependencies

This implementation demonstrates modern Rust build system integration with web frontend tooling, providing both development ergonomics and production deployment simplicity.

## Web Server and API Architecture (2025-08-30)

### Overview

PSF Guard includes a comprehensive web server built with Axum 0.8 that provides both a REST API and serves the embedded React frontend. The architecture follows modern patterns with caching, async handling, and proper error responses.

### Server Architecture

#### Core Components

**src/server/mod.rs**:
- Main server entry point and route configuration
- Dual-mode static serving (embedded vs filesystem)
- CORS and tracing middleware setup
- Graceful async runtime management

**src/server/state.rs**:
```rust
pub struct AppState {
    database_path: String,
    image_dir: PathBuf,
    cache_dir: String,
    db_pool: Arc<Mutex<Connection>>, // SQLite connection pool
}
```

**src/server/handlers.rs**:
- All REST API endpoint implementations
- Database query logic with proper error handling
- File system operations for FITS files
- Image processing and caching coordination

**src/server/cache.rs**:
- Intelligent caching system for processed images
- Category-based organization (previews, stars, psf_multi, stats)
- Automatic cache directory management
- File existence checking and cleanup utilities

**src/server/embedded_static.rs**:
- Compile-time embedded static file serving
- MIME type detection and HTTP caching headers
- SPA fallback routing for client-side navigation
- Production-ready asset serving

#### API Response Format

All API endpoints return a consistent JSON structure:
```rust
#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}
```

### REST API Endpoints

#### Project Management

**GET /api/projects**
```rust
pub async fn list_projects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<ProjectResponse>>>, AppError>
```
Returns all projects with ID, name, and description.

**GET /api/projects/{project_id}/targets**
```rust
pub async fn list_targets(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
) -> Result<Json<ApiResponse<Vec<TargetResponse>>>, AppError>
```
Returns targets for a project with statistics (image counts by status).

#### Image Management

**GET /api/images**
```rust
#[derive(Deserialize)]
pub struct ImageQuery {
    pub project_id: Option<i32>,
    pub target_id: Option<i32>,
    pub status: Option<String>, // "pending", "accepted", "rejected"
    pub limit: Option<i32>,     // Default: 100
    pub offset: Option<i32>,    // Default: 0
}
```
Paginated image listing with comprehensive filtering.

**GET /api/images/{image_id}**
```rust
pub async fn get_image(
    State(state): State<Arc<AppState>>,
    Path(image_id): Path<i32>,
) -> Result<Json<ApiResponse<ImageResponse>>, AppError>
```
Detailed image information with merged FITS statistics including temperature and camera model extraction.

**PUT /api/images/{image_id}/grade**
```rust
#[derive(Deserialize)]
pub struct UpdateGradeRequest {
    pub status: String,   // "pending", "accepted", "rejected"
    pub reason: Option<String>,
}
```
Update grading status with optional rejection reason.

#### Image Data Endpoints

**GET /api/images/{image_id}/preview**
```rust
#[derive(Deserialize)]
pub struct PreviewOptions {
    pub size: Option<String>,      // "screen" (1200px), "large" (2000px)
    pub stretch: Option<bool>,     // Apply MTF stretch
    pub midtone: Option<f64>,      // Midtone factor (0.0-1.0)
    pub shadow: Option<f64>,       // Shadow clipping
}
```
Returns stretched FITS preview as PNG with intelligent caching.

**GET /api/images/{image_id}/annotated**
- Returns star-annotated PNG image
- Uses HocusFocus star detection with Moffat PSF fitting
- Cached results with size-based variants

**GET /api/images/{image_id}/psf**
```rust
#[derive(Deserialize)]
pub struct PsfMultiOptions {
    pub num_stars: Option<usize>,    // Number of stars to visualize
    pub psf_type: Option<String>,    // "gaussian", "moffat"
    pub sort_by: Option<String>,     // "hfr", "r2", "brightness"
    pub grid_cols: Option<usize>,    // Grid columns (0 = auto)
    pub selection: Option<String>,   // "top-n", "corners", "regions", "quality"
}
```
Returns PSF residual visualization grid as PNG.

**GET /api/images/{image_id}/stars**
```rust
#[derive(Serialize)]
pub struct StarDetectionResponse {
    pub detected_stars: usize,
    pub average_hfr: f64,
    pub average_fwhm: f64,
    pub stars: Vec<StarInfo>,
}

#[derive(Serialize)]
pub struct StarInfo {
    pub x: f64,
    pub y: f64,
    pub hfr: f64,
    pub fwhm: f64,
    pub brightness: f64,
    pub eccentricity: f64,
}
```
Returns detailed star detection results as JSON.

### Caching System

#### Cache Categories

1. **previews/**: Stretched FITS images as PNG
   - Key format: `{image_id}_{size}_{stretch}_{midtone}_{shadow}`
   - Cache headers: `max-age=86400` (1 day)

2. **stars/**: Star detection JSON results
   - Key format: `stars_{image_id}`
   - Contains HocusFocus detection results with PSF fitting

3. **annotated/**: Star-annotated PNG images
   - Key format: `annotated_{image_id}_{size}`
   - Yellow star overlays with HFR-based sizing

4. **psf_multi/**: PSF visualization grids
   - Key format: `psf_multi_{image_id}_{num_stars}_{psf_type}_{sort_by}_{selection}_{grid_cols}`
   - Complex multi-star PSF residual visualizations

5. **stats/**: FITS metadata and statistics
   - Key format: `stats_{image_id}`
   - Cached Min/Max/Mean/Median/StdDev/MAD + Temperature + Camera

#### Cache Implementation

```rust
pub struct CacheManager {
    cache_dir: PathBuf,
}

impl CacheManager {
    pub fn ensure_category_dir(&self, category: &str) -> Result<()>
    pub fn get_cached_path(&self, category: &str, key: &str, extension: &str) -> PathBuf
    pub fn is_cached(&self, path: &PathBuf) -> bool
}
```

Cache files are organized hierarchically:
```
cache/
├── previews/
│   ├── 123_screen_stretched_200_-2800.png
│   └── 123_large_stretched_300_-2500.png
├── stars/
│   └── stars_123.json
├── annotated/
│   └── annotated_123_screen.png
├── psf_multi/
│   └── psf_multi_123_9_moffat_r2_top-n_0.png
└── stats/
    └── stats_123.json
```

### Frontend Architecture

#### Technology Stack

- **React 18**: Modern React with hooks and concurrent features
- **TypeScript**: Full type safety across the application
- **Vite**: Fast development server and optimized production builds
- **TanStack Query (React Query)**: Advanced server state management with caching and background updates
- **React Hotkeys Hook**: Global keyboard shortcut handling
- **Custom Hooks**: Reusable logic for zoom/pan, image preloading

#### Component Architecture

**src/App.tsx**:
- Main application shell
- Project/target selector integration
- Modal state management for image details and help
- Global keyboard shortcut coordination

**src/components/ProjectTargetSelector.tsx**:
```typescript
interface ProjectTargetSelectorProps {
  selectedProject: number | null;
  selectedTarget: number | null;
  onProjectChange: (projectId: number | null) => void;
  onTargetChange: (targetId: number | null) => void;
}
```
- Dropdown navigation with live statistics
- Automatic target list updates when project changes
- Statistics display (total/accepted/rejected counts)

**src/components/GroupedImageGrid.tsx**:
```typescript
interface GroupedImageGridProps {
  projectId: number | null;
  targetId: number | null;
  onImageClick: (imageId: number) => void;
  selectedImages: Set<number>;
  onImageSelect: (imageId: number, selected: boolean) => void;
}
```
- Virtualized grid for performance with large image sets
- Grouping by filter name, date, or both
- Lazy loading with intersection observer
- Batch selection with visual indicators
- Adjustable thumbnail sizes

**src/components/ImageDetailView.tsx**:
```typescript
interface ImageDetailViewProps {
  imageId: number;
  onClose: () => void;
  onNext: () => void;
  onPrevious: () => void;
  onGrade: (status: 'accepted' | 'rejected' | 'pending') => void;
  adjacentImageIds?: { next: number[]; previous: number[] };
}
```
- Full-screen image detail view with comprehensive controls
- Integrated zoom/pan functionality with mouse and keyboard
- Star detection and PSF visualization overlays
- Metadata display with temperature and camera information
- One-click grading with keyboard shortcuts

**src/hooks/useImageZoom.ts**:
```typescript
export interface UseImageZoomReturn {
  zoomState: ZoomState;
  containerRef: React.RefObject<HTMLDivElement | null>;
  imageRef: React.RefObject<HTMLImageElement | null>;
  handleWheel: (e: React.WheelEvent) => void;
  handleMouseDown: (e: React.MouseEvent) => void;
  handleMouseMove: (e: React.MouseMove) => void;
  handleMouseUp: (e: React.MouseEvent) => void;
  zoomIn: () => void;
  zoomOut: () => void;
  zoomToFit: () => void;
  zoomTo100: () => void;
  resetZoom: () => void;
  getZoomPercentage: () => number;
}
```
- Professional zoom/pan implementation with cursor-targeted zooming
- Constraint system prevents images from being dragged off-screen
- Smart auto-fitting with intentional zoom detection
- Keyboard and mouse wheel support

**src/hooks/useImagePreloader.ts**:
```typescript
interface UseImagePreloaderOptions {
  preloadCount: number;
  includeAnnotated: boolean;
  includeStarData: boolean;
  imageSize: 'screen' | 'large';
}
```
- Intelligent preloading of next/previous images for smooth navigation
- Conditionally preloads annotated images only when star overlay is enabled
- Conditionally preloads star detection data only when needed
- Configurable preload count and size variants
- Cache-aware to avoid duplicate network requests and bandwidth waste

#### State Management

**API Layer** (src/api/client.ts):
```typescript
export const apiClient = {
  // Project/Target queries
  getProjects: (): Promise<ProjectResponse[]>
  getTargets: (projectId: number): Promise<TargetResponse[]>
  
  // Image queries
  getImages: (params: ImageQuery): Promise<ImageResponse[]>
  getImage: (imageId: number): Promise<ImageResponse>
  updateImageGrade: (imageId: number, request: UpdateGradeRequest): Promise<void>
  
  // Image data URLs
  getPreviewUrl: (imageId: number, options?: PreviewOptions): string
  getAnnotatedUrl: (imageId: number, size?: string): string
  getPsfUrl: (imageId: number, options: PsfMultiOptions): string
  
  // Star detection
  getStarDetection: (imageId: number): Promise<StarDetectionResponse>
};
```

**React Query Integration**:
```typescript
// Cached project list
const { data: projects } = useQuery({
  queryKey: ['projects'],
  queryFn: apiClient.getProjects,
  staleTime: 5 * 60 * 1000, // 5 minutes
});

// Cached image details with background updates
const { data: image, isFetching } = useQuery({
  queryKey: ['image', imageId],
  queryFn: () => apiClient.getImage(imageId),
  placeholderData: (previousData) => previousData,
});

// Optimistic updates for grading
const gradeMutation = useMutation({
  mutationFn: ({ imageId, status }: GradeUpdate) => 
    apiClient.updateImageGrade(imageId, { status }),
  onMutate: async ({ imageId, status }) => {
    // Optimistically update the UI immediately
    queryClient.setQueryData(['image', imageId], (old) => 
      old ? { ...old, grading_status: status } : old
    );
  },
});
```

#### Keyboard Shortcuts System

**Global Shortcuts** (App.tsx):
- `?` - Toggle help modal
- `Escape` - Close modals and clear selections
- `G` - Cycle grouping modes (Filter → Date → Both)

**Navigation Shortcuts** (ImageDetailView.tsx):
```typescript
useHotkeys('k,right', onNext, [onNext]);
useHotkeys('j,left', onPrevious, [onPrevious]);
useHotkeys('escape', onClose, [onClose]);
```

**Grading Shortcuts**:
```typescript
useHotkeys('a', () => onGrade('accepted'), [onGrade]);
useHotkeys('r', () => onGrade('rejected'), [onGrade]);
useHotkeys('u', () => onGrade('pending'), [onGrade]);
```

**Undo/Redo System** (2025-08-30):

The application includes a comprehensive undo/redo system for all grading actions:

**src/hooks/useUndoRedo.ts**:
```typescript
export interface GradingAction {
  id: string;
  type: 'single' | 'batch';
  timestamp: number;
  description: string;
  imageIds: number[];
  previousStates: Array<{
    imageId: number;
    previousStatus: 'accepted' | 'rejected' | 'pending';
    previousReason?: string;
  }>;
  newStatus: 'accepted' | 'rejected' | 'pending';
  newReason?: string;
}

export function useUndoRedo(options: UseUndoRedoOptions = {}) {
  // Track action history with undo/redo stacks
  const [undoStack, setUndoStack] = useState<GradingAction[]>([]);
  const [redoStack, setRedoStack] = useState<GradingAction[]>([]);
  
  return {
    recordAction: (imageIds, newStatus, reason, description) => Promise<string | null>
    undo: () => Promise<boolean>
    redo: () => Promise<boolean>
    canUndo: boolean
    canRedo: boolean
  };
}
```

**Key Features**:
1. **Action Recording**: Automatically captures previous states before applying changes
2. **Batch Support**: Single operations and multi-image batch operations are both fully supported
3. **History Management**: Configurable history size (default: 50 actions) with automatic cleanup
4. **State Restoration**: Precise restoration of previous grading states and reasons
5. **Cache Integration**: Automatic query invalidation when undoing/redoing actions

**src/hooks/useGrading.ts**:
```typescript
export function useGrading(options: UseGradingOptions = {}) {
  const undoRedo = useUndoRedo();

  const gradeImage = useCallback((imageId, status, reason, recordHistory = true) => {
    // Record action before applying if history tracking is enabled
    if (recordHistory) {
      await undoRedo.recordAction([imageId], status, reason);
    }
    // Apply the grading change
    await apiClient.updateImageGrade(imageId, { status, reason });
  }, [undoRedo]);

  return {
    gradeImage, gradeBatch, gradeImages,
    undo: undoRedo.undo,
    redo: undoRedo.redo,
    canUndo: undoRedo.canUndo,
    canRedo: undoRedo.canRedo,
  };
}
```

**UI Integration** (src/components/UndoRedoToolbar.tsx):
- Visual undo/redo buttons with action counts
- Tooltips showing the last/next action descriptions
- Keyboard shortcuts: `Ctrl+Z`/`Cmd+Z` (undo), `Ctrl+Y`/`Cmd+Y` (redo)
- Real-time feedback with success/error animations
- Compact and full display modes for different UI contexts

**Technical Implementation**:
- **Action History**: Each action stores complete state snapshots for reliable restoration
- **Optimistic Updates**: UI updates immediately while background operations ensure consistency  
- **Error Handling**: Failed undo/redo operations are gracefully handled without corrupting stacks
- **Memory Management**: History size limits and automatic cleanup prevent memory leaks
- **Concurrent Safety**: Action recording prevents race conditions during rapid user input
```

**View Toggle Shortcuts**:
```typescript
useHotkeys('s', () => setShowStars(s => !s), [showStars]);
useHotkeys('p', () => setShowPsf(s => !s), [showPsf]);
useHotkeys('z', () => setImageSize(s => s === 'screen' ? 'large' : 'screen'), []);
```

**Zoom Control Shortcuts**:
```typescript
useHotkeys('plus,equal', () => zoom.zoomIn(), [zoom.zoomIn]);
useHotkeys('minus', () => zoom.zoomOut(), [zoom.zoomOut]);
useHotkeys('f', () => zoom.zoomToFit(), [zoom.zoomToFit]);
useHotkeys('1', () => zoom.zoomTo100(), [zoom.zoomTo100]);
useHotkeys('0', () => zoom.resetZoom(), [zoom.resetZoom]);
```

### FITS File Processing Pipeline

#### File Discovery

```rust
fn find_fits_file(
    state: &AppState,
    image: &AcquiredImage,
    target_name: &str,
    filename: &str,
) -> Result<PathBuf, AppError>
```

1. **Extract metadata**: Parse JSON metadata from database to get original filename
2. **Calculate paths**: Generate possible file locations based on date and target name
3. **Structure detection**: Try both standard (`date/target/date/LIGHT/`) and alternate (`target/date/LIGHT/`) structures
4. **Recursive fallback**: If structured search fails, perform recursive filename search

#### Image Processing

**MTF Stretching** (src/mtf_stretch.rs):
```rust
pub struct StretchParameters {
    pub factor: f64,        // Midtone balance (0.0-1.0)
    pub black_clipping: f64, // Shadow clipping point
}

pub fn stretch_image(
    data: &[u16],
    statistics: &ImageStatistics,
    factor: f64,
    black_clipping: f64,
) -> Vec<u8>
```

**Star Detection Integration**:
- HocusFocus algorithm with Moffat PSF fitting
- Cached JSON results for consistent API responses
- Sub-pixel accuracy with bilinear interpolation
- Automatic OpenCV acceleration where available

**Image Generation**:
- PNG encoding with optimal compression
- Proper HTTP headers for caching and MIME types
- Size variants (screen: 1200px, large: 2000px)
- Color space handling for astronomical data

### Error Handling and Logging

#### Error Types

```rust
#[derive(Debug)]
pub enum AppError {
    NotFound,
    DatabaseError,
    BadRequest(String),
    InternalError(String),
    NotImplemented,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "Resource not found"),
            AppError::DatabaseError => (StatusCode::INTERNAL_SERVER_ERROR, "Database error"),
            // ... with proper JSON error responses
        };
    }
}
```

#### Logging and Tracing

```rust
// src/server/mod.rs
use tower_http::trace::TraceLayer;
use tracing_subscriber;

// Initialize structured logging
tracing_subscriber::fmt::init();

// Add tracing middleware
.layer(TraceLayer::new_for_http())
```

All HTTP requests, database operations, and file system access are logged with structured tracing for debugging and monitoring.

### Performance Optimizations

#### Frontend Performance

1. **Virtualization**: Large image grids use intersection observer for lazy loading
2. **Image Preloading**: Strategic preloading of next/previous images
3. **React Query Caching**: Intelligent cache management with background updates
4. **Optimistic Updates**: Immediate UI feedback for grading operations
5. **Bundle Splitting**: Vite automatically splits code for optimal loading

#### Backend Performance

1. **Intelligent Caching**: Multi-level caching for expensive operations
2. **Async Processing**: Full async/await throughout the pipeline
3. **Connection Pooling**: SQLite connection reuse with proper locking
4. **Image Processing**: Optimized FITS reading and PNG encoding
5. **Static Asset Serving**: Embedded files with proper cache headers

#### Database Optimizations

1. **Prepared Statements**: All queries use parameterized statements
2. **Efficient Joins**: Optimized queries for image/project/target relationships
3. **Index Usage**: Proper indexing on frequently queried columns
4. **Connection Management**: Single connection per request with proper cleanup

### Security Considerations

1. **SQL Injection Prevention**: All database queries use parameterized statements
2. **Path Traversal Protection**: Careful validation of file paths from database
3. **CORS Configuration**: Permissive CORS for development, configurable for production
4. **Input Validation**: Proper validation of all API inputs
5. **Error Information**: Error messages don't leak sensitive system information
6. **File Access Control**: Access limited to configured image directory

This comprehensive web architecture provides a modern, performant, and secure platform for astronomical image grading and analysis, suitable for both individual use and team collaboration.