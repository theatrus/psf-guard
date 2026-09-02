//! Persistent registry of configured databases.
//!
//! This is the on-disk source of truth for "which N.I.N.A. databases does the
//! user have configured?" It is shared by both the Tauri app and the CLI
//! `server` command — both read from and write to the same JSON file at the
//! platform-standard config location (or a user-supplied path via `--config`).
//!
//! The file is versioned. v1 was single-DB (`{database_path, image_directories}`).
//! v2 (current) supports many DBs, each with its own slug, display name,
//! `.sqlite` path, and image directories. Loading a v1 file migrates it to v2
//! in place, preserving a `.bak` backup.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::server::slug::{compute_default_slug, validate_slug};

/// Current on-disk schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// One configured database. The `id` is the canonical URL-safe slug used in
/// `/api/db/{id}/...` and cache directories.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DbEntry {
    pub id: String,
    pub name: String,
    pub db_path: String,
    #[serde(default)]
    pub image_dirs: Vec<String>,
    /// Per-DB overrides for the out-of-tree reject-archive feature (see
    /// `docs/design/reject-archive.md`). All fields optional; absent values fall
    /// back to the CLI flag, then the compiled-in defaults
    /// (`segment_name = "REJECT"`, `depth = 1`,
    /// `sidecar_exts = [".json", ".txt"]`).
    ///
    /// The block itself is also optional; absent in older configs means
    /// "no per-DB overrides — use CLI flags or defaults entirely."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_archive: Option<RejectArchiveOverrides>,
    /// Opt-in receiver for images posted by a remote N.I.N.A. instance or
    /// another acquisition client. The token is stored only as a salted
    /// SHA-256 digest; plaintext exists only in the management request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_image_upload: Option<RemoteImageUploadConfig>,
    /// Server-side destination for UI-triggered exports. Because the
    /// operator names this directory here (or in Settings), the export
    /// action itself needs no database-management grant — the UI only ever
    /// asks to export into this pre-consented location. Absent means server
    /// export is off and the UI offers the archive download instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_dir: Option<String>,
}

/// One paired client credential. Each pairing mints its own, so revoking a
/// laptop does not sign out the observatory machine. Only the salted hash
/// is stored; the plaintext existed once, in the pairing response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoteClient {
    pub client_uuid: String,
    /// Operator-facing label, from the client's pairing request.
    pub name: String,
    pub token_salt: String,
    pub token_sha256: String,
    pub paired_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RemoteImageUploadPlacement {
    /// Keep every received frame directly below the selected image root.
    /// This is the compatibility default for registries written before
    /// target-aware placement existed.
    #[default]
    Flat,
    /// Group lights and flats by target, with the frame type and filter below
    /// it. Other calibration kinds use their own top-level frame-type folder.
    TargetTree,
}

pub const DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE: &str = "%TARGET%/%TYPE%/%FILTER%";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RemoteImageUploadTemplateSource {
    #[default]
    Preset,
    Catalog,
}

/// A validated server-owned directory layout below the configured receive
/// root. The client still supplies only the image basename.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteImageUploadDirectoryLayout {
    /// Effective template rendered for new uploads.
    pub template: String,
    /// Operator-selected template retained when catalog detection succeeds, so
    /// an empty or ambiguous later scan does not silently change the fallback.
    #[serde(default = "default_remote_upload_directory_template")]
    pub fallback_template: String,
    #[serde(default)]
    pub source: RemoteImageUploadTemplateSource,
    /// Catalog files which supported a detected layout. Zero for a preset.
    #[serde(default)]
    pub samples: usize,
}

/// Whether a layout carries a calendar date below its `%TARGET%` level — a
/// per-night folder that detection failed to turn into a token.
pub fn template_carries_a_date(template: &str) -> bool {
    fn is_iso_date(value: &str) -> bool {
        value.len() == 10
            && value.as_bytes().get(4) == Some(&b'-')
            && value.as_bytes().get(7) == Some(&b'-')
            && chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
    }
    template
        .split('/')
        .skip_while(|component| !component.contains("%TARGET%"))
        .any(|component| {
            (0..component.len().saturating_sub(9))
                .any(|start| component.get(start..start + 10).is_some_and(is_iso_date))
        })
}

fn default_remote_upload_directory_template() -> String {
    DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RemoteImageUploadConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub image_dir: String,
    #[serde(default)]
    pub token_salt: String,
    #[serde(default)]
    pub token_sha256: String,
    /// Per-client credentials from pairing. The legacy single token above
    /// keeps working for installs configured before pairing existed.
    #[serde(default)]
    pub clients: Vec<RemoteClient>,
    /// Opt this database into the remote scheduler sync protocol
    /// (`/api/sync/v1`). Independent of `enabled`, which covers image upload
    /// only. Defaults to false, so a token configured for uploads before the
    /// sync protocol existed does not silently gain merge and apply rights.
    #[serde(default)]
    pub sync_enabled: bool,
    /// Server-derived layout below `image_dir`. The client still supplies
    /// only a basename; it can never choose a directory or relative path.
    #[serde(default)]
    pub placement: RemoteImageUploadPlacement,
    /// Persisted preset or catalog-derived template for `TargetTree`.
    /// Missing in older registries means the original target/type/filter tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_layout: Option<RemoteImageUploadDirectoryLayout>,
}

impl RemoteImageUploadConfig {
    pub const MIN_TOKEN_LENGTH: usize = 24;
    pub const MAX_TOKEN_LENGTH: usize = 256;

    pub fn directory_template(&self) -> &str {
        self.directory_layout
            .as_ref()
            .map(|layout| layout.template.as_str())
            .unwrap_or(DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE)
    }

    pub fn fallback_directory_template(&self) -> &str {
        self.directory_layout
            .as_ref()
            .map(|layout| layout.fallback_template.as_str())
            .unwrap_or(DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE)
    }

    pub fn set_token(&mut self, token: &str) -> Result<()> {
        if token.len() < Self::MIN_TOKEN_LENGTH || token.len() > Self::MAX_TOKEN_LENGTH {
            anyhow::bail!(
                "remote image upload token must be {}-{} characters",
                Self::MIN_TOKEN_LENGTH,
                Self::MAX_TOKEN_LENGTH
            );
        }
        if token.chars().any(char::is_control) {
            anyhow::bail!("remote image upload token cannot contain control characters");
        }
        self.token_salt = uuid::Uuid::new_v4().simple().to_string();
        self.token_sha256 = salted_token_sha256(&self.token_salt, token);
        Ok(())
    }

    pub fn token_is_configured(&self) -> bool {
        let legacy = self.token_salt.len() == 32
            && self.token_salt.bytes().all(|byte| byte.is_ascii_hexdigit())
            && self.token_sha256.len() == 64
            && self
                .token_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit());
        legacy || !self.clients.is_empty()
    }

    pub fn token_matches(&self, token: &str) -> bool {
        // Legacy single token, then every paired client. All comparisons run
        // (no early exit on the legacy match shape) with constant-time
        // equality per candidate.
        let legacy = {
            let candidate = salted_token_sha256(&self.token_salt, token);
            constant_time_eq(self.token_sha256.as_bytes(), candidate.as_bytes())
        };
        let client = self
            .clients
            .iter()
            .filter(|client| client.token_salt.len() == 32 && client.token_sha256.len() == 64)
            .fold(false, |matched, client| {
                let candidate = salted_token_sha256(&client.token_salt, token);
                matched | constant_time_eq(client.token_sha256.as_bytes(), candidate.as_bytes())
            });
        legacy | client
    }

    /// Add a paired client credential, returning its id. Validates the token
    /// the same way `set_token` does; the legacy single token is untouched.
    pub fn add_client(&mut self, name: &str, token: &str) -> Result<String> {
        if token.len() < Self::MIN_TOKEN_LENGTH || token.len() > Self::MAX_TOKEN_LENGTH {
            anyhow::bail!(
                "client token must be {}-{} characters",
                Self::MIN_TOKEN_LENGTH,
                Self::MAX_TOKEN_LENGTH
            );
        }
        let client_uuid = uuid::Uuid::new_v4().to_string();
        let token_salt = uuid::Uuid::new_v4().simple().to_string();
        let token_sha256 = salted_token_sha256(&token_salt, token);
        self.clients.push(RemoteClient {
            client_uuid: client_uuid.clone(),
            name: name.trim().to_string(),
            token_salt,
            token_sha256,
            paired_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0),
        });
        Ok(client_uuid)
    }

    /// Revoke one paired client. Returns whether anything was removed.
    pub fn revoke_client(&mut self, client_uuid: &str) -> bool {
        let before = self.clients.len();
        self.clients
            .retain(|client| client.client_uuid != client_uuid);
        self.clients.len() != before
    }

    /// Resolve the selected receive directory and prove that it is exactly
    /// one of this database's configured image roots.
    pub fn validated_image_dir(&self, image_dirs: &[String]) -> Result<PathBuf> {
        if !self.enabled {
            anyhow::bail!("remote image upload is disabled");
        }
        if !self.token_is_configured() {
            anyhow::bail!("remote image upload token is not configured");
        }
        if self.image_dir.trim().is_empty() {
            anyhow::bail!("remote image upload directory is not configured");
        }

        let selected = dunce::canonicalize(&self.image_dir).with_context(|| {
            format!("resolving remote image upload directory {}", self.image_dir)
        })?;
        if !selected.is_dir() {
            anyhow::bail!(
                "remote image upload directory is not a directory: {}",
                selected.display()
            );
        }

        let is_registered_root = image_dirs.iter().any(|root| {
            dunce::canonicalize(root)
                .map(|root| root == selected)
                .unwrap_or(false)
        });
        if !is_registered_root {
            anyhow::bail!("remote image upload directory must be one of the database's image_dirs");
        }
        Ok(selected)
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (&left, &right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

fn salted_token_sha256(salt: &str, token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update([0]);
    hasher.update(token.as_bytes());
    sha256_digest_hex(hasher.finalize())
}

fn sha256_digest_hex(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

/// Persisted per-DB override block for the reject archive. All fields are
/// optional so users can set just the knobs they care about (e.g. only the
/// segment name) without re-specifying every default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RejectArchiveOverrides {
    /// Folder name inserted into the archive path. Default `"REJECT"`.
    /// Validated (URL-safe-ish, no path separators) at command time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_name: Option<String>,
    /// How many path segments below `image_dir` to descend before
    /// inserting `segment_name`. Default `1` (right under the project
    /// folder); set to `0` to drop everything into a single per-image-dir
    /// REJECT bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// Extensions of sibling files that move alongside the primary frame.
    /// Defaults to `.json`, `.txt` (set via the resolver in the CLI command —
    /// this slot is only the override). A frame container is refused here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar_exts: Option<Vec<String>>,
}

/// A remote PSF Guard this instance can sync with.
///
/// Unlike an incoming key, which is stored as a digest because the server only
/// ever needs to *check* it, an outgoing key has to be presented on every
/// request — so it is kept in the clear. That makes the registry file a
/// credential store: it is the user's own config, but it should be readable
/// only by them. The API never returns the key to a browser.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeerEntry {
    /// URL-safe slug used in `/api/peers/{id}`.
    pub id: String,
    pub name: String,
    /// Base URL, e.g. `https://telescope.example:3000`.
    pub base_url: String,
    #[serde(default)]
    pub token: String,
    /// Which of the peer's catalogs to use. Absent means "whichever its key
    /// opens", which is the only one it will accept anyway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
}

/// Persisted shape of the database registry on disk (v2+).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbRegistry {
    pub schema_version: u32,
    #[serde(default)]
    pub databases: Vec<DbEntry>,
    /// Hint for the UI: which DB was last interacted with. Optional; the
    /// merged-overview UI ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_db_id: Option<String>,
    /// Process-global Seiza catalog configuration shared by every database.
    /// Additive within registry v2: an absent block lets Seiza search its
    /// standard environment, executable-adjacent, and platform data paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub astrometry: Option<crate::astrometry::AstrometryConfig>,
    /// Remote PSF Guard instances this one can sync with. Additive within
    /// registry v2; absent means none are configured.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<PeerEntry>,
    /// Process-global calibration matching settings shared by every database,
    /// edited from the settings panel. Additive within registry v2; absent
    /// means the library defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<CalibrationSettings>,
    /// Process-global export defaults shared by every database, edited from
    /// the settings panel. Additive within registry v2; absent means the
    /// standard layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<ExportSettings>,
}

/// How far apart two readings may sit and still calibrate each other.
///
/// Lives beside [`DbRegistry::astrometry`] rather than in the server TOML for
/// the same reason: it is a property of the person's rig, not of one
/// deployment, and the settings panel — served identically in browser and
/// desktop modes — is where they will reach for it when a flat is refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CalibrationSettings {
    /// Rotator angle between a flat and what it corrects, in degrees.
    /// Absent uses the library default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_tolerance_deg: Option<f64>,
    /// How masters built by other software are used: `prefer` (the
    /// default), `fallback`, or `ignore`. Absent means prefer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_masters: Option<crate::calibration::ExternalMasterPolicy>,
}

/// What the export dialog starts from. The dialog still offers every layout
/// on each export; this only seeds the choice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ExportSettings {
    /// Layout new exports start from. Absent means the standard
    /// grouped-by-target tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_layout: Option<crate::commands::export::ExportLayout>,
}

impl Default for DbRegistry {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            databases: Vec::new(),
            active_db_id: None,
            astrometry: None,
            peers: Vec::new(),
            calibration: None,
            export: None,
        }
    }
}

/// v1 (legacy) on-disk shape: single database, single set of image dirs.
#[derive(Debug, Clone, Deserialize)]
struct LegacyConfigV1 {
    #[serde(default)]
    database_path: Option<String>,
    #[serde(default)]
    image_directories: Vec<String>,
}

impl DbRegistry {
    /// Default path on this platform where the registry is persisted.
    pub fn default_path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
            .join("psf-guard");
        std::fs::create_dir_all(&dir).context("creating config directory")?;
        Ok(dir.join("config.json"))
    }

    /// Load from the given file. Returns `Default::default()` if the file
    /// doesn't exist yet. If the file is v1, migrates in place and writes back
    /// a v2 file (preserving the original as `<file>.bak`).
    pub fn load_or_init(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config at {}", path.display()))?;

        // Try to parse as v2 (or anything with schema_version).
        match serde_json::from_str::<DbRegistry>(&raw) {
            Ok(mut reg) if reg.schema_version >= 1 => {
                // Dedup any duplicate slugs introduced by hand-editing.
                reg.dedup_and_validate()?;
                if reg.retire_dated_catalog_layouts() {
                    // Persist the repair now: this loader serves read paths
                    // that never save, and a file left as it was would warn
                    // on every load and resume filing under one night for any
                    // other reader.
                    if let Err(error) = reg.save(path) {
                        tracing::warn!(
                            "Could not persist the repaired remote upload layout to {}: {error:#}",
                            path.display()
                        );
                    }
                }
                Ok(reg)
            }
            _ => {
                // Fall through to v1 migration.
                let v1: LegacyConfigV1 = serde_json::from_str(&raw)
                    .with_context(|| "config is neither v2 nor a recognizable v1 shape")?;
                let migrated = Self::migrate_from_v1(v1, path)?;
                Ok(migrated)
            }
        }
    }

    /// 0.9.2 could detect a catalog layout with a literal date in it
    /// (`…/NIGHT_2025-12-14/…`) and every upload since has been filed under
    /// that one night. Detection no longer produces such a layout, but a
    /// stored one is only replaced by a rescan, so a catalog match that
    /// carries a date below the target is returned to its fallback here, at
    /// load, and logged. The next save persists the repair.
    fn retire_dated_catalog_layouts(&mut self) -> bool {
        let mut changed = false;
        for entry in &mut self.databases {
            let Some(config) = entry.remote_image_upload.as_mut() else {
                continue;
            };
            let Some(layout) = config.directory_layout.as_ref() else {
                continue;
            };
            if layout.source != RemoteImageUploadTemplateSource::Catalog
                || !template_carries_a_date(&layout.template)
            {
                continue;
            }
            let fallback = layout.fallback_template.clone();
            tracing::warn!(
                database = %entry.id,
                template = %layout.template,
                "Retired a remote upload layout that fixes one date; using {fallback} until the next catalog rescan"
            );
            config.directory_layout = Some(RemoteImageUploadDirectoryLayout {
                template: fallback.clone(),
                fallback_template: fallback,
                source: RemoteImageUploadTemplateSource::Preset,
                samples: 0,
            });
            changed = true;
        }
        changed
    }

    fn migrate_from_v1(v1: LegacyConfigV1, path: &Path) -> Result<Self> {
        let bak = path.with_extension("json.bak");
        std::fs::copy(path, &bak)
            .with_context(|| format!("backing up v1 config to {}", bak.display()))?;
        tracing::info!(
            "Migrated legacy single-DB config; backup written to {}",
            bak.display()
        );

        let mut reg = DbRegistry::default();
        if let Some(db_path) = v1.database_path.filter(|s| !s.trim().is_empty()) {
            let slug = compute_default_slug(&db_path);
            let name = PathBuf::from(&db_path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Database".to_string());
            reg.active_db_id = Some(slug.clone());
            reg.databases.push(DbEntry {
                id: slug,
                name,
                db_path,
                image_dirs: v1.image_directories,
                reject_archive: None,
                remote_image_upload: None,
                export_dir: None,
            });
        }
        reg.save(path)?;
        Ok(reg)
    }

    /// Persist to disk atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("creating config directory")?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(self).context("serializing registry")?;
        std::fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("renaming temp to {}", path.display()))?;
        Ok(())
    }

    /// Find an entry by slug.
    pub fn find(&self, id: &str) -> Option<&DbEntry> {
        self.databases.iter().find(|d| d.id == id)
    }

    /// Find an entry whose `db_path` canonicalizes to the same file as the
    /// supplied path. Falls back to a literal string match if neither path
    /// can be canonicalized.
    pub fn find_by_path(&self, db_path: &str) -> Option<&DbEntry> {
        let canon_target = std::fs::canonicalize(db_path).ok();
        for entry in &self.databases {
            if entry.db_path == db_path {
                return Some(entry);
            }
            if let Some(target) = &canon_target
                && let Ok(canon_entry) = std::fs::canonicalize(&entry.db_path)
                && canon_entry == *target
            {
                return Some(entry);
            }
        }
        None
    }

    /// Add a new entry. The caller may supply a desired slug; if absent or
    /// already taken, a deterministic default is computed from the path and
    /// disambiguated with a `-N` suffix.
    pub fn add(
        &mut self,
        name: String,
        db_path: String,
        image_dirs: Vec<String>,
        desired_slug: Option<String>,
    ) -> Result<&DbEntry> {
        let slug = match desired_slug {
            Some(s) => {
                validate_slug(&s).map_err(|msg| anyhow::anyhow!(msg))?;
                self.unique_slug(s)
            }
            None => self.unique_slug(compute_default_slug(&db_path)),
        };
        self.databases.push(DbEntry {
            id: slug,
            name,
            db_path,
            image_dirs,
            reject_archive: None,
            remote_image_upload: None,
            export_dir: None,
        });
        Ok(self.databases.last().unwrap())
    }

    /// Update an existing entry. Slug rename validates the new slug.
    /// Returns whether the slug itself changed (so callers can rename cache dirs).
    pub fn update(
        &mut self,
        id: &str,
        new_name: Option<String>,
        new_slug: Option<String>,
        new_db_path: Option<String>,
        new_image_dirs: Option<Vec<String>>,
    ) -> Result<bool> {
        // Validate the requested slug change up-front (and avoid renaming to a
        // slug that collides with a different entry).
        let renamed = if let Some(slug) = &new_slug {
            validate_slug(slug).map_err(|msg| anyhow::anyhow!(msg))?;
            if slug != id {
                if self.databases.iter().any(|d| d.id == *slug) {
                    return Err(anyhow::anyhow!(
                        "slug '{}' is already used by another database",
                        slug
                    ));
                }
                if self.active_db_id.as_deref() == Some(id) {
                    self.active_db_id = Some(slug.clone());
                }
                true
            } else {
                false
            }
        } else {
            false
        };

        let entry = self
            .databases
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| anyhow::anyhow!("no database with slug '{}'", id))?;

        if let Some(slug) = new_slug {
            entry.id = slug;
        }
        if let Some(name) = new_name {
            entry.name = name;
        }
        if let Some(db_path) = new_db_path {
            entry.db_path = db_path;
        }
        if let Some(image_dirs) = new_image_dirs {
            entry.image_dirs = image_dirs;
        }
        Ok(renamed)
    }

    /// Remove an entry by slug. Returns Ok(true) if anything was removed.
    pub fn remove(&mut self, id: &str) -> Result<bool> {
        let before = self.databases.len();
        self.databases.retain(|d| d.id != id);
        if self.active_db_id.as_deref() == Some(id) {
            self.active_db_id = None;
        }
        Ok(self.databases.len() < before)
    }

    /// Return a slug not currently in use. Tries the supplied seed first,
    /// then appends `-2`, `-3`, ... as needed.
    pub fn unique_slug(&self, seed: String) -> String {
        if !self.databases.iter().any(|d| d.id == seed) {
            return seed;
        }
        for i in 2..u32::MAX {
            let candidate = format!("{}-{}", seed, i);
            if !self.databases.iter().any(|d| d.id == candidate) {
                return candidate;
            }
        }
        // Astronomically unlikely.
        format!("{}-x", seed)
    }

    fn dedup_and_validate(&mut self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        let mut dedup = Vec::with_capacity(self.databases.len());
        for entry in self.databases.drain(..) {
            if entry.id.is_empty() || validate_slug(&entry.id).is_err() {
                tracing::warn!(
                    "Skipping config entry with invalid slug '{}' (db={})",
                    entry.id,
                    entry.db_path
                );
                continue;
            }
            if !seen.insert(entry.id.clone()) {
                tracing::warn!("Dropping duplicate config entry with slug '{}'", entry.id);
                continue;
            }
            dedup.push(entry);
        }
        self.databases = dedup;
        if let Some(active) = &self.active_db_id
            && !self.databases.iter().any(|d| d.id == *active)
        {
            self.active_db_id = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn paired_clients_and_the_legacy_key_authenticate_independently() {
        let mut config = RemoteImageUploadConfig::default();
        config.set_token("legacy-manual-key-0123456789").unwrap();
        let laptop = config
            .add_client("Laptop", "psfrc_laptop_credential_0123456789")
            .unwrap();
        let observatory = config
            .add_client("Observatory", "psfrc_observatory_credential_0123456789")
            .unwrap();

        assert!(config.token_is_configured());
        assert!(config.token_matches("legacy-manual-key-0123456789"));
        assert!(config.token_matches("psfrc_laptop_credential_0123456789"));
        assert!(config.token_matches("psfrc_observatory_credential_0123456789"));
        assert!(!config.token_matches("psfrc_never_issued_0123456789"));

        // Revoking one client leaves the other and the legacy key alone.
        assert!(config.revoke_client(&laptop));
        assert!(!config.revoke_client(&laptop));
        assert!(!config.token_matches("psfrc_laptop_credential_0123456789"));
        assert!(config.token_matches("psfrc_observatory_credential_0123456789"));
        assert!(config.token_matches("legacy-manual-key-0123456789"));
        let _ = observatory;
    }

    #[test]
    fn clients_alone_configure_the_token_check() {
        // A catalog that was only ever paired (no manual key) still counts
        // as configured, or capabilities would hide it from its own client.
        let mut config = RemoteImageUploadConfig::default();
        config
            .add_client("Only client", "psfrc_only_credential_0123456789")
            .unwrap();
        assert!(config.token_is_configured());
        assert!(config.token_matches("psfrc_only_credential_0123456789"));
    }

    #[test]
    fn loads_empty_when_file_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        let reg = DbRegistry::load_or_init(&path).unwrap();
        assert_eq!(reg.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(reg.databases.is_empty());
    }

    #[test]
    fn migrates_v1_to_v2_preserving_data_and_writes_bak() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        // Create a dummy DB file so the migration can canonicalize its path.
        let db_path = dir.path().join("legacy.sqlite");
        std::fs::write(&db_path, b"").unwrap();
        let img_dir = dir.path().join("imgs");
        std::fs::create_dir(&img_dir).unwrap();

        let v1 = serde_json::json!({
            "database_path": db_path.to_string_lossy(),
            "image_directories": [img_dir.to_string_lossy()],
        });
        write(&path, &v1.to_string());

        let reg = DbRegistry::load_or_init(&path).unwrap();
        assert_eq!(reg.schema_version, 2);
        assert_eq!(reg.databases.len(), 1);
        let entry = &reg.databases[0];
        assert!(entry.id.starts_with("db-"));
        assert_eq!(entry.db_path, db_path.to_string_lossy());
        assert_eq!(entry.image_dirs.len(), 1);
        // Active hint defaults to the migrated entry.
        assert_eq!(reg.active_db_id.as_deref(), Some(entry.id.as_str()));
        // Backup file written.
        assert!(path.with_extension("json.bak").exists());
        // Round-trip: re-read should now look like v2.
        let reloaded = DbRegistry::load_or_init(&path).unwrap();
        assert_eq!(reloaded.databases.len(), 1);
    }

    #[test]
    fn round_trips_v2_unchanged() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut reg = DbRegistry::default();
        reg.add(
            "Imaging Rig".into(),
            "/tmp/imaging.sqlite".into(),
            vec!["/tmp/imgs".into()],
            Some("imaging-rig".into()),
        )
        .unwrap();
        reg.save(&path).unwrap();
        let reloaded = DbRegistry::load_or_init(&path).unwrap();
        assert_eq!(reloaded.databases, reg.databases);
    }

    #[test]
    fn a_stored_catalog_layout_with_a_fixed_date_is_retired_on_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut reg = DbRegistry::default();
        reg.databases.push(DbEntry {
            id: "remote".into(),
            name: "Remote".into(),
            db_path: "/tmp/remote.sqlite".into(),
            image_dirs: vec!["/images".into()],
            reject_archive: None,
            remote_image_upload: Some(RemoteImageUploadConfig {
                placement: RemoteImageUploadPlacement::TargetTree,
                directory_layout: Some(RemoteImageUploadDirectoryLayout {
                    template: "ZWO ASI2600MM Pro/%TARGET%/NIGHT_2025-12-14/%FILTER%/%TYPE%".into(),
                    fallback_template: "%CAMERA%/%TARGET%/NIGHT_%NIGHT%/%FILTER%/%TYPE%".into(),
                    source: RemoteImageUploadTemplateSource::Catalog,
                    samples: 157,
                }),
                ..Default::default()
            }),
            export_dir: None,
        });
        reg.save(&path).unwrap();

        let reloaded = DbRegistry::load_or_init(&path).unwrap();
        let layout = reloaded.databases[0]
            .remote_image_upload
            .as_ref()
            .unwrap()
            .directory_layout
            .clone()
            .unwrap();
        assert_eq!(
            layout.template,
            "%CAMERA%/%TARGET%/NIGHT_%NIGHT%/%FILTER%/%TYPE%"
        );
        assert_eq!(layout.source, RemoteImageUploadTemplateSource::Preset);
        // Persisted, so the file itself no longer carries the fixed date.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains("NIGHT_2025-12-14"), "{on_disk}");

        assert!(template_carries_a_date("%TARGET%/NIGHT_2025-12-14/%TYPE%"));
        assert!(!template_carries_a_date(
            "Trip_2025-08-01/%TARGET%/%NIGHT%/%TYPE%"
        ));
        assert!(!template_carries_a_date("%TARGET%/%NIGHT%/%TYPE%"));
    }

    #[test]
    fn round_trips_calibration_settings_and_absence_stays_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        let reg = DbRegistry {
            calibration: Some(CalibrationSettings {
                rotation_tolerance_deg: Some(3.5),
                external_masters: Some(crate::calibration::ExternalMasterPolicy::Fallback),
            }),
            ..Default::default()
        };
        reg.save(&path).unwrap();

        let reloaded = DbRegistry::load_or_init(&path).unwrap();
        assert_eq!(reloaded.calibration, reg.calibration);
        let serialized = std::fs::read_to_string(&path).unwrap();
        assert!(serialized.contains("\"calibration\""));
        assert!(serialized.contains("rotation_tolerance_deg"));
        assert!(serialized.contains("\"external_masters\": \"fallback\""));

        // A registry that never configured it keeps a clean file: additive
        // within v2, and an older build reading this file sees nothing new.
        let bare = DbRegistry::default();
        bare.save(&path).unwrap();
        let serialized = std::fs::read_to_string(&path).unwrap();
        assert!(!serialized.contains("calibration"));
    }

    #[test]
    fn round_trips_export_settings_and_absence_stays_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        let reg = DbRegistry {
            export: Some(ExportSettings {
                default_layout: Some(crate::commands::export::ExportLayout::Wbpp),
            }),
            ..Default::default()
        };
        reg.save(&path).unwrap();

        let reloaded = DbRegistry::load_or_init(&path).unwrap();
        assert_eq!(reloaded.export, reg.export);
        let serialized = std::fs::read_to_string(&path).unwrap();
        assert!(serialized.contains("\"export\""));
        assert!(serialized.contains("wbpp"));

        let bare = DbRegistry::default();
        bare.save(&path).unwrap();
        let serialized = std::fs::read_to_string(&path).unwrap();
        assert!(!serialized.contains("\"export\""));
    }

    #[test]
    fn round_trips_process_global_astrometry_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        let reg = DbRegistry {
            astrometry: Some(crate::astrometry::AstrometryConfig {
                data_dir: Some("/catalogs/seiza".to_string()),
                objects: None,
                stars: Some("stars-lite-tycho2.bin".to_string()),
                satellite_elements: Some("active-satellites.json".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        reg.save(&path).unwrap();

        let reloaded = DbRegistry::load_or_init(&path).unwrap();
        assert_eq!(reloaded.astrometry, reg.astrometry);
        let serialized = std::fs::read_to_string(path).unwrap();
        assert!(serialized.contains("\"astrometry\""));
        assert!(serialized.contains("stars-lite-tycho2.bin"));
        assert!(serialized.contains("active-satellites.json"));
    }

    #[test]
    fn loads_v2_config_without_reject_archive_block() {
        // Configs written before A2 don't have the `reject_archive` key.
        // Older configs must keep loading; the field defaults to None.
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        let body = serde_json::json!({
            "schema_version": 2,
            "databases": [
                {"id": "a", "name": "A", "db_path": "/tmp/a.sqlite", "image_dirs": []}
            ],
        });
        write(&path, &body.to_string());
        let reg = DbRegistry::load_or_init(&path).unwrap();
        assert_eq!(reg.databases.len(), 1);
        assert!(reg.databases[0].reject_archive.is_none());
    }

    #[test]
    fn round_trips_reject_archive_overrides() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut reg = DbRegistry::default();
        reg.add(
            "Imaging Rig".into(),
            "/tmp/imaging.sqlite".into(),
            vec!["/tmp/imgs".into()],
            Some("imaging-rig".into()),
        )
        .unwrap();
        reg.databases[0].reject_archive = Some(RejectArchiveOverrides {
            segment_name: Some("BAD".into()),
            depth: Some(2),
            sidecar_exts: Some(vec![".wcs".into(), ".json".into()]),
        });
        reg.save(&path).unwrap();
        let reloaded = DbRegistry::load_or_init(&path).unwrap();
        assert_eq!(reloaded.databases, reg.databases);

        // The serialized JSON should NOT include the block when it's None,
        // so older psf-guards skip it cleanly (forward-compat sanity).
        let mut bare = DbRegistry::default();
        bare.add("X".into(), "/tmp/x.sqlite".into(), vec![], Some("x".into()))
            .unwrap();
        let json = serde_json::to_string(&bare).unwrap();
        assert!(
            !json.contains("reject_archive"),
            "default config should not write the key: {json}"
        );
    }

    #[test]
    fn remote_upload_round_trip_stores_only_a_token_digest() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        let images = dir.path().join("images");
        std::fs::create_dir(&images).unwrap();
        let token = "a-long-random-upload-token-123456";
        let mut config = RemoteImageUploadConfig {
            enabled: true,
            image_dir: images.to_string_lossy().into_owned(),
            placement: RemoteImageUploadPlacement::TargetTree,
            ..Default::default()
        };
        config.set_token(token).unwrap();
        assert!(config.token_matches(token));
        assert!(!config.token_matches("a-different-long-upload-token"));
        assert_eq!(
            config
                .validated_image_dir(&[images.to_string_lossy().into_owned()])
                .unwrap(),
            dunce::canonicalize(&images).unwrap()
        );

        let mut registry = DbRegistry::default();
        registry
            .add(
                "Remote".into(),
                "/tmp/remote.sqlite".into(),
                vec![images.to_string_lossy().into_owned()],
                Some("remote".into()),
            )
            .unwrap();
        registry.databases[0].remote_image_upload = Some(config);
        registry.save(&path).unwrap();

        let serialized = std::fs::read_to_string(&path).unwrap();
        assert!(!serialized.contains(token));
        assert!(serialized.contains("token_sha256"));
        let loaded = DbRegistry::load_or_init(&path).unwrap();
        assert!(loaded.databases[0]
            .remote_image_upload
            .as_ref()
            .unwrap()
            .token_matches(token));
        assert_eq!(
            loaded.databases[0]
                .remote_image_upload
                .as_ref()
                .unwrap()
                .placement,
            RemoteImageUploadPlacement::TargetTree
        );
    }

    #[test]
    fn remote_upload_without_placement_keeps_the_flat_compatibility_layout() {
        let config: RemoteImageUploadConfig = serde_json::from_value(serde_json::json!({
            "enabled": false,
            "image_dir": ""
        }))
        .unwrap();
        assert_eq!(config.placement, RemoteImageUploadPlacement::Flat);
    }

    #[test]
    fn unique_slug_disambiguates_on_collision() {
        let mut reg = DbRegistry::default();
        reg.add(
            "A".into(),
            "/tmp/a.sqlite".into(),
            vec![],
            Some("imaging-rig".into()),
        )
        .unwrap();
        let id2 = reg.unique_slug("imaging-rig".into());
        assert_eq!(id2, "imaging-rig-2");
    }

    #[test]
    fn update_renames_slug_and_rejects_collisions() {
        let mut reg = DbRegistry::default();
        reg.add("A".into(), "/tmp/a.sqlite".into(), vec![], Some("a".into()))
            .unwrap();
        reg.add("B".into(), "/tmp/b.sqlite".into(), vec![], Some("b".into()))
            .unwrap();
        // Rename b -> c
        let renamed = reg.update("b", None, Some("c".into()), None, None).unwrap();
        assert!(renamed);
        assert!(reg.find("c").is_some());
        assert!(reg.find("b").is_none());
        // Collision: c -> a should fail
        assert!(reg.update("c", None, Some("a".into()), None, None).is_err());
    }

    #[test]
    fn remove_clears_active_hint() {
        let mut reg = DbRegistry::default();
        reg.add("A".into(), "/tmp/a.sqlite".into(), vec![], Some("a".into()))
            .unwrap();
        reg.active_db_id = Some("a".into());
        assert!(reg.remove("a").unwrap());
        assert!(reg.active_db_id.is_none());
    }

    #[test]
    fn dedup_drops_duplicate_slugs_on_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        let body = serde_json::json!({
            "schema_version": 2,
            "databases": [
                {"id": "a", "name": "A", "db_path": "/tmp/a.sqlite", "image_dirs": []},
                {"id": "a", "name": "A2", "db_path": "/tmp/a2.sqlite", "image_dirs": []},
            ],
        });
        write(&path, &body.to_string());
        let reg = DbRegistry::load_or_init(&path).unwrap();
        assert_eq!(reg.databases.len(), 1);
        assert_eq!(reg.databases[0].name, "A");
    }
}
