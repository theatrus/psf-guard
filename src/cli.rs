use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "psf-guard")]
#[command(about = "PSF Guard: Astronomical image analysis and quality assessment tool", long_about = None)]
pub struct Cli {
    #[arg(short, long, default_value = "schedulerdb.sqlite")]
    pub database: String,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Dump grading results for all images
    DumpGrading {
        /// Show only specific grading status (pending, accepted, rejected)
        #[arg(short, long)]
        status: Option<String>,

        /// Filter by project name
        #[arg(short, long)]
        project: Option<String>,

        /// Filter by target name
        #[arg(short, long)]
        target: Option<String>,

        /// Output format (json, csv, table)
        #[arg(short, long, default_value = "table")]
        format: String,
    },

    /// List all projects
    ListProjects,

    /// List targets for a specific project
    ListTargets {
        /// Project ID or name
        project: String,
    },

    /// Filter rejected files and move them to LIGHT_REJECT folders
    FilterRejected {
        /// Database file to use
        database: String,

        /// Base directory containing the image files
        base_dir: String,

        /// Perform a dry run (show what would be moved without actually moving)
        #[arg(long)]
        dry_run: bool,

        /// Filter by project name
        #[arg(short, long)]
        project: Option<String>,

        /// Filter by target name
        #[arg(short, long)]
        target: Option<String>,

        /// Enable verbose output for debugging path issues
        #[arg(short, long)]
        verbose: bool,

        #[command(flatten)]
        stat_options: StatisticalOptions,
    },

    /// Regrade images in the database based on statistical analysis
    Regrade {
        /// Database file to use
        database: String,

        /// Perform a dry run (show what would be changed without actually updating)
        #[arg(long)]
        dry_run: bool,

        /// Filter by target name
        #[arg(short, long)]
        target: Option<String>,

        /// Filter by project name
        #[arg(short, long)]
        project: Option<String>,

        /// Number of days to look back (default: 90)
        #[arg(long, default_value = "90")]
        days: u32,

        /// Reset mode: automatic, all, or none (default: none)
        #[arg(long, default_value = "none")]
        reset: String,

        #[command(flatten)]
        stat_options: StatisticalOptions,
    },

    /// Show details for specific images by ID
    ShowImages {
        /// Comma-separated list of image IDs
        ids: String,
    },

    /// Manually update the grading status of an image
    UpdateGrade {
        /// Image ID to update
        id: i32,

        /// New grading status (pending, accepted, rejected)
        status: String,

        /// Rejection reason (optional, used when status is rejected)
        #[arg(short, long)]
        reason: Option<String>,
    },

    /// Read and display metadata from FITS files
    ReadFits {
        /// Path to FITS file or directory containing FITS files
        path: String,

        /// Show verbose output with all headers
        #[arg(short, long)]
        verbose: bool,

        /// Output format (table, json, csv)
        #[arg(short, long, default_value = "table")]
        format: String,
    },

    /// Analyze FITS images and compare computed statistics with database values
    AnalyzeFits {
        /// Path to FITS file or directory containing FITS files
        path: String,

        /// Filter by project name
        #[arg(short, long)]
        project: Option<String>,

        /// Filter by target name
        #[arg(short, long)]
        target: Option<String>,

        /// Output format (table, json, csv)
        #[arg(short, long, default_value = "table")]
        format: String,

        /// Star detection algorithm to use (nina, hocusfocus)
        #[arg(long, default_value = "hocusfocus")]
        detector: String,

        /// Star detection sensitivity (normal, high, highest)
        #[arg(long, default_value = "normal")]
        sensitivity: String,

        /// Apply MTF stretch before detection (enabled by default, use --no-apply-stretch to disable)
        #[arg(long, default_value = "false")]
        apply_stretch: bool,

        /// Compare all detector combinations (overrides individual settings)
        #[arg(long)]
        compare_all: bool,

        /// PSF fitting type (none, gaussian, moffat4)
        #[arg(long, default_value = "none")]
        psf_type: String,

        /// Enable verbose debug output
        #[arg(long, short)]
        verbose: bool,
    },

    /// Convert FITS to PNG with MTF stretch applied
    StretchToPng {
        /// Path to FITS file
        fits_path: String,

        /// Output PNG path (if not provided, uses FITS filename with .png extension)
        #[arg(short, long)]
        output: Option<String>,

        /// MTF midtone balance factor (0.0-1.0, default: 0.2)
        #[arg(long, default_value = "0.2")]
        midtone_factor: f64,

        /// Shadow clipping in standard deviations (negative value, default: -2.8)
        #[arg(long, default_value = "-2.8")]
        shadow_clipping: f64,

        /// Apply logarithmic scaling instead of MTF stretch
        #[arg(long)]
        logarithmic: bool,

        /// Invert the image (black stars on white background)
        #[arg(long)]
        invert: bool,
    },

    /// Create annotated PNG with detected stars marked
    AnnotateStars {
        /// Path to FITS file
        fits_path: String,

        /// Output PNG path (if not provided, uses FITS filename with _annotated.png suffix)
        #[arg(short, long)]
        output: Option<String>,

        /// Maximum number of stars to annotate (default: 500)
        #[arg(long, default_value = "500")]
        max_stars: usize,

        /// Star detection algorithm to use: nina or hocusfocus
        #[arg(long, default_value = "hocusfocus")]
        detector: String,

        /// Star detection sensitivity (normal, high, highest) - only for nina detector
        #[arg(long, default_value = "normal")]
        sensitivity: String,

        /// MTF midtone balance factor (0.0-1.0, default: 0.2)
        #[arg(long, default_value = "0.2")]
        midtone_factor: f64,

        /// Shadow clipping in standard deviations (negative value, default: -2.8)
        #[arg(long, default_value = "-2.8")]
        shadow_clipping: f64,

        /// Color for star annotations (red, green, blue, yellow, cyan, magenta, white)
        #[arg(long, default_value = "red")]
        annotation_color: String,

        /// PSF fitting type (none, gaussian, moffat4)
        #[arg(long, default_value = "none")]
        psf_type: String,

        /// Enable verbose debug output
        #[arg(long, short)]
        verbose: bool,
    },

    /// Visualize PSF fit residuals for detected stars
    VisualizePsf {
        /// Path to FITS file
        fits_path: String,

        /// Output PNG path (if not provided, uses FITS filename with _psf_residuals.png suffix)
        #[arg(short, long)]
        output: Option<String>,

        /// Star index to visualize (0-based, default: 0 for best star)
        #[arg(long)]
        star_index: Option<usize>,

        /// PSF fitting type (gaussian or moffat4)
        #[arg(long, default_value = "moffat4")]
        psf_type: String,

        /// Maximum number of stars to consider (default: 9)
        #[arg(long, default_value = "9")]
        max_stars: usize,

        /// Star selection mode (top, regions, quality, corners)
        #[arg(long, default_value = "top")]
        selection_mode: String,

        /// Sort criteria (r2, hfr, brightness)
        #[arg(long, default_value = "r2")]
        sort_by: String,

        /// Enable verbose debug output
        #[arg(long, short)]
        verbose: bool,
    },

    /// Advanced multi-star PSF visualization with flexible layouts
    VisualizePsfMulti {
        /// Path to FITS file
        fits_path: String,

        /// Output PNG path
        #[arg(short, long)]
        output: Option<String>,

        /// Number of stars to visualize
        #[arg(long, default_value = "15")]
        num_stars: usize,

        /// PSF fitting type (gaussian or moffat4)
        #[arg(long, default_value = "moffat4")]
        psf_type: String,

        /// Sort criteria (r2, hfr, brightness)
        #[arg(long, default_value = "r2")]
        sort_by: String,

        /// Number of grid columns
        #[arg(long, default_value = "5")]
        grid_cols: usize,

        /// Star selection mode (top, regions, quality, corners)
        #[arg(long, default_value = "corners")]
        selection_mode: String,

        /// Enable verbose debug output
        #[arg(long, short)]
        verbose: bool,
    },

    /// Benchmark PSF fitting performance
    BenchmarkPsf {
        /// Path to FITS file
        fits_path: String,

        /// Number of runs for averaging (default: 5)
        #[arg(long, default_value = "5")]
        runs: usize,

        /// Enable verbose debug output
        #[arg(long, short)]
        verbose: bool,
    },

    /// Start the web server for API access and static file serving
    Server {
        /// Database file to use
        database: String,

        /// Base directory containing the image files
        image_dir: String,

        /// Directory to serve static files from (for React app, optional - uses embedded files if not provided)
        #[arg(long)]
        static_dir: Option<String>,

        /// Cache directory for processed images
        #[arg(long, default_value = "./cache")]
        cache_dir: String,

        /// Port to listen on
        #[arg(short, long, default_value = "3000")]
        port: u16,

        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },
}

#[derive(Parser, Debug, Clone)]
pub struct StatisticalOptions {
    /// Enable statistical analysis
    #[arg(long)]
    pub enable_statistical: bool,

    /// Enable HFR outlier detection
    #[arg(long, requires = "enable_statistical")]
    pub stat_hfr: bool,

    /// Standard deviations for HFR outlier detection
    #[arg(long, default_value = "2.0", requires = "stat_hfr")]
    pub hfr_stddev: f64,

    /// Enable star count outlier detection
    #[arg(long, requires = "enable_statistical")]
    pub stat_stars: bool,

    /// Standard deviations for star count outlier detection
    #[arg(long, default_value = "2.0", requires = "stat_stars")]
    pub star_stddev: f64,

    /// Enable distribution analysis (median/mean shift detection)
    #[arg(long, requires = "enable_statistical")]
    pub stat_distribution: bool,

    /// Percentage threshold for median shift from mean (0.0-1.0)
    #[arg(long, default_value = "0.1", requires = "stat_distribution")]
    pub median_shift_threshold: f64,

    /// Enable cloud detection (sudden rises in median HFR or drops in star count)
    #[arg(long, requires = "enable_statistical")]
    pub stat_clouds: bool,

    /// Percentage threshold for cloud detection (0.0-1.0, e.g. 0.2 = 20% change)
    #[arg(long, default_value = "0.2", requires = "stat_clouds")]
    pub cloud_threshold: f64,

    /// Number of images needed to establish baseline after cloud event
    #[arg(long, default_value = "5", requires = "stat_clouds")]
    pub cloud_baseline_count: usize,
}

impl StatisticalOptions {
    pub fn to_grading_config(&self) -> Option<crate::grading::StatisticalGradingConfig> {
        if self.enable_statistical {
            Some(crate::grading::StatisticalGradingConfig {
                enable_hfr_analysis: self.stat_hfr,
                hfr_stddev_threshold: self.hfr_stddev,
                enable_star_count_analysis: self.stat_stars,
                star_count_stddev_threshold: self.star_stddev,
                enable_distribution_analysis: self.stat_distribution,
                median_shift_threshold: self.median_shift_threshold,
                enable_cloud_detection: self.stat_clouds,
                cloud_threshold: self.cloud_threshold,
                cloud_baseline_count: self.cloud_baseline_count,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_statistical_options_to_grading_config_disabled() {
        let options = StatisticalOptions {
            enable_statistical: false,
            stat_hfr: true,
            hfr_stddev: 2.0,
            stat_stars: true,
            star_stddev: 2.0,
            stat_distribution: true,
            median_shift_threshold: 0.1,
            stat_clouds: true,
            cloud_threshold: 0.2,
            cloud_baseline_count: 5,
        };

        assert!(options.to_grading_config().is_none());
    }

    #[test]
    fn test_statistical_options_to_grading_config_enabled() {
        let options = StatisticalOptions {
            enable_statistical: true,
            stat_hfr: true,
            hfr_stddev: 1.5,
            stat_stars: false,
            star_stddev: 2.5,
            stat_distribution: true,
            median_shift_threshold: 0.15,
            stat_clouds: false,
            cloud_threshold: 0.25,
            cloud_baseline_count: 10,
        };

        let config = options.to_grading_config().unwrap();
        assert!(config.enable_hfr_analysis);
        assert_eq!(config.hfr_stddev_threshold, 1.5);
        assert!(!config.enable_star_count_analysis);
        assert_eq!(config.star_count_stddev_threshold, 2.5);
        assert!(config.enable_distribution_analysis);
        assert_eq!(config.median_shift_threshold, 0.15);
        assert!(!config.enable_cloud_detection);
        assert_eq!(config.cloud_threshold, 0.25);
        assert_eq!(config.cloud_baseline_count, 10);
    }
}
