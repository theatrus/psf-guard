//! Named processing setups, shared across every configured database.
//!
//! A setup captures the parameters of one processing editor — the mono view
//! processing (stretch and deconvolution) or the color pipeline (background
//! extraction, per-channel input processing, output stretches) — under a name
//! the user chose. The registry lives beside the database registry, like the
//! auth registry does, so one file serves every catalog and both the desktop
//! app and a server.
//!
//! The stored settings are canonical JSON produced by round-tripping through
//! the same Rust types the build endpoints deserialize, so a setup that loads
//! is a setup the pipeline can parse. Parameter *ranges* are still enforced at
//! build time by the endpoints themselves; this module only guarantees shape.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
pub const MAX_SETUP_NAME_CHARS: usize = 64;
pub const MAX_SETUPS: usize = 200;

/// Which processing editor a setup belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingSetupKind {
    /// Mono view processing: a display stretch plus optional deconvolution.
    View,
    /// The color pipeline: background extraction, per-channel input
    /// processing, and output stretches.
    Color,
}

impl ProcessingSetupKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Color => "color",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessingSetupRecord {
    pub name: String,
    pub kind: ProcessingSetupKind,
    /// Canonical settings JSON for the editor named by `kind`.
    pub settings: serde_json::Value,
    pub created_unix_seconds: i64,
    pub updated_unix_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessingSetupsRegistry {
    pub schema_version: u32,
    pub setups: Vec<ProcessingSetupRecord>,
}

impl Default for ProcessingSetupsRegistry {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            setups: Vec::new(),
        }
    }
}

/// Reject a name the UI could not display or another file could collide on.
pub fn validate_setup_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("A processing setup needs a name");
    }
    if trimmed != name {
        bail!("A processing setup name cannot start or end with spaces");
    }
    if name.chars().count() > MAX_SETUP_NAME_CHARS {
        bail!("A processing setup name is limited to {MAX_SETUP_NAME_CHARS} characters");
    }
    if name.chars().any(char::is_control) {
        bail!("A processing setup name cannot contain control characters");
    }
    Ok(())
}

fn same_name(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

impl ProcessingSetupsRegistry {
    /// The setups file that accompanies a database registry. The standard
    /// `config.json` registry uses `processing-setups.json`; a custom registry
    /// gets its stem as a prefix, exactly like the auth registry.
    pub fn path_for_database_registry(database_registry_path: &Path) -> PathBuf {
        if database_registry_path
            .file_name()
            .and_then(|name| name.to_str())
            == Some("config.json")
        {
            return database_registry_path.with_file_name("processing-setups.json");
        }
        let stem = database_registry_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("config");
        database_registry_path.with_file_name(format!("{stem}.processing-setups.json"))
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("reading processing setups {}", path.display()))?;
        let registry: Self = serde_json::from_str(&contents)
            .with_context(|| format!("parsing processing setups {}", path.display()))?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        let contents = serde_json::to_vec_pretty(self).context("serializing processing setups")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("creating temporary file in {}", parent.display()))?;
        temporary
            .write_all(&contents)
            .context("writing temporary processing setups")?;
        temporary
            .write_all(b"\n")
            .context("finishing temporary processing setups")?;
        temporary
            .as_file()
            .sync_all()
            .context("syncing temporary processing setups")?;
        temporary
            .persist(path)
            .with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            bail!(
                "Unsupported processing setups schema {} (this build understands {})",
                self.schema_version,
                CURRENT_SCHEMA_VERSION
            );
        }
        if self.setups.len() > MAX_SETUPS {
            bail!("A processing setups registry is limited to {MAX_SETUPS} setups");
        }
        for setup in &self.setups {
            validate_setup_name(&setup.name)?;
        }
        for (index, setup) in self.setups.iter().enumerate() {
            if self.setups[..index]
                .iter()
                .any(|other| same_name(&other.name, &setup.name))
            {
                bail!("Duplicate processing setup name: {}", setup.name);
            }
        }
        Ok(())
    }

    pub fn find(&self, name: &str) -> Option<&ProcessingSetupRecord> {
        self.setups
            .iter()
            .find(|setup| same_name(&setup.name, name))
    }

    /// Insert or replace by name (case-insensitive). Returns true when an
    /// existing setup was replaced. A replacement keeps the original creation
    /// time and the original spelling wins on case-only differences, so a
    /// re-import cannot silently fork "SHO" and "sho".
    pub fn upsert(&mut self, mut record: ProcessingSetupRecord) -> Result<bool> {
        validate_setup_name(&record.name)?;
        if let Some(existing) = self
            .setups
            .iter_mut()
            .find(|setup| same_name(&setup.name, &record.name))
        {
            record.name = existing.name.clone();
            record.created_unix_seconds = existing.created_unix_seconds;
            *existing = record;
            return Ok(true);
        }
        if self.setups.len() >= MAX_SETUPS {
            bail!("A processing setups registry is limited to {MAX_SETUPS} setups");
        }
        self.setups.push(record);
        self.setups.sort_by_key(|setup| setup.name.to_lowercase());
        Ok(false)
    }

    /// Remove by name (case-insensitive). Returns false when nothing matched.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.setups.len();
        self.setups.retain(|setup| !same_name(&setup.name, name));
        self.setups.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, kind: ProcessingSetupKind) -> ProcessingSetupRecord {
        ProcessingSetupRecord {
            name: name.into(),
            kind,
            settings: serde_json::json!({ "model": { "type": "auto-mtf" } }),
            created_unix_seconds: 100,
            updated_unix_seconds: 100,
        }
    }

    #[test]
    fn the_setups_file_sits_beside_the_database_registry() {
        assert_eq!(
            ProcessingSetupsRegistry::path_for_database_registry(Path::new(
                "/data/psf-guard/config.json"
            )),
            Path::new("/data/psf-guard/processing-setups.json")
        );
        assert_eq!(
            ProcessingSetupsRegistry::path_for_database_registry(Path::new("/srv/catalogs.json")),
            Path::new("/srv/catalogs.processing-setups.json")
        );
    }

    #[test]
    fn a_registry_round_trips_through_disk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("processing-setups.json");
        let mut registry = ProcessingSetupsRegistry::default();
        registry
            .upsert(record("Deep SHO", ProcessingSetupKind::Color))
            .unwrap();
        registry.save(&path).unwrap();

        let restored = ProcessingSetupsRegistry::load(&path).unwrap();
        assert_eq!(restored.setups.len(), 1);
        assert_eq!(restored.setups[0].name, "Deep SHO");
        assert_eq!(restored.setups[0].kind, ProcessingSetupKind::Color);
    }

    #[test]
    fn a_missing_file_is_an_empty_registry() {
        let directory = tempfile::tempdir().unwrap();
        let registry =
            ProcessingSetupsRegistry::load(&directory.path().join("absent.json")).unwrap();
        assert!(registry.setups.is_empty());
    }

    #[test]
    fn upsert_replaces_by_name_and_keeps_the_original_spelling_and_birth() {
        let mut registry = ProcessingSetupsRegistry::default();
        assert!(!registry
            .upsert(record("Bright", ProcessingSetupKind::View))
            .unwrap());
        let mut replacement = record("bright", ProcessingSetupKind::View);
        replacement.created_unix_seconds = 999;
        replacement.updated_unix_seconds = 999;
        assert!(registry.upsert(replacement).unwrap());
        assert_eq!(registry.setups.len(), 1);
        assert_eq!(registry.setups[0].name, "Bright");
        assert_eq!(registry.setups[0].created_unix_seconds, 100);
        assert_eq!(registry.setups[0].updated_unix_seconds, 999);
    }

    #[test]
    fn names_are_bounded_and_printable() {
        assert!(validate_setup_name("").is_err());
        assert!(validate_setup_name("  padded  ").is_err());
        assert!(validate_setup_name("line\nbreak").is_err());
        assert!(validate_setup_name(&"n".repeat(MAX_SETUP_NAME_CHARS + 1)).is_err());
        assert!(validate_setup_name("Foraxx · deep OIII").is_ok());
    }

    #[test]
    fn a_registry_with_duplicate_names_does_not_load() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("processing-setups.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "setups": [
                    { "name": "Same", "kind": "view",
                      "settings": {}, "created_unix_seconds": 1, "updated_unix_seconds": 1 },
                    { "name": "same", "kind": "color",
                      "settings": {}, "created_unix_seconds": 1, "updated_unix_seconds": 1 },
                ],
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(ProcessingSetupsRegistry::load(&path).is_err());
    }

    #[test]
    fn removal_is_case_insensitive_and_reports_misses() {
        let mut registry = ProcessingSetupsRegistry::default();
        registry
            .upsert(record("Bright", ProcessingSetupKind::View))
            .unwrap();
        assert!(!registry.remove("absent"));
        assert!(registry.remove("BRIGHT"));
        assert!(registry.setups.is_empty());
    }
}
