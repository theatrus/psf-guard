//! Backups of PSF Guard's own state and of the catalogs it extends.
//!
//! One archive is a zip: `manifest.json`, a short `README.txt` on restoring,
//! PSF Guard's settings under `meta/` and, for a full backup, each
//! registered catalog under `catalogs/<slug>/`. SQLite files are copied with
//! SQLite's online backup ([`sqlite_copy`]), never by copying the file, so a
//! catalog N.I.N.A. is writing is copied consistently. The archive is written
//! under a temporary name in the destination and renamed once complete.
//!
//! The CLI's `psf-guard backup` and the server's backup job share this
//! module. See `docs/BACKUPS.md`.

pub mod schedule;
pub mod secrets;
pub mod sqlite_copy;

use crate::db_registry::DbRegistry;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use sqlite_copy::Cancelled;

pub const FORMAT: &str = "psf-guard-backup";
pub const FORMAT_VERSION: u32 = 1;
/// Every archive this module writes starts with this.
pub const FILE_PREFIX: &str = "psf-guard-backup-";
/// Work in progress: a staging folder and the archive before its rename.
pub const TEMP_PREFIX: &str = ".psf-guard-backup-";
/// A temporary file this old belongs to a run that died; pruning removes it.
pub const STALE_TEMP_AGE: Duration = Duration::from_secs(24 * 3600);
const MANIFEST: &str = "manifest.json";
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
/// Pause between online-backup steps, so writers get the catalog back.
pub const SQLITE_STEP_PAUSE: Duration = Duration::from_millis(20);

/// What a backup holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum BackupScope {
    /// PSF Guard's settings, accounts and stores: small.
    Meta,
    /// The settings and every registered catalog.
    #[default]
    Full,
}

impl BackupScope {
    pub fn label(self) -> &'static str {
        match self {
            BackupScope::Meta => "meta",
            BackupScope::Full => "full",
        }
    }
}

/// Who asked for a backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// "Back up now" in Settings.
    Manual,
    /// The periodic schedule. Only these are ever pruned.
    Schedule,
    /// `psf-guard backup`.
    Cli,
}

/// What a file in the archive is to PSF Guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    /// The database registry (`config.json`): catalogs, settings, peers.
    Registry,
    /// Browser accounts: password hashes and API token hashes.
    Auth,
    UserPreferences,
    ProcessingSetups,
    /// Collaboration service tokens, in the clear.
    CollaborationCredentials,
    /// The desktop app's own settings.
    DesktopSettings,
    /// Director's planning store.
    DirectorMeta,
    /// The server's TOML configuration file.
    ServerConfig,
    /// The record of remote sync actions.
    RemoteAudit,
    /// A registered Target Scheduler catalog.
    Catalog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Sqlite,
    Json,
    Toml,
    Text,
}

/// Where PSF Guard keeps its own state, beyond what the registry names.
#[derive(Debug, Clone, Default)]
pub struct Locations {
    pub registry: PathBuf,
    /// The TOML file a server started with `--config`.
    pub server_config: Option<PathBuf>,
    /// Director's store, when not the default beside the registry.
    pub director_meta: Option<PathBuf>,
    /// The cache folder, which holds the remote sync audit log.
    pub cache_root: Option<PathBuf>,
}

impl Locations {
    pub fn beside_registry(registry: impl Into<PathBuf>) -> Self {
        Self {
            registry: registry.into(),
            ..Default::default()
        }
    }

    /// Every file PSF Guard keeps for itself, whether or not it exists yet.
    pub fn meta_files(&self) -> Vec<(FileRole, FileKind, PathBuf)> {
        let registry = &self.registry;
        let mut files = vec![
            (FileRole::Registry, FileKind::Json, registry.clone()),
            (
                FileRole::Auth,
                FileKind::Json,
                crate::auth_registry::AuthRegistry::path_for_database_registry(registry),
            ),
            (
                FileRole::UserPreferences,
                FileKind::Json,
                crate::user_preferences::UserPreferencesRegistry::path_for_database_registry(
                    registry,
                ),
            ),
            (
                FileRole::ProcessingSetups,
                FileKind::Json,
                crate::processing_setups::ProcessingSetupsRegistry::path_for_database_registry(
                    registry,
                ),
            ),
            (
                FileRole::CollaborationCredentials,
                FileKind::Json,
                crate::server::director::collaboration_credentials_path(registry),
            ),
            // `desktop_server::Settings::path`, which only the desktop build has.
            (
                FileRole::DesktopSettings,
                FileKind::Json,
                registry.with_file_name("desktop.json"),
            ),
            (
                FileRole::DirectorMeta,
                FileKind::Sqlite,
                self.director_meta
                    .clone()
                    .unwrap_or_else(|| crate::server::director::default_meta_path(registry)),
            ),
        ];
        if let Some(config) = &self.server_config {
            files.push((FileRole::ServerConfig, FileKind::Toml, config.clone()));
        }
        if let Some(cache) = &self.cache_root {
            files.push((
                FileRole::RemoteAudit,
                FileKind::Text,
                crate::server::remote_audit::RemoteAuditLog::path_in(cache),
            ));
        }
        files
    }
}

/// One file the backup copies.
#[derive(Debug, Clone, Serialize)]
pub struct PlanItem {
    pub role: FileRole,
    pub kind: FileKind,
    pub source: PathBuf,
    pub archive_path: String,
    /// On disk now, a SQLite file's write-ahead log included.
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    /// Secrets this copy leaves out, in words.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub redacts: Vec<String>,
}

/// A file PSF Guard keeps that the backup leaves out, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Omitted {
    pub role: FileRole,
    pub source: String,
    pub reason: String,
}

/// A registered catalog, as the archive records it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogRecord {
    pub id: String,
    pub name: String,
    pub db_path: String,
    #[serde(default)]
    pub image_dirs: Vec<String>,
    pub included: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// What a backup will copy, before it copies anything. A dry run prints it.
#[derive(Debug, Clone, Serialize)]
pub struct BackupPlan {
    pub scope: BackupScope,
    pub include_secrets: bool,
    pub items: Vec<PlanItem>,
    pub omitted: Vec<Omitted>,
    pub catalogs: Vec<CatalogRecord>,
    /// Which install made the archive; pruning touches only its own.
    pub install_id: String,
}

/// The install a backup belongs to: the id the schedule saved in the
/// registry, or, before one is saved, one derived from the registry's path
/// and the machine's name. Two installs sharing a backup folder each prune
/// only their own archives.
pub fn install_id(registry: &DbRegistry, registry_path: &Path) -> String {
    if let Some(id) = registry
        .backups
        .as_ref()
        .and_then(|settings| settings.install_id.as_deref())
        .filter(|id| !id.trim().is_empty())
    {
        return id.to_string();
    }
    let path = dunce::canonicalize(registry_path).unwrap_or_else(|_| registry_path.to_path_buf());
    let machine = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update([0]);
    hasher.update(machine.as_bytes());
    format!("path-{}", &hex(&hasher.finalize())[..16])
}

impl BackupPlan {
    pub fn total_bytes(&self) -> u64 {
        self.items.iter().map(|item| item.bytes).sum()
    }
}

/// What the plan copies from the registry's folder and the catalogs.
pub fn plan(
    locations: &Locations,
    registry: &DbRegistry,
    scope: BackupScope,
    include_secrets: bool,
) -> BackupPlan {
    let mut items = Vec::new();
    let mut omitted = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    for (role, kind, path) in locations.meta_files() {
        if !path.is_file() {
            continue;
        }
        if role == FileRole::CollaborationCredentials && !include_secrets {
            omitted.push(Omitted {
                role,
                source: path.display().to_string(),
                reason: "holds collaboration service tokens; back up with secrets to keep it"
                    .into(),
            });
            continue;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let archive_path = unique(&mut names, format!("meta/{name}"));
        let redacts = if include_secrets {
            Vec::new()
        } else {
            secrets::would_redact(role, &path)
        };
        items.push(PlanItem {
            role,
            kind,
            bytes: if kind == FileKind::Sqlite {
                sqlite_copy::size_on_disk(&path)
            } else {
                std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            },
            source: path,
            archive_path,
            catalog_id: None,
            redacts,
        });
    }
    let mut catalogs = Vec::new();
    for entry in &registry.databases {
        let path = PathBuf::from(&entry.db_path);
        let mut record = CatalogRecord {
            id: entry.id.clone(),
            name: entry.name.clone(),
            db_path: entry.db_path.clone(),
            image_dirs: entry.image_dirs.clone(),
            included: false,
            archive_path: None,
            problem: None,
        };
        if scope == BackupScope::Full {
            if path.is_file() {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "catalog.sqlite".into());
                let archive_path = unique(&mut names, format!("catalogs/{}/{name}", entry.id));
                record.included = true;
                record.archive_path = Some(archive_path.clone());
                items.push(PlanItem {
                    role: FileRole::Catalog,
                    kind: FileKind::Sqlite,
                    bytes: sqlite_copy::size_on_disk(&path),
                    source: path,
                    archive_path,
                    catalog_id: Some(entry.id.clone()),
                    redacts: Vec::new(),
                });
            } else {
                record.problem = Some(format!("{} is not reachable", entry.db_path));
            }
        }
        catalogs.push(record);
    }
    BackupPlan {
        scope,
        include_secrets,
        items,
        omitted,
        catalogs,
        install_id: install_id(registry, &locations.registry),
    }
}

fn unique(names: &mut std::collections::BTreeSet<String>, wanted: String) -> String {
    if names.insert(wanted.clone()) {
        return wanted;
    }
    let (stem, extension) = match wanted.rsplit_once('.') {
        Some((stem, extension)) if !stem.ends_with('/') => {
            (stem.to_string(), format!(".{extension}"))
        }
        _ => (wanted.clone(), String::new()),
    };
    (2..)
        .map(|n| format!("{stem}-{n}{extension}"))
        .find(|candidate| names.insert(candidate.clone()))
        .expect("an unused name")
}

/// One file in the archive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub role: FileRole,
    pub kind: FileKind,
    /// Where it came from, and where a restore puts it back.
    pub source: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sqlite: Option<sqlite_copy::SqliteInfo>,
    /// Secrets removed from this copy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redacted: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretsMode {
    Redacted,
    Included,
}

/// `manifest.json`: what the archive holds and where each file came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub format_version: u32,
    pub psf_guard_version: String,
    /// RFC 3339, UTC.
    pub created_at: String,
    pub created_unix: i64,
    pub trigger: Trigger,
    pub scope: BackupScope,
    /// The install that made it; pruning touches only its own archives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
    pub secrets: SecretsMode,
    /// What the secrets setting means for this archive, in words.
    pub secrets_note: String,
    pub files: Vec<ManifestFile>,
    #[serde(default)]
    pub catalogs: Vec<CatalogRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<Omitted>,
    /// Files that could not be copied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
    /// What a backup never holds, because PSF Guard can make it again.
    #[serde(default)]
    pub not_included: Vec<String>,
}

/// What [`write`] reports as it goes.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Progress {
    /// `copying`, `compressing` or `finishing`.
    pub stage: &'static str,
    /// The file at hand, as a person reads it.
    pub current: Option<String>,
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// How [`write`] meets the world while it runs.
pub struct Hooks<'a> {
    /// True once the backup should stop. Checked between steps.
    pub cancelled: &'a dyn Fn() -> bool,
    /// Called before each file and each SQLite step; may block while other
    /// work goes first.
    pub yield_now: &'a dyn Fn(),
    pub progress: &'a dyn Fn(&Progress),
    pub sqlite_pause: Duration,
}

impl Hooks<'_> {
    /// No yielding, no reporting, never cancelled.
    pub fn quiet() -> Hooks<'static> {
        Hooks {
            cancelled: &|| false,
            yield_now: &|| {},
            progress: &|_| {},
            sqlite_pause: SQLITE_STEP_PAUSE,
        }
    }
}

/// A finished archive.
#[derive(Debug, Clone)]
pub struct Written {
    pub path: PathBuf,
    pub file_name: String,
    pub bytes: u64,
    pub manifest: Manifest,
}

/// The archive's name: `psf-guard-backup-2026-10-16T130000Z-full.zip`, with
/// `-auto` for a scheduled one.
pub fn archive_name(created: DateTime<Utc>, scope: BackupScope, trigger: Trigger) -> String {
    format!(
        "{FILE_PREFIX}{}-{}{}.zip",
        created.format("%Y-%m-%dT%H%M%SZ"),
        scope.label(),
        if trigger == Trigger::Schedule {
            "-auto"
        } else {
            ""
        }
    )
}

/// Whether `name` is shaped like an archive this module writes.
pub fn is_archive_name(name: &str) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"^psf-guard-backup-\d{4}-\d{2}-\d{2}T\d{6}Z-(meta|full)(-auto)?(-\d+)?\.zip$",
        )
        .unwrap()
    });
    PATTERN.is_match(name)
}

/// Copy everything `plan` names into one archive in `destination`.
pub fn write(
    plan: &BackupPlan,
    destination: &Path,
    trigger: Trigger,
    created: DateTime<Utc>,
    hooks: &Hooks,
) -> Result<Written> {
    prepare_destination(destination, trigger)?;
    remove_stale_temporaries(destination);
    let staging = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempdir_in(destination)
        .with_context(|| format!("writing in {}", destination.display()))?;
    let mut archive_file = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .suffix(".zip.partial")
        .tempfile_in(destination)
        .with_context(|| format!("writing in {}", destination.display()))?;

    // A SQLite file is read twice (copy, then compress); others once.
    let weight = |item: &PlanItem| match item.kind {
        FileKind::Sqlite => item.bytes.max(1) * 2,
        _ => item.bytes.max(1),
    };
    let mut progress = Progress {
        stage: "copying",
        files_total: plan.items.len(),
        bytes_total: plan.items.iter().map(weight).sum(),
        ..Default::default()
    };
    let cancelled = || -> Result<()> {
        if (hooks.cancelled)() {
            Err(Cancelled.into())
        } else {
            Ok(())
        }
    };

    let mut zip = zip::ZipWriter::new(archive_file.as_file_mut());
    let mut files = Vec::new();
    // A catalog the plan could not reach is a problem of this backup too,
    // so the schedule does not prune on its account.
    let mut problems: Vec<String> = plan
        .catalogs
        .iter()
        .filter_map(|catalog| {
            catalog
                .problem
                .as_ref()
                .map(|problem| format!("catalog {}: {problem}", catalog.id))
        })
        .collect();
    let mut catalogs = plan.catalogs.clone();
    for (index, item) in plan.items.iter().enumerate() {
        cancelled()?;
        (hooks.yield_now)();
        cancelled()?;
        let label = describe(item);
        progress.current = Some(label.clone());
        progress.stage = "copying";
        (hooks.progress)(&progress);
        let base = progress.bytes_done;
        let staged = staging.path().join(format!("{index}"));
        let prepared = match item.kind {
            FileKind::Sqlite => {
                let step_hooks = sqlite_copy::StepHooks {
                    pause: hooks.sqlite_pause,
                    cancelled: hooks.cancelled,
                    before_step: hooks.yield_now,
                    progress: &|done, total| {
                        let mut now = progress.clone();
                        now.bytes_done =
                            base + (item.bytes.max(1) * done).checked_div(total).unwrap_or(0);
                        (hooks.progress)(&now);
                    },
                };
                sqlite_copy::copy_online(&item.source, &staged, &step_hooks)
                    .and_then(|_| sqlite_copy::inspect(&staged))
                    .map(|info| (Some(info), Vec::new()))
            }
            _ => std::fs::read(&item.source)
                .with_context(|| format!("reading {}", item.source.display()))
                .and_then(|bytes| {
                    let (bytes, redacted) = if plan.include_secrets {
                        (bytes, Vec::new())
                    } else {
                        secrets::redact(item.role, &bytes)?
                    };
                    std::fs::write(&staged, bytes)
                        .with_context(|| format!("staging {}", item.source.display()))?;
                    Ok((None, redacted))
                }),
        };
        let (sqlite, redacted) = match prepared {
            Ok(prepared) => prepared,
            Err(error) if error.downcast_ref::<Cancelled>().is_some() => return Err(error),
            Err(error) => {
                let message = format!("{label}: {error:#}");
                tracing::warn!("Backup left out {message}");
                problems.push(message.clone());
                if let Some(record) = catalogs
                    .iter_mut()
                    .find(|record| Some(&record.id) == item.catalog_id.as_ref())
                {
                    record.included = false;
                    record.archive_path = None;
                    record.problem = Some(format!("{error:#}"));
                }
                progress.files_done += 1;
                progress.bytes_done = base + weight(item);
                continue;
            }
        };
        if item.kind == FileKind::Sqlite {
            progress.bytes_done = base + item.bytes.max(1);
        }
        progress.stage = "compressing";
        (hooks.progress)(&progress);
        let staged_bytes = std::fs::metadata(&staged)?.len();
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(staged_bytes >= u64::from(u32::MAX));
        zip.start_file(item.archive_path.as_str(), options)?;
        let mut reader = std::fs::File::open(&staged)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        let compress_base = progress.bytes_done;
        let mut read_total = 0u64;
        loop {
            cancelled()?;
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            zip.write_all(&buffer[..read])?;
            read_total += read as u64;
            let mut now = progress.clone();
            let share = if item.kind == FileKind::Sqlite {
                item.bytes.max(1)
            } else {
                weight(item)
            };
            now.bytes_done = compress_base + share * read_total / staged_bytes.max(1);
            (hooks.progress)(&now);
        }
        drop(reader);
        let _ = std::fs::remove_file(&staged);
        files.push(ManifestFile {
            path: item.archive_path.clone(),
            role: item.role,
            kind: item.kind,
            source: item.source.display().to_string(),
            bytes: staged_bytes,
            sha256: hex(&hasher.finalize()),
            catalog_id: item.catalog_id.clone(),
            sqlite,
            redacted,
        });
        progress.files_done += 1;
        progress.bytes_done = base + weight(item);
        (hooks.progress)(&progress);
    }
    cancelled()?;

    progress.stage = "finishing";
    progress.current = None;
    (hooks.progress)(&progress);
    let manifest = Manifest {
        format: FORMAT.into(),
        format_version: FORMAT_VERSION,
        psf_guard_version: env!("CARGO_PKG_VERSION").into(),
        created_at: created.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        created_unix: created.timestamp(),
        trigger,
        scope: plan.scope,
        install_id: Some(plan.install_id.clone()),
        secrets: if plan.include_secrets {
            SecretsMode::Included
        } else {
            SecretsMode::Redacted
        },
        secrets_note: if plan.include_secrets {
            "Holds peer keys, collaboration tokens and server tokens in the clear. Keep it \
             private."
                .into()
        } else {
            "Peer keys, collaboration tokens and server tokens were removed; enter them \
             again after a restore. Account password and token hashes are kept."
                .into()
        },
        files,
        catalogs,
        omitted: plan.omitted.clone(),
        problems,
        not_included: NOT_INCLUDED.iter().map(|line| line.to_string()).collect(),
    };
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("README.txt", options)?;
    zip.write_all(README.as_bytes())?;
    zip.start_file(MANIFEST, options)?;
    zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    zip.finish()?;
    archive_file.as_file().sync_all()?;
    let bytes = archive_file.as_file().metadata()?.len();

    let name = archive_name(created, plan.scope, trigger);
    let (path, file_name) = persist(archive_file, destination, &name)?;
    if let Ok(directory) = std::fs::File::open(destination) {
        let _ = directory.sync_all();
    }
    drop(staging);
    Ok(Written {
        path,
        file_name,
        bytes,
        manifest,
    })
}

/// Make sure the destination folder is there without creating a mount
/// point. Only the last folder is created, and a scheduled run creates
/// none: an unmounted disk then fails the run instead of filling the system
/// disk.
fn prepare_destination(destination: &Path, trigger: Trigger) -> Result<()> {
    if destination.is_dir() {
        return Ok(());
    }
    if destination.exists() {
        anyhow::bail!("{} is not a folder", destination.display());
    }
    let parent_exists = destination
        .parent()
        .is_some_and(|parent| parent.as_os_str().is_empty() || parent.is_dir());
    if trigger == Trigger::Schedule || !parent_exists {
        anyhow::bail!(
            "destination not found: is its disk mounted? ({})",
            destination.display()
        );
    }
    std::fs::create_dir(destination).with_context(|| format!("creating {}", destination.display()))
}

/// Whether a rename failed because the file system cannot rename without
/// replacing, as exFAT, FAT and some network shares cannot.
fn no_replace_unsupported(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
    ) {
        return true;
    }
    #[cfg(unix)]
    {
        matches!(
            error.raw_os_error(),
            Some(code) if code == libc::ENOTSUP || code == libc::EOPNOTSUPP || code == libc::EINVAL
        )
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// The fallback where a no-replace rename is unsupported: claim the name
/// with an exclusive create, which every file system supports, then rename
/// over that empty placeholder, which is ours. A file already there is
/// never replaced: the claim fails with `AlreadyExists`.
fn persist_over_claim(
    file: tempfile::NamedTempFile,
    path: &Path,
) -> std::result::Result<(), (tempfile::NamedTempFile, std::io::Error)> {
    if let Err(error) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        return Err((file, error));
    }
    match file.persist(path) {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(path);
            Err((error.file, error.error))
        }
    }
}

/// Rename the finished archive into place without replacing a file there.
fn persist(
    file: tempfile::NamedTempFile,
    destination: &Path,
    name: &str,
) -> Result<(PathBuf, String)> {
    let stem = name.trim_end_matches(".zip");
    let mut file = file;
    for attempt in 1..100 {
        let candidate = if attempt == 1 {
            name.to_string()
        } else {
            format!("{stem}-{attempt}.zip")
        };
        let path = destination.join(&candidate);
        let failed = match file.persist_noclobber(&path) {
            Ok(_) => return Ok((path, candidate)),
            Err(error) if no_replace_unsupported(&error.error) => {
                match persist_over_claim(error.file, &path) {
                    Ok(()) => return Ok((path, candidate)),
                    Err((back, error)) => {
                        file = back;
                        error
                    }
                }
            }
            Err(error) => {
                file = error.file;
                error.error
            }
        };
        if failed.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(failed).with_context(|| format!("saving {}", path.display()));
        }
    }
    anyhow::bail!("could not find a free name for {name}")
}

fn describe(item: &PlanItem) -> String {
    match (&item.role, &item.catalog_id) {
        (FileRole::Catalog, Some(id)) => format!("catalog {id}"),
        _ => item.archive_path.trim_start_matches("meta/").to_string(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Generated files a backup leaves out, for the manifest and the docs.
pub const NOT_INCLUDED: &[&str] = &[
    "image previews, annotated previews and star lists (the cache folder)",
    "stack previews and their processing (the stacks folder)",
    "calibration masters (the calibration folder)",
    "astrometry solves, satellite elements and sky maps (the cache folder)",
    "Seiza star catalogs (Settings › Catalogs installs them again)",
    "the FITS and XISF frames themselves",
];

const README: &str = "\
PSF Guard backup
================

manifest.json lists every file here, where it came from (source), its SHA-256
and, for SQLite files, the schema versions.

To restore by hand:

1. Stop PSF Guard: quit the desktop app, or stop the server. Close N.I.N.A.
   if a catalog you restore is its scheduler database.
2. Keep a copy of the files you are about to replace.
3. Copy each file under meta/ back to its source path in manifest.json, and,
   for a full backup, each catalog under catalogs/<slug>/ back to its
   db_path.
4. If the backup was made without secrets, enter peer keys, collaboration
   sign-ins and the server's remote tokens again.
5. Start PSF Guard. Previews, stacks and masters are made again as needed.

See docs/BACKUPS.md in the PSF Guard repository.
";

/// Read the manifest of an archive at `path`.
pub fn read_manifest(path: &Path) -> Result<Manifest> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("reading the zip")?;
    let entry = archive.by_name(MANIFEST).context("no manifest.json")?;
    if entry.size() > MAX_MANIFEST_BYTES {
        anyhow::bail!("manifest.json is too large");
    }
    let mut text = String::new();
    entry.take(MAX_MANIFEST_BYTES).read_to_string(&mut text)?;
    let manifest: Manifest = serde_json::from_str(&text).context("reading manifest.json")?;
    if manifest.format != FORMAT {
        anyhow::bail!("not a PSF Guard backup");
    }
    Ok(manifest)
}

/// One archive found in a folder.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Listing {
    pub file_name: String,
    #[serde(skip)]
    pub path: PathBuf,
    pub bytes: u64,
    pub created_at: String,
    pub created_unix: i64,
    pub scope: BackupScope,
    pub trigger: Trigger,
    pub secrets: SecretsMode,
    pub psf_guard_version: String,
    pub catalogs: usize,
    pub problems: usize,
    /// The install that made it.
    #[serde(skip)]
    pub install_id: Option<String>,
    /// The catalogs it holds, by slug.
    #[serde(skip)]
    pub catalog_ids: Vec<String>,
}

/// The archives in `dir` this module wrote, newest first. A file that only
/// looks like one, or has no readable manifest, is not listed.
pub fn list(dir: &Path) -> Vec<Listing> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Listing> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_archive_name(&name) || !entry.file_type().ok()?.is_file() {
                return None;
            }
            let path = entry.path();
            let manifest = read_manifest(&path).ok()?;
            Some(Listing {
                file_name: name,
                bytes: entry.metadata().ok()?.len(),
                path,
                created_at: manifest.created_at,
                created_unix: manifest.created_unix,
                scope: manifest.scope,
                trigger: manifest.trigger,
                secrets: manifest.secrets,
                psf_guard_version: manifest.psf_guard_version,
                catalogs: manifest.catalogs.iter().filter(|c| c.included).count(),
                problems: manifest.problems.len(),
                install_id: manifest.install_id,
                catalog_ids: manifest
                    .catalogs
                    .into_iter()
                    .filter(|c| c.included)
                    .map(|c| c.id)
                    .collect(),
            })
        })
        .collect();
    found.sort_by(|a, b| {
        b.created_unix
            .cmp(&a.created_unix)
            .then(b.file_name.cmp(&a.file_name))
    });
    found
}

/// Remove the scheduled archives in `dir` that `retention` no longer keeps,
/// and work in progress a dead run left over a day ago. `keep` is the
/// archive the run just wrote, and the measure of what may go:
///
/// - only archives the schedule wrote are touched: the name pattern and a
///   manifest saying so;
/// - only those of `keep`'s install, so two installs sharing a folder keep
///   each other's;
/// - never one holding a catalog `keep` lacks, so a run that could not
///   reach a catalog does not remove the last copy of it;
/// - never `keep` itself. Without a readable `keep`, nothing is pruned.
///
/// Returns the names removed.
pub fn prune(
    dir: &Path,
    retention: schedule::Retention,
    keep: Option<&Path>,
    now_unix: i64,
) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    remove_stale_temporaries(dir);
    let Some(kept) = keep.and_then(|keep| read_manifest(keep).ok()) else {
        return Ok(removed);
    };
    let Some(install) = kept.install_id.clone() else {
        return Ok(removed);
    };
    let kept_catalogs: std::collections::BTreeSet<String> = kept
        .catalogs
        .iter()
        .filter(|catalog| catalog.included)
        .map(|catalog| catalog.id.clone())
        .collect();
    let scheduled: Vec<Listing> = list(dir)
        .into_iter()
        .filter(|listing| {
            listing.trigger == Trigger::Schedule
                && listing.install_id.as_deref() == Some(install.as_str())
        })
        .collect();
    let doomed: Vec<&Listing> = match retention {
        schedule::Retention::Count { count } => {
            scheduled.iter().skip(count.max(1) as usize).collect()
        }
        schedule::Retention::Age { days } => {
            let cutoff = now_unix - i64::from(days) * 86_400;
            scheduled
                .iter()
                .skip(1)
                .filter(|listing| listing.created_unix < cutoff)
                .collect()
        }
    };
    for listing in doomed {
        if keep.is_some_and(|keep| keep == listing.path)
            || listing
                .catalog_ids
                .iter()
                .any(|catalog| !kept_catalogs.contains(catalog))
        {
            continue;
        }
        std::fs::remove_file(&listing.path)
            .with_context(|| format!("removing {}", listing.path.display()))?;
        removed.push(listing.file_name.clone());
    }
    Ok(removed)
}

/// Staging folders and partial archives from a run that died over a day
/// ago. Every run sweeps its folder first.
pub fn remove_stale_temporaries(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(TEMP_PREFIX) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= STALE_TEMP_AGE);
        if !old {
            continue;
        }
        let path = entry.path();
        let _ = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

/// Where a backup folder sits that a person should know about: beside a
/// catalog, in an image folder, or in a folder PSF Guard culls. A relative
/// path is refused by the callers before this is asked.
pub fn destination_warnings(
    destination: &Path,
    registry: &DbRegistry,
    generated: &[PathBuf],
) -> Vec<String> {
    let destination = resolved(destination);
    let mut warnings = Vec::new();
    for entry in &registry.databases {
        if let Some(folder) = Path::new(&entry.db_path).parent()
            && !folder.as_os_str().is_empty()
            && destination.starts_with(resolved(folder))
        {
            warnings.push(format!(
                "This folder holds the catalog {} ({}); a failing disk would take the \
                 backups with it.",
                entry.name, entry.db_path
            ));
        }
        for images in &entry.image_dirs {
            if !images.trim().is_empty() && destination.starts_with(resolved(Path::new(images))) {
                warnings.push(format!(
                    "This folder is inside the image folder {images} of {}.",
                    entry.name
                ));
            }
        }
    }
    for folder in generated {
        if destination.starts_with(resolved(folder)) {
            warnings.push(format!(
                "This folder is inside PSF Guard's generated files ({}), which may be \
                 cleared or moved.",
                folder.display()
            ));
        }
    }
    warnings.dedup();
    warnings
}

/// `path` with its existing part canonical, so links and `..` compare as
/// the folders they are.
fn resolved(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut existing = absolute.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(canonical) = dunce::canonicalize(existing) {
            let mut full = canonical;
            for part in rest.iter().rev() {
                full.push(part);
            }
            return full;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => return absolute,
        }
    }
}

#[cfg(test)]
mod tests;
