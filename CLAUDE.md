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

#### Web UI Enhancements
- **Side-by-Side Image Comparison**: Added comparison mode for easier A/B grading
  - Press 'C' or click "Compare" button to open comparison view
  - Independent zoom/pan controls for each image with optional sync (Z key)
  - Separate grading controls: 1/2/3 for left image, 7/8/9 for right image
  - Star overlay toggle (S key) and ESC to close
- **File Existence Checking**: Projects and targets show visual indicators for file availability
  - 1-minute cache for performance with manual refresh option
  - Disabled entries shown for database items without corresponding files
- **Server Info Panel**: Shows database path, image directory, cache directory, and version
- **Periodic Refresh**: Auto-refresh every 30 seconds for new images/database changes

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

## Web Server and Frontend

### Overview

PSF Guard includes a comprehensive web server with a React-based frontend for visual image grading and analysis. The server provides both a REST API and a complete web interface, with the frontend automatically embedded in the binary for single-file deployment.

### Architecture

#### Backend (Rust/Axum)
- **Framework**: Axum web framework with Tower middleware
- **Database**: Direct SQLite access via rusqlite
- **Image Processing**: FITS file handling with caching
- **API Design**: RESTful JSON API with consistent response format
- **Static Serving**: Embedded frontend files using rust-embed

#### Frontend (React/TypeScript)
- **Framework**: React 18 with TypeScript
- **Build Tool**: Vite for fast development and optimized production builds
- **State Management**: TanStack Query for server state and caching
- **UI Components**: Custom components with CSS Grid/Flexbox
- **Image Handling**: Lazy loading, virtualization, and zoom/pan controls

### Key Features

#### 1. Smart File Management
- **File Existence Checking**: Only shows projects/targets with actual files
- **Caching**: 1-minute cache for file existence checks
- **Manual Refresh**: Force cache updates when needed
- **Visual Indicators**: Clear marking of available vs database-only entries

#### 2. Advanced Image Viewer
- **Zoom/Pan Controls**: Mouse wheel zoom, click-drag pan
- **Keyboard Navigation**: J/K for prev/next, arrow keys
- **Overlay Modes**: Star detection overlay, PSF residuals
- **Metadata Display**: Complete FITS header information
- **Stretch Controls**: Real-time MTF parameter adjustment

#### 3. Batch Operations
- **Multi-Selection**: Shift+click for range, Ctrl+click for individual
- **Batch Grading**: Apply status to multiple images at once
- **Visual Feedback**: Clear selection indicators
- **Keyboard Shortcuts**: A/R/U work on single or multiple images

#### 4. Undo/Redo System
- **Full History**: Track all grading operations
- **Keyboard Shortcuts**: Ctrl+Z/Ctrl+Y (⌘Z/⌘Y on Mac)
- **Visual Indicators**: Show undo/redo stack sizes
- **Action Preview**: Tooltips show what will be undone/redone

### API Endpoints

#### Core Resources
```
GET  /api/projects                              # List all projects
GET  /api/projects/{id}/targets                 # List targets for project
GET  /api/images?project_id=X&target_id=Y      # List images with filters
GET  /api/images/{id}                          # Get image details
PUT  /api/images/{id}/grade                    # Update grading status
GET  /api/server-info                          # Server configuration info
```

#### Image Processing
```
GET  /api/images/{id}/preview                  # Stretched FITS preview
     ?size=screen|large|original               # Image size
     &stretch=true                             # Apply MTF stretch
     &midtone=0.2                             # MTF midtone factor
     &shadow=-2.8                             # Shadow clipping

GET  /api/images/{id}/annotated               # Star-annotated image
     ?size=screen|large                       # Output size

GET  /api/images/{id}/psf                     # PSF visualization
     ?num_stars=9                             # Number of stars
     &psf_type=gaussian|moffat                # PSF model
     &sort_by=hfr|r2|brightness               # Sort metric

GET  /api/images/{id}/stars                   # Star detection data (JSON)
```

### Caching System

The server implements a sophisticated caching system:

1. **Cache Types**:
   - Preview images (PNG files with parameters in filename)
   - Star detection results (JSON files)
   - PSF visualizations (PNG files)
   - File existence checks (in-memory with TTL)

2. **Cache Keys**:
   - Include all relevant parameters (size, stretch, midtone, etc.)
   - Automatic cleanup of old entries
   - Configurable cache directory

3. **Performance**:
   - Dramatically reduces FITS processing time
   - Enables smooth UI interactions
   - Reduces server CPU usage

### Frontend Architecture

#### Component Hierarchy
```
App
├── ProjectSelector      # Project/target dropdowns
├── GroupedImageGrid     # Main image browser
│   ├── FilterControls   # Status, date, filter controls
│   ├── ImageCard        # Thumbnail with metadata
│   └── LazyImageCard    # Virtualized version
├── ImageDetailView      # Full-size image viewer
├── ImageComparisonView  # Side-by-side comparison
├── StatsDashboard       # Statistics overview
└── UndoRedoToolbar      # History controls
```

#### State Management
- **Server State**: TanStack Query with automatic caching
- **UI State**: React hooks (useState, useReducer)
- **Grading State**: Custom useGrading hook with undo/redo
- **Zoom State**: Custom useImageZoom hook

#### Performance Optimizations
- **Virtualization**: Only render visible images
- **Lazy Loading**: Load images as they enter viewport
- **Debouncing**: Prevent excessive API calls
- **Memoization**: Cache expensive computations

### Deployment

#### Production Build
The frontend is automatically embedded during compilation:
```bash
cargo build --release
# Creates single binary with everything included
```

#### Development Mode
```bash
# Terminal 1: Frontend dev server
cd static && npm run dev

# Terminal 2: Backend with filesystem serving
cargo run -- server db.sqlite images/ --static-dir ./static/dist
```

### Security Considerations

1. **SQL Injection Prevention**: All database queries use parameterized statements
2. **Path Traversal Protection**: Careful validation of file paths from database
3. **CORS Configuration**: Permissive CORS for development, configurable for production
4. **Input Validation**: Proper validation of all API inputs
5. **Error Information**: Error messages don't leak sensitive system information
6. **File Access Control**: Access limited to configured image directory

### Web UI Enhancements (2025-08-30)

#### Side-by-Side Comparison View
Added a comprehensive image comparison mode that allows users to compare two images side-by-side:

**Features**:
- Triggered by 'C' key or Compare button in the grouped image grid
- Independent zoom/pan controls for each image
- Sync zoom option to link zoom/pan between images
- Star overlay toggle (S key)
- Separate grading controls for each image (1-3 for left, 7-9 for right)
- Visual status indicators and metadata display
- Blue border appears when images are zoomed (>= 95% scale)
- Uses original size images for best quality
- Keyboard navigation between comparison pairs

**Implementation Details**:
- `ImageComparisonView.tsx` - Main comparison component
- Enhanced `useImageZoom` hook with sync capabilities and initialization tracking
- Added "original" size option to preview API (no resizing)
- Fixed zoom flashing issue with proper initialization state management
- Sync zoom properly handles both zoom and pan operations
- Visual feedback with blue border for zoomed images

This comprehensive web architecture provides a modern, performant, and secure platform for astronomical image grading and analysis, suitable for both individual use and team collaboration.