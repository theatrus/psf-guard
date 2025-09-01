# Statistical Grading Documentation

## Overview

The statistical grading feature in PSF Guard provides advanced image quality analysis beyond the basic accept/reject status stored in the database. It uses statistical methods to identify outliers and detect transient issues like clouds that may affect image quality.

## Key Features

### 1. Per-Target Analysis
All statistical analysis is performed on a per-target basis, grouping images by both target and filter. This ensures that:
- Different targets with varying star densities are analyzed appropriately
- Sky conditions specific to each target's location are considered
- Filter-specific characteristics are preserved

### 2. Distribution-Based Outlier Detection

#### HFR (Half Flux Radius) Analysis
- Detects images with focus quality significantly different from the target's baseline
- Uses standard deviation (σ) for normally distributed data
- Switches to Median Absolute Deviation (MAD) for skewed distributions
- Default threshold: 2.0σ (configurable)

#### Star Count Analysis
- Identifies images with abnormal star detection counts
- Useful for detecting:
  - Partial clouds or haze
  - Tracking errors
  - Field rotation issues
- Default threshold: 2.0σ (configurable)

### 3. Cloud Detection (Sequence Analysis)

The cloud detection algorithm monitors image sequences for sudden deterioration in quality that indicates clouds:

#### How It Works
1. **Baseline Establishment**: The first N images (default: 5) establish a baseline median
2. **Continuous Monitoring**: Each subsequent image is compared to the rolling baseline
3. **Cloud Detection**: 
   - HFR increase > threshold (default: 20%) indicates clouds
   - Star count decrease > threshold (default: 20%) indicates clouds
4. **Recovery**: After a cloud event, a new baseline must be established before accepting images

#### Detection Metrics
- **Primary**: HFR increase (higher HFR = worse seeing/clouds)
- **Secondary**: Star count decrease (fewer stars = obscuration)

## Configuration Options

### Command Line Arguments

```bash
# Enable all statistical features
--enable-statistical

# HFR outlier detection
--stat-hfr                    # Enable HFR analysis
--hfr-stddev <value>         # Standard deviations threshold (default: 2.0)

# Star count outlier detection  
--stat-stars                  # Enable star count analysis
--star-stddev <value>        # Standard deviations threshold (default: 2.0)

# Distribution analysis
--stat-distribution           # Enable median/mean shift detection
--median-shift-threshold <value>  # Threshold for distribution skew (default: 0.1)

# Cloud detection
--stat-clouds                 # Enable cloud detection
--cloud-threshold <value>     # Sensitivity threshold (default: 0.2 = 20%)
--cloud-baseline-count <n>    # Images for baseline (default: 5)
```

## Usage Examples

### Basic Statistical Analysis
```bash
# Dry run with HFR and star count analysis
psf-guard filter-rejected mydb.sqlite ./images --dry-run \
  --enable-statistical --stat-hfr --stat-stars
```

### Cloud Detection Only
```bash
# Focus on cloud detection with custom sensitivity
psf-guard filter-rejected mydb.sqlite ./images --dry-run \
  --enable-statistical --stat-clouds --cloud-threshold 0.15
```

### Full Analysis with Custom Thresholds
```bash
# Conservative settings for critical data
psf-guard filter-rejected mydb.sqlite ./images --dry-run \
  --enable-statistical \
  --stat-hfr --hfr-stddev 1.5 \
  --stat-stars --star-stddev 1.5 \
  --stat-clouds --cloud-threshold 0.1 --cloud-baseline-count 10
```

### Target-Specific Analysis
```bash
# Analyze specific target with all features
psf-guard filter-rejected mydb.sqlite ./images --dry-run \
  --target "M31" \
  --enable-statistical --stat-hfr --stat-stars --stat-clouds
```

## Understanding the Output

### Statistical Rejection Messages

1. **HFR Outlier**:
   ```
   Image 123: Statistical HFR - HFR 3.456 is 2.5σ from mean 2.890 (threshold: 2.0σ)
   ```

2. **Star Count Outlier**:
   ```
   Image 456: Statistical Stars - Star count 150 is 3.1σ from mean 420 (threshold: 2.0σ)
   ```

3. **Distribution-Based (MAD)**:
   ```
   Image 789: Distribution HFR - HFR 4.123 deviates 2.8 MAD from median 3.100 (threshold: 2.0)
   ```

4. **Cloud Detection**:
   ```
   Image 321: Cloud Detection - HFR 3.890 is 25% above baseline 3.112 (threshold: 20%)
   Image 654: Cloud Detection (Stars) - Star count 210 is 35% below baseline 323 (threshold: 20%)
   ```

## Best Practices

### 1. Start with Dry Runs
Always use `--dry-run` first to preview what would be rejected before moving files.

### 2. Tune Thresholds Gradually
- Start with defaults (2.0σ)
- Review rejected images
- Adjust thresholds based on your quality requirements
- More aggressive: 1.5σ
- More conservative: 2.5σ or 3.0σ

### 3. Cloud Detection Sensitivity
- Default 20% works well for obvious clouds
- 10-15% for detecting thin clouds or haze
- 25-30% for only severe cloud events

### 4. Baseline Count Considerations
- Default 5 images works for most scenarios
- Increase to 10+ for:
  - Rapidly changing conditions
  - High-cadence imaging
  - Critical data where false positives must be minimized
- Decrease to 3 for:
  - Slow-cadence imaging
  - Stable conditions
  - When you want faster cloud recovery

### 5. Combining Features
- Use HFR + Stars for comprehensive quality control
- Add cloud detection for unattended/automated sessions
- Distribution analysis helps with non-normal data

## Algorithm Details

### Standard Deviation Method
Used when data is normally distributed:
```
z-score = |value - mean| / stddev
reject if z-score > threshold
```

### Median Absolute Deviation (MAD)
Used when median significantly differs from mean (skewed distribution):
```
MAD = median(|xi - median|) × 1.4826
z-score = |value - median| / MAD
reject if z-score > threshold
```

### Cloud Detection Algorithm
```
1. Sort images by timestamp within target/filter group
2. Establish baseline from first N images
3. For each subsequent image:
   - Calculate % change from baseline median
   - If change > threshold:
     - Mark as cloud-affected
     - Reset baseline collection
   - Else:
     - Update rolling baseline
```

## Troubleshooting

### Too Many False Positives
- Increase stddev thresholds (e.g., 2.5 or 3.0)
- Increase cloud threshold (e.g., 0.25 or 0.3)
- Increase baseline count for more stable baseline

### Missing Obvious Problems
- Decrease stddev thresholds (e.g., 1.5)
- Decrease cloud threshold (e.g., 0.15)
- Check if --enable-statistical flag is set

### Different Results Than Expected
- Remember analysis is per-target
- Check if distribution analysis is triggering (MAD vs stddev)
- Verify chronological ordering for cloud detection

## Database Regrading

The `regrade` command allows you to apply statistical grading directly to the database, updating image statuses based on analysis results.

### Basic Usage
```bash
# Dry run - see what would change
psf-guard regrade mydb.sqlite --dry-run --enable-statistical --stat-hfr --stat-stars

# Actually update the database
psf-guard regrade mydb.sqlite --enable-statistical --stat-hfr --stat-stars
```

### Reset Options

The `--reset` parameter controls how existing grades are handled:

1. **none** (default): Keep existing grades, only add new rejections
2. **automatic**: Reset automatically graded images to pending (preserves manual grades)
3. **all**: Reset all images to pending status

```bash
# Reset automatic grades from last 30 days
psf-guard regrade mydb.sqlite --reset automatic --days 30

# Reset all grades for a specific target
psf-guard regrade mydb.sqlite --reset all --target "NGC 7000" --days 7
```

### Automatic Grade Markers

When the regrade command rejects an image, it prefixes the rejection reason with `[Auto]` to distinguish from manual grades:
- `[Auto] Statistical HFR - HFR 3.456 is 2.5σ from mean...`
- `[Auto] Cloud Detection - HFR 3.890 is 25% above baseline...`

This allows the `--reset automatic` option to identify and reset only these automatic grades.

### Safety Features

1. **Always use dry-run first**: Check what will be changed before updating
2. **Date filtering**: Default 90 days prevents analyzing very old data
3. **Preserves manual grades**: With `--reset automatic`, manual rejections are kept
4. **Atomic updates**: Each rejection is updated individually with error handling

### Workflow Examples

#### Initial Statistical Grading
```bash
# First time - add statistical grades to ungraded images
psf-guard regrade mydb.sqlite --enable-statistical --stat-hfr --stat-stars
```

#### Periodic Reanalysis
```bash
# Weekly regrade with updated baselines
psf-guard regrade mydb.sqlite --reset automatic --days 7 \
  --enable-statistical --stat-hfr --stat-stars --stat-clouds
```

#### Target-Specific Tuning
```bash
# Aggressive grading for critical target
psf-guard regrade mydb.sqlite --target "SN 2023xyz" \
  --enable-statistical --stat-hfr --hfr-stddev 1.5 \
  --stat-stars --star-stddev 1.5
```

## Performance Considerations

- Statistical analysis requires loading all images into memory
- For large datasets (10,000+ images), expect increased processing time
- Cloud detection requires chronological sorting, adding overhead
- Consider filtering by project/target to reduce dataset size
- Database updates are performed individually for safety