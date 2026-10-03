//! Folder layout below each database's storage roots.
//!
//! A database keeps three roots on [`DatabaseContext`]:
//! - `cache_dir_path`: per-image results (previews, star lists, astrometry,
//!   satellites, spatial metrics, stats);
//! - `stack_root`: [`STACKS`], [`STACK_PROCESSING`] and [`WBPP_RUNS`];
//! - `calibration_root`: [`CALIBRATION_MASTERS`].
//!
//! Every path below a root goes through this module, so a folder name lives
//! in one place.
//!
//! [`DatabaseContext`]: super::database_context::DatabaseContext

use std::path::{Path, PathBuf};

/// Stack outputs and their indices, below the stack root.
pub const STACKS: &str = "stack-previews";
/// Saved stretch and processing choices per stack, below the stack root.
pub const STACK_PROCESSING: &str = "stack-processing";
/// WBPP work folders when neither the settings nor the database name one,
/// below the stack root.
pub const WBPP_RUNS: &str = "wbpp";
/// Built calibration masters, below the calibration root.
pub const CALIBRATION_MASTERS: &str = "calibration-masters";

/// Image preview folders below the cache root, which the disk budget culls
/// first.
pub const PREVIEW_CATEGORIES: [&str; 3] = ["previews", "annotated", "stars"];
/// Preview folders whose files carry the configured image encoding.
pub const ENCODED_PREVIEW_CATEGORIES: [&str; 2] = ["previews", "annotated"];

/// Folders below [`STACKS`] that are not stack jobs.
pub mod stack_kind {
    pub const COLOR: &str = "color";
    pub const COLOR_INPUTS: &str = "color-inputs";
    pub const STRETCH: &str = "stretch";
    pub const DECONVOLUTION: &str = "deconvolution";
    pub const RC_ASTRO: &str = "rc-astro";
    pub const ARTIFACT_SEARCHES: &str = "artifact-searches";
    pub const REFERENCE_SCORES: &str = "reference-scores";
    pub const RESUME: &str = "resume";
}

/// `<stack_root>/stack-previews`.
pub fn stacks(stack_root: &Path) -> PathBuf {
    stack_root.join(STACKS)
}

/// `<stack_root>/stack-previews/<kind>`, for one of [`stack_kind`].
pub fn stack_folder(stack_root: &Path, kind: &str) -> PathBuf {
    stacks(stack_root).join(kind)
}

/// `<stack_root>/stack-processing`.
pub fn stack_processing(stack_root: &Path) -> PathBuf {
    stack_root.join(STACK_PROCESSING)
}

/// `<calibration_root>/calibration-masters`.
pub fn calibration_masters(calibration_root: &Path) -> PathBuf {
    calibration_root.join(CALIBRATION_MASTERS)
}

/// The three folders a server keeps its generated files in. Each database
/// gets its own slug folder below every one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageRoots {
    pub cache: PathBuf,
    pub stacks: PathBuf,
    pub calibration: PathBuf,
}

impl StorageRoots {
    /// Everything in one folder, as before the folders could be split.
    pub fn single(cache: impl Into<PathBuf>) -> Self {
        let cache = cache.into();
        Self {
            stacks: cache.clone(),
            calibration: cache.clone(),
            cache,
        }
    }

    pub fn get(&self, kind: StorageKind) -> &Path {
        match kind {
            StorageKind::Cache => &self.cache,
            StorageKind::Stacks => &self.stacks,
            StorageKind::Calibration => &self.calibration,
        }
    }

    pub fn set(&mut self, kind: StorageKind, path: PathBuf) {
        match kind {
            StorageKind::Cache => self.cache = path,
            StorageKind::Stacks => self.stacks = path,
            StorageKind::Calibration => self.calibration = path,
        }
    }
}

impl From<String> for StorageRoots {
    fn from(cache: String) -> Self {
        Self::single(cache)
    }
}

impl From<&str> for StorageRoots {
    fn from(cache: &str) -> Self {
        Self::single(cache)
    }
}

impl From<PathBuf> for StorageRoots {
    fn from(cache: PathBuf) -> Self {
        Self::single(cache)
    }
}

impl From<&Path> for StorageRoots {
    fn from(cache: &Path) -> Self {
        Self::single(cache)
    }
}

/// Which of the three folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    Cache,
    Stacks,
    Calibration,
}

impl StorageKind {
    pub const ALL: [StorageKind; 3] = [
        StorageKind::Cache,
        StorageKind::Stacks,
        StorageKind::Calibration,
    ];

    /// The folders this kind owns below each database's slug folder. The
    /// cache owns everything the other two do not.
    pub fn database_folders(self) -> &'static [&'static str] {
        match self {
            StorageKind::Cache => &[],
            StorageKind::Stacks => &[STACKS, STACK_PROCESSING, WBPP_RUNS],
            StorageKind::Calibration => &[CALIBRATION_MASTERS],
        }
    }

    /// Lower-case, for messages.
    pub fn label(self) -> &'static str {
        match self {
            StorageKind::Cache => "cache",
            StorageKind::Stacks => "stacks",
            StorageKind::Calibration => "calibration masters",
        }
    }
}

/// The storage folders the server config file or command line fixes. They
/// win over Settings; whatever neither names falls back to the cache, and
/// the cache to `default_cache`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    /// `./cache` for the server, the platform cache folder for the desktop
    /// app.
    pub default_cache: PathBuf,
    pub cache: Option<PathBuf>,
    pub stacks: Option<PathBuf>,
    pub calibration: Option<PathBuf>,
}

/// Where a folder in use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderSource {
    /// The server config file or command line; Settings cannot change it.
    ServerConfig,
    Settings,
    Default,
}

impl StorageConfig {
    /// Nothing fixed: every folder is `default_cache` unless Settings names
    /// another.
    pub fn defaulting_to(default_cache: impl Into<PathBuf>) -> Self {
        Self {
            default_cache: default_cache.into(),
            cache: None,
            stacks: None,
            calibration: None,
        }
    }

    fn fixed(&self, kind: StorageKind) -> Option<&Path> {
        match kind {
            StorageKind::Cache => self.cache.as_deref(),
            StorageKind::Stacks => self.stacks.as_deref(),
            StorageKind::Calibration => self.calibration.as_deref(),
        }
    }

    /// Whether the config file or command line fixes this folder.
    pub fn is_fixed(&self, kind: StorageKind) -> bool {
        self.fixed(kind).is_some()
    }

    /// The folders to use, and where each came from, given what Settings
    /// chose.
    pub fn resolve(
        &self,
        settings: Option<&crate::db_registry::StorageSettings>,
    ) -> (StorageRoots, [FolderSource; 3]) {
        let chosen = |kind: StorageKind| {
            settings
                .and_then(|settings| match kind {
                    StorageKind::Cache => settings.cache_dir.as_deref(),
                    StorageKind::Stacks => settings.stack_dir.as_deref(),
                    StorageKind::Calibration => settings.calibration_dir.as_deref(),
                })
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        };
        let mut sources = [FolderSource::Default; 3];
        let mut pick = |kind: StorageKind, fallback: &Path| {
            let index = kind as usize;
            if let Some(path) = self.fixed(kind) {
                sources[index] = FolderSource::ServerConfig;
                path.to_path_buf()
            } else if let Some(path) = chosen(kind) {
                sources[index] = FolderSource::Settings;
                path
            } else {
                fallback.to_path_buf()
            }
        };
        let cache = pick(StorageKind::Cache, &self.default_cache);
        let roots = StorageRoots {
            stacks: pick(StorageKind::Stacks, &cache),
            calibration: pick(StorageKind::Calibration, &cache),
            cache,
        };
        (roots, sources)
    }
}

/// The moves to record when Settings saves new folders: one per folder
/// whose next path differs from the one in use, unless the config file
/// fixes that folder. Paths are absolute, so a changed working directory
/// cannot make a move look necessary or unnecessary.
pub fn planned_moves(
    in_use: &StorageRoots,
    next: &StorageRoots,
    sources: &[FolderSource; 3],
) -> Vec<crate::db_registry::StorageMove> {
    let text = |path: &Path| relocate::normalized(path).to_string_lossy().into_owned();
    StorageKind::ALL
        .into_iter()
        .filter(|kind| sources[*kind as usize] != FolderSource::ServerConfig)
        .filter(|kind| !relocate::same_folder(in_use.get(*kind), next.get(*kind)))
        .map(|kind| crate::db_registry::StorageMove {
            kind,
            from: text(in_use.get(kind)),
            to: text(next.get(kind)),
        })
        .collect()
}

/// What the server starts with, after any folder move.
#[derive(Debug)]
pub struct StartupStorage {
    pub roots: StorageRoots,
    /// One line per folder moved or kept back.
    pub notes: Vec<String>,
    /// Calibration master folders that moved, per database slug.
    pub calibration_moves: Vec<(String, PathBuf, PathBuf)>,
}

/// Work out the folders to run with and carry out the moves Settings asked
/// for. Runs before the databases open. Never fails the start: a folder that
/// cannot move or be created stays where it was, with a note saying why.
///
/// `allow_moves` is false for a server restarted inside a running desktop
/// app: the old server's background work may still be writing, so its moves
/// wait for the next full start and the folders they leave stay in use.
pub fn prepare(
    config: &StorageConfig,
    registry_path: Option<&Path>,
    allow_moves: bool,
) -> StartupStorage {
    let mut notes = Vec::new();
    let mut registry = match registry_path.map(crate::db_registry::DbRegistry::load_or_init) {
        Some(Ok(registry)) => Some(registry),
        Some(Err(error)) => {
            notes.push(format!(
                "Could not read the settings file, so folders chosen in Settings are not \
                 used: {error:#}"
            ));
            None
        }
        None => None,
    };
    let settings = registry
        .as_ref()
        .and_then(|registry| registry.storage.as_ref());
    let (wanted, sources) = config.resolve(settings);
    let recorded = settings
        .map(|settings| settings.moves.clone())
        .unwrap_or_default();

    let mut roots = wanted.clone();
    let mut moves = Vec::new();
    let mut kept = Vec::new();
    for pending in &recorded {
        // A folder the config file now fixes, or a choice changed since,
        // makes the recorded move moot.
        if sources[pending.kind as usize] == FolderSource::ServerConfig
            || !relocate::same_folder(Path::new(&pending.to), wanted.get(pending.kind))
        {
            continue;
        }
        let step = relocate::Move {
            kind: pending.kind,
            from: pending.from.clone().into(),
            to: pending.to.clone().into(),
        };
        if allow_moves {
            moves.push(step);
        } else {
            notes.push(format!(
                "The {} stay in {} until PSF Guard restarts fully; then they move to {}.",
                pending.kind.label(),
                pending.from,
                pending.to
            ));
            roots.set(pending.kind, step.from);
            kept.push(pending.clone());
        }
    }
    let relocated = relocate::relocate(&moves, &mut roots);
    notes.extend(relocated.notes);
    for step in &moves {
        if !relocated.finished.contains(&step.kind) {
            kept.extend(
                recorded
                    .iter()
                    .filter(|pending| pending.kind == step.kind)
                    .cloned(),
            );
        }
    }

    for kind in StorageKind::ALL {
        let path = roots.get(kind).to_path_buf();
        if let Err(error) = std::fs::create_dir_all(&path) {
            notes.push(format!(
                "Could not create the {} folder {}: {error}",
                kind.label(),
                path.display()
            ));
        }
    }

    if let (Some(path), Some(registry)) = (registry_path, registry.as_mut())
        && kept != recorded
        && let Some(storage) = registry.storage.as_mut()
    {
        storage.moves = kept;
        if storage.is_empty() {
            registry.storage = None;
        }
        if let Err(error) = registry.save(path) {
            notes.push(format!(
                "Could not record the finished moves in {}: {error}",
                path.display()
            ));
        }
    }
    StartupStorage {
        roots,
        notes,
        calibration_moves: relocated.calibration_moves,
    }
}

pub mod relocate;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_registry::{DbRegistry, StorageMove, StorageSettings};

    #[test]
    fn the_config_file_wins_then_settings_then_the_cache() {
        let config = StorageConfig {
            calibration: Some("/fixed/masters".into()),
            ..StorageConfig::defaulting_to("/default")
        };
        let chosen = StorageSettings {
            cache_dir: Some("/chosen/cache".into()),
            calibration_dir: Some("/chosen/masters".into()),
            ..Default::default()
        };

        let (roots, sources) = config.resolve(Some(&chosen));

        assert_eq!(roots.cache, Path::new("/chosen/cache"));
        assert_eq!(roots.stacks, Path::new("/chosen/cache"));
        assert_eq!(roots.calibration, Path::new("/fixed/masters"));
        assert_eq!(
            sources,
            [
                FolderSource::Settings,
                FolderSource::Default,
                FolderSource::ServerConfig
            ]
        );
        let (defaults, _) = config.resolve(None);
        assert_eq!(defaults.cache, Path::new("/default"));
    }

    #[test]
    fn a_new_cache_plans_moves_for_every_folder_that_follows_it() {
        let in_use = StorageRoots::single("/old");
        let next = StorageRoots {
            calibration: "/masters".into(),
            ..StorageRoots::single("/new")
        };
        let sources = [
            FolderSource::Settings,
            FolderSource::Default,
            FolderSource::ServerConfig,
        ];

        let moves = planned_moves(&in_use, &next, &sources);

        assert_eq!(
            moves.iter().map(|step| step.kind).collect::<Vec<_>>(),
            vec![StorageKind::Cache, StorageKind::Stacks]
        );
        assert_eq!(moves[0].from, "/old");
        assert_eq!(moves[0].to, "/new");
    }

    fn registry_with(path: &Path, storage: StorageSettings) {
        DbRegistry {
            storage: Some(storage),
            ..Default::default()
        }
        .save(path)
        .unwrap();
    }

    #[test]
    fn a_recorded_move_runs_at_the_next_start_and_is_then_forgotten() {
        let temp = tempfile::tempdir().unwrap();
        let registry_path = temp.path().join("config.json");
        let cache = temp.path().join("cache");
        let stacks = temp.path().join("stacks");
        std::fs::create_dir_all(cache.join("db/stack-previews/job")).unwrap();
        std::fs::write(cache.join("db/stack-previews/job/manifest.json"), "{}").unwrap();
        registry_with(
            &registry_path,
            StorageSettings {
                stack_dir: Some(stacks.to_string_lossy().into_owned()),
                moves: vec![StorageMove {
                    kind: StorageKind::Stacks,
                    from: cache.to_string_lossy().into_owned(),
                    to: stacks.to_string_lossy().into_owned(),
                }],
                ..Default::default()
            },
        );
        let config = StorageConfig::defaulting_to(&cache);

        let started = prepare(&config, Some(&registry_path), true);

        assert_eq!(started.roots.stacks, stacks);
        assert_eq!(started.notes.len(), 1, "{:?}", started.notes);
        assert!(stacks.join("db/stack-previews/job/manifest.json").is_file());
        let storage = DbRegistry::load_or_init(&registry_path)
            .unwrap()
            .storage
            .unwrap();
        assert!(storage.moves.is_empty());
        assert_eq!(
            storage.stack_dir,
            Some(stacks.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn a_restart_inside_the_app_keeps_the_old_folder_until_a_full_start() {
        let temp = tempfile::tempdir().unwrap();
        let registry_path = temp.path().join("config.json");
        let cache = temp.path().join("cache");
        let masters = temp.path().join("masters");
        std::fs::create_dir_all(cache.join("db/calibration-masters")).unwrap();
        std::fs::write(cache.join("db/calibration-masters/dark-a.fits"), "m").unwrap();
        registry_with(
            &registry_path,
            StorageSettings {
                calibration_dir: Some(masters.to_string_lossy().into_owned()),
                moves: vec![StorageMove {
                    kind: StorageKind::Calibration,
                    from: cache.to_string_lossy().into_owned(),
                    to: masters.to_string_lossy().into_owned(),
                }],
                ..Default::default()
            },
        );
        let config = StorageConfig::defaulting_to(&cache);

        let restarted = prepare(&config, Some(&registry_path), false);
        assert_eq!(restarted.roots.calibration, cache);
        assert!(cache.join("db/calibration-masters/dark-a.fits").is_file());

        let started = prepare(&config, Some(&registry_path), true);
        assert_eq!(started.roots.calibration, masters);
        assert!(masters.join("db/calibration-masters/dark-a.fits").is_file());
        assert_eq!(started.calibration_moves.len(), 1);
    }

    #[test]
    fn a_folder_changed_on_the_command_line_moves_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let registry_path = temp.path().join("config.json");
        let old = temp.path().join("old");
        std::fs::create_dir_all(old.join("db/previews")).unwrap();
        std::fs::write(old.join("db/previews/1.png"), "p").unwrap();
        let config = StorageConfig {
            cache: Some(temp.path().join("new")),
            ..StorageConfig::defaulting_to(&old)
        };

        let started = prepare(&config, Some(&registry_path), true);

        assert_eq!(started.roots, StorageRoots::single(temp.path().join("new")));
        assert!(old.join("db/previews/1.png").is_file());
        assert!(started.notes.is_empty(), "{:?}", started.notes);
    }
}
