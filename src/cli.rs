use anyhow::Context;
use clap::{Parser, Subcommand};
use std::time::Duration;

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

    /// Create a new Target Scheduler database and import FITS folders into it.
    ///
    /// Bootstraps a fully-faithful scheduler database (vendored upstream
    /// schema, user_version 23) at the given path, then scans the supplied
    /// directories for light frames and synthesizes projects, targets,
    /// exposure templates and plans from their headers. Frames grade as
    /// Pending; quality backfill runs later (Scan Quality / screen-fits).
    /// Registers the new database in the shared registry unless
    /// `--no-register`.
    CreateDb {
        /// Path of the new .sqlite database file to create.
        database: String,

        /// Directories (or single FITS files) to import.
        #[arg(required = true)]
        directories: Vec<String>,

        /// Display name for the registry entry (defaults to the DB filename).
        #[arg(long)]
        name: Option<String>,

        /// Time gap (days) between frames of the same rig that starts a new
        /// project.
        #[arg(long, default_value_t = crate::commands::import::grouping::DEFAULT_TIME_GAP_DAYS)]
        time_gap_days: f64,

        /// Profile ID to attach imported rows to (defaults to a fresh one).
        #[arg(long)]
        profile_id: Option<String>,

        /// Preview the plan; the database file is still created but the
        /// import transaction is rolled back.
        #[arg(long)]
        dry_run: bool,

        /// Do not register the new database in the shared registry.
        #[arg(long)]
        no_register: bool,

        /// Path to the database registry JSON file (defaults to the platform
        /// config directory). Useful for dev/test isolation.
        #[arg(long)]
        registry: Option<String>,
    },

    /// Import FITS folders into an existing Target Scheduler database.
    ///
    /// Accepts a registry slug or a path to a .sqlite file. Frames whose
    /// basename is already recorded are skipped; remaining frames attach to
    /// EXISTING targets when the OBJECT name or coordinates match, and only
    /// unmatched frames create new projects/targets. Use --dry-run first to
    /// preview exactly what will happen.
    Import {
        /// Registry slug or path of the target database.
        db: String,

        /// Directories (or single FITS files) to import.
        #[arg(required = true)]
        directories: Vec<String>,

        /// Time gap (days) between frames of the same rig that starts a new
        /// project.
        #[arg(long, default_value_t = crate::commands::import::grouping::DEFAULT_TIME_GAP_DAYS)]
        time_gap_days: f64,

        /// Profile ID to attach imported rows to (defaults to the database's
        /// single profile; required if it has several).
        #[arg(long)]
        profile_id: Option<String>,

        /// Preview the plan and roll back without writing.
        #[arg(long)]
        dry_run: bool,

        /// Do NOT attach frames to existing targets — synthesize new
        /// structure for everything (ground-zero import).
        #[arg(long)]
        no_attach: bool,

        /// Coordinate-match radius (degrees) for attaching to an existing
        /// target.
        #[arg(long, default_value_t = crate::commands::import::DEFAULT_MATCH_RADIUS_DEG)]
        match_radius_deg: f64,

        /// Path to the database registry JSON file (defaults to the platform
        /// config directory).
        #[arg(long)]
        registry: Option<String>,
    },

    /// Remove everything a PSF Guard import created from a database.
    ///
    /// Deletes projects whose description carries the `Imported by PSF
    /// Guard` marker, together with their targets, exposure plans, rule
    /// weights, and acquired images. Frames that were ATTACHED to
    /// pre-existing projects are not touched. Recovery hatch for an import
    /// that should not have happened.
    RemoveImported {
        /// Registry slug or path of the database.
        db: String,

        /// Show what would be removed without writing.
        #[arg(long)]
        dry_run: bool,

        /// Path to the database registry JSON file (defaults to the platform
        /// config directory).
        #[arg(long)]
        registry: Option<String>,
    },

    /// Export ("take out") graded lights into a stacking-friendly folder.
    ///
    /// Selects non-rejected images (accepted only by default), locates the
    /// files under the database's image directories, and lays them out
    /// WBPP-style: <dest>/<target>/LIGHT/<filter>/. Re-runs skip files that
    /// already exist with the right size, so an export folder can be topped
    /// up after each session. Rejects are never exported.
    Export {
        /// Registry slug or path of the database.
        db: String,

        /// Destination folder for the export tree.
        #[arg(long)]
        dest: String,

        /// Also export ungraded (Pending) frames.
        #[arg(long)]
        include_pending: bool,

        /// Filter by project name (substring match).
        #[arg(short, long)]
        project: Option<String>,

        /// Filter by target name (substring match).
        #[arg(short, long)]
        target: Option<String>,

        /// Restrict to one filter name (exact, case-insensitive).
        #[arg(long)]
        filter: Option<String>,

        /// Hardlink instead of copy (instant + no extra disk when the
        /// destination is on the same filesystem; falls back to copy).
        #[arg(long)]
        link: bool,

        /// Print the plan without writing anything.
        #[arg(long)]
        dry_run: bool,

        /// Override the image directories to search (defaults to the
        /// registry entry's; required when `db` is a bare .sqlite path).
        #[arg(long, value_delimiter = ',')]
        image_dirs: Option<Vec<String>>,

        /// Path to the database registry JSON file (defaults to the platform
        /// config directory).
        #[arg(long)]
        registry: Option<String>,
    },

    /// List all projects
    ListProjects,

    /// List targets for a specific project
    ListTargets {
        /// Project ID or name
        project: String,
    },

    /// Move rejected files out of the directory tree PixInsight scans.
    ///
    /// Walks `gradingStatus = 2` images for the specified database (selected
    /// from the registry by slug), looks them up on disk under the DB's
    /// configured image_dirs, and moves each one — plus same-stem sidecars —
    /// to `<image_dir>/<P>/REJECT/<rest>` by default. Idempotent: re-runs
    /// skip rows already recorded in the `psf_guard_archive` sibling table.
    ///
    /// See REJECT_ARCHIVE_PLAN.md for the full design. Eventually deprecates
    /// `filter-rejected`.
    MoveRejects {
        /// Slug of the database (from the registry) to operate on.
        #[arg(long)]
        db: String,

        /// Perform a dry run (print the plan; no files moved, no DB writes —
        /// the `psf_guard_archive` table is not created in dry-run mode).
        #[arg(long)]
        dry_run: bool,

        /// Override the archive segment name (default `REJECT`).
        #[arg(long)]
        reject_segment: Option<String>,

        /// Override the depth at which the segment is inserted (default 1 —
        /// just below the project folder).
        #[arg(long)]
        reject_depth: Option<u32>,

        /// Override the sidecar extension list (comma-separated, e.g.
        /// `.xisf,.json,.txt`).
        #[arg(long, value_delimiter = ',')]
        sidecar_exts: Option<Vec<String>>,

        /// Path to the database registry JSON file (defaults to the platform
        /// config directory). Useful for dev/test isolation.
        #[arg(long)]
        registry: Option<String>,

        /// Filter by project name (substring match)
        #[arg(short, long)]
        project: Option<String>,

        /// Filter by target name (substring match)
        #[arg(short, long)]
        target: Option<String>,

        /// Verbose: print per-image trace including paths that didn't match.
        #[arg(short, long)]
        verbose: bool,
    },

    /// Move archived rejects back into the directory tree.
    ///
    /// Reverses `move-rejects`. By default restores only files that are no
    /// longer graded `Rejected` (i.e. you un-rejected them in the UI — to
    /// Accepted or Pending); use `--all` to restore everything in the
    /// archive. Never overwrites: if the original path is occupied, the file
    /// is restored beside it with a `.restored` suffix. On success the
    /// `psf_guard_archive` row is removed and emptied REJECT directories are
    /// pruned (a directory left holding only the manifest is kept).
    RestoreRejects {
        /// Slug of the database (from the registry) to operate on.
        #[arg(long)]
        db: String,

        /// Restore every archived row regardless of current grade.
        #[arg(long)]
        all: bool,

        /// Restore only this acquiredimage Id (regardless of grade).
        #[arg(long)]
        image_id: Option<i64>,

        /// Restore only this acquiredimage guid (regardless of grade).
        #[arg(long)]
        guid: Option<String>,

        /// Perform a dry run (print the plan; no files moved, no DB writes).
        #[arg(long)]
        dry_run: bool,

        /// Path to the database registry JSON file (defaults to the platform
        /// config directory). Useful for dev/test isolation.
        #[arg(long)]
        registry: Option<String>,

        /// Verbose: print per-image trace.
        #[arg(short, long)]
        verbose: bool,
    },

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

    /// Screen FITS frames for occlusion, clouds, pointing and cached satellite risk
    ScreenFits {
        /// Path to a FITS file or directory (searched recursively)
        path: String,

        /// Star detection algorithm to use (nina, hocusfocus)
        #[arg(long, default_value = "hocusfocus")]
        detector: String,

        /// Output format (table, csv, json)
        #[arg(short, long, default_value = "table")]
        format: String,

        /// Quality score below which a frame is rejected (0.0-1.0)
        #[arg(long, default_value = "0.35")]
        min_score: f64,

        /// Rise in dead-cell fraction over baseline that flags occlusion
        /// (0.0-1.0; lower = stricter). Clean-frame jitter is ~0.04.
        #[arg(long, default_value = "0.08")]
        dead_cell_rise: f64,

        /// Worker threads for frame analysis (default: all cores, bounded by
        /// available memory)
        #[arg(long)]
        threads: Option<usize>,

        /// Gap in minutes that splits an imaging session into sequences
        #[arg(long, default_value = "60")]
        session_gap: u64,

        /// Plate solve frames, add cached satellite risk, then write [Auto]
        /// rejections for supported quality/pointing findings into this
        /// scheduler DB (registry slug or path to a .sqlite file). Frames are
        /// matched by FITS filename and capture time. Isolated no-solves and
        /// operational failures are not rejected; already-rejected frames are
        /// left untouched.
        #[arg(long)]
        regrade_db: Option<String>,

        /// With --regrade-db: show what would change without writing
        #[arg(long, requires = "regrade_db")]
        dry_run: bool,

        /// Registry file for resolving --regrade-db slugs (defaults to the
        /// platform config location)
        #[arg(long)]
        registry: Option<String>,

        /// Cache root used by the server (for cached orbital elements and
        /// per-database satellite predictions)
        #[arg(long, default_value = "./cache")]
        cache_dir: String,

        /// Write annotated diagnostic PNGs for WARN/REJECT frames into this
        /// directory (grid overlay showing which cells drove the verdict)
        #[arg(long)]
        annotate: Option<String>,

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

    /// Sync state between two Target Scheduler databases, matched by the stable
    /// `acquiredimage.guid` (TS plugin schema v22+). Two complementary one-way
    /// operations: `sync pull` mirrors structure + captured images from a
    /// telescope DB into your local DB (preserving your local grading), and
    /// `sync grades` pushes your grading decisions from the local DB back into
    /// the telescope DB. Use them together — pull to refresh, grade locally,
    /// push grades back — rather than reversing one direction, which would
    /// overwrite work.
    Sync {
        #[command(subcommand)]
        kind: SyncKind,
    },

    /// Start the web server for API access and static file serving
    Server {
        /// Path to TOML configuration file
        #[arg(short, long)]
        config: Option<String>,

        /// Path to the database registry JSON file (defaults to the platform
        /// config directory; useful for dev/test isolation).
        #[arg(long)]
        registry: Option<String>,

        /// Database file to use. Registered into the registry on first run
        /// so subsequent starts pick it up automatically.
        database: Option<String>,

        /// Base directories containing the image files. Used as the new
        /// database's image directories when registering for the first time.
        image_dirs: Vec<String>,

        /// Directory to serve static files from (for React app, optional - uses embedded files if not provided)
        #[arg(long)]
        static_dir: Option<String>,

        /// Cache directory for processed images (overrides config file)
        #[arg(long)]
        cache_dir: Option<String>,

        /// Port to listen on (overrides config file)
        #[arg(short, long)]
        port: Option<u16>,

        /// Host to bind to (overrides config file; defaults to 0.0.0.0)
        #[arg(long)]
        host: Option<String>,

        /// Enable background pre-generation of screen-sized preview images
        #[arg(long)]
        pregenerate_screen: bool,

        /// Enable background pre-generation of large preview images (2000px max)
        #[arg(long)]
        pregenerate_large: bool,

        /// Enable background pre-generation of original-sized preview images
        #[arg(long)]
        pregenerate_original: bool,

        /// Enable background pre-generation of star-annotated images
        #[arg(long)]
        pregenerate_annotated: bool,

        /// Enable all image pre-generation types (equivalent to all --pregenerate-* flags)
        #[arg(long)]
        pregenerate_all: bool,

        /// Cache expiration time for pre-generated images (default: 1y)
        #[arg(long, default_value = "1y")]
        cache_expiry: String,

        /// Allow HTTP clients to add/edit/remove databases via the
        /// `/api/databases` endpoints. Off by default because the same UI
        /// could let any reachable client mutate the user's configured DB list
        /// and image directories. Enable only when the server is bound to a
        /// trusted interface (e.g. localhost). Tauri mode always enables it.
        #[arg(long)]
        allow_database_management: bool,
    },
}

#[derive(Subcommand)]
pub enum SyncKind {
    /// Push grading state from one database into another (one-way, by guid).
    Grades {
        /// Source database (read-only): a registry slug or a path to a .sqlite file.
        #[arg(long)]
        from: String,

        /// Destination database (written): a registry slug or a path to a .sqlite file.
        #[arg(long)]
        to: String,

        /// Print the plan without writing to the destination.
        #[arg(long)]
        dry_run: bool,

        /// Only push rows whose SOURCE grade is this (pending|accepted|rejected).
        #[arg(long)]
        status: Option<String>,

        /// Restrict to source rows whose project name matches (substring).
        #[arg(short, long)]
        project: Option<String>,

        /// Restrict to source rows whose target name matches (substring).
        #[arg(short, long)]
        target: Option<String>,

        /// Path to the database registry JSON file (only consulted when
        /// --from/--to is a slug; defaults to the platform config directory).
        #[arg(long)]
        registry: Option<String>,

        /// Verbose: print a per-image trace of each grade transition.
        #[arg(short, long)]
        verbose: bool,
    },

    /// Pull structure + captured images FROM a telescope DB INTO our local DB.
    ///
    /// Mirrors projects, targets (coordinates), exposure templates/plans, rule
    /// weights, and acquired images, matched by `guid` (TS schema v22+) with
    /// foreign keys remapped onto the destination's local Ids. The telescope
    /// wins for structure fields, but local grading is preserved: an existing
    /// image keeps its grade unless it is still Pending, in which case it
    /// adopts the telescope's grade. Push grades back with `sync grades`.
    Pull {
        /// Telescope database (source, read-only): a registry slug or a .sqlite path.
        #[arg(long)]
        from: String,

        /// Local database (destination, written): a registry slug or a .sqlite path.
        #[arg(long)]
        to: String,

        /// Print the plan without writing to the destination.
        #[arg(long)]
        dry_run: bool,

        /// Skip copying the (large) imagedata thumbnail BLOBs (copied by default).
        #[arg(long)]
        no_image_data: bool,

        /// Restrict the pull to projects whose name matches (substring);
        /// cascades to their targets, plans, and images.
        #[arg(short, long)]
        project: Option<String>,

        /// Path to the database registry JSON file (only consulted when
        /// --from/--to is a slug; defaults to the platform config directory).
        #[arg(long)]
        registry: Option<String>,

        /// Verbose: print a per-entity trace of inserts/updates.
        #[arg(short, long)]
        verbose: bool,
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

/// Configuration for background image pre-generation
#[derive(Debug, Clone)]
pub struct PregenerationConfig {
    pub screen_enabled: bool,
    pub large_enabled: bool,
    pub original_enabled: bool,
    pub annotated_enabled: bool,
    pub cache_expiry: Duration,
}

impl Default for PregenerationConfig {
    fn default() -> Self {
        Self {
            screen_enabled: false,
            large_enabled: false,
            original_enabled: false,
            annotated_enabled: false,
            cache_expiry: Duration::from_secs(86400 * 365), // 1 year default
        }
    }
}

impl PregenerationConfig {
    /// Create configuration from server command arguments
    pub fn from_server_args(
        pregenerate_screen: bool,
        pregenerate_large: bool,
        pregenerate_original: bool,
        pregenerate_annotated: bool,
        pregenerate_all: bool,
        cache_expiry_str: &str,
    ) -> anyhow::Result<Self> {
        // Parse cache expiry using humantime
        let cache_expiry = humantime::parse_duration(cache_expiry_str)
            .with_context(|| format!("Invalid cache expiry format '{}'", cache_expiry_str))?;

        // If pregenerate_all is true, enable all types
        let (screen, large, original, annotated) = if pregenerate_all {
            (true, true, true, true)
        } else {
            (
                pregenerate_screen,
                pregenerate_large,
                pregenerate_original,
                pregenerate_annotated,
            )
        };

        Ok(Self {
            screen_enabled: screen,
            large_enabled: large,
            original_enabled: original,
            annotated_enabled: annotated,
            cache_expiry,
        })
    }

    /// Create from config module's PregenerationConfig
    pub fn from_config(config: Option<&crate::config::PregenerationConfig>) -> Self {
        if let Some(cfg) = config {
            Self {
                screen_enabled: cfg.enabled.unwrap_or(false) && cfg.screen.unwrap_or(true),
                large_enabled: cfg.enabled.unwrap_or(false) && cfg.large.unwrap_or(false),
                original_enabled: false,  // Not supported in config yet
                annotated_enabled: false, // Not supported in config yet
                cache_expiry: Duration::from_secs(86400 * 365), // 1 year default
            }
        } else {
            Self::default()
        }
    }

    /// Check if any pre-generation is enabled
    pub fn is_enabled(&self) -> bool {
        self.screen_enabled || self.large_enabled || self.original_enabled || self.annotated_enabled
    }

    /// Get list of enabled formats for logging
    pub fn enabled_formats(&self) -> Vec<&'static str> {
        let mut formats = Vec::new();
        if self.screen_enabled {
            formats.push("screen");
        }
        if self.large_enabled {
            formats.push("large");
        }
        if self.original_enabled {
            formats.push("original");
        }
        if self.annotated_enabled {
            formats.push("annotated");
        }
        formats
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
