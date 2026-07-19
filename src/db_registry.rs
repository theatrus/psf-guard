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
    /// REJECT_ARCHIVE_PLAN.md). All fields optional; absent values fall
    /// back to the CLI flag, then the compiled-in defaults
    /// (`segment_name = "REJECT"`, `depth = 1`,
    /// `sidecar_exts = [".xisf", ".json", ".txt"]`).
    ///
    /// The block itself is also optional; absent in older configs means
    /// "no per-DB overrides — use CLI flags or defaults entirely."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_archive: Option<RejectArchiveOverrides>,
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
    /// Extensions of sibling files that move alongside the primary FITS.
    /// Defaults to `.xisf`, `.json`, `.txt` (set via the resolver in the
    /// CLI command — this slot is only the override).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar_exts: Option<Vec<String>>,
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
}

impl Default for DbRegistry {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            databases: Vec::new(),
            active_db_id: None,
            astrometry: None,
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
            sidecar_exts: Some(vec![".xisf".into(), ".json".into()]),
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
