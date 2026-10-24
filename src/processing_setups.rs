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
//!
//! One Color setup is the default: new color previews, a person's and the
//! ones automatic refreshes compose, start from its pipeline. A registry that
//! has never had a default gets a setup named "Default" with the built-in
//! pipeline in [`DEFAULT_COLOR_SETTINGS`]; the server adds it when it loads
//! the file and writes it with the next change, so a read never writes.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
pub const MAX_SETUP_NAME_CHARS: usize = 64;
pub const MAX_SETUPS: usize = 200;

/// The built-in color pipeline, which the seeded "Default" setup starts with:
/// background extraction, an automatic MTF stretch on every channel, solar
/// color calibration, and inputs that follow their channel cards. The
/// frontend's `defaultColorProcessing` must produce exactly this JSON; a test
/// on each side checks it.
pub const DEFAULT_COLOR_SETTINGS: &str = include_str!("processing_setups/default_color.json");

/// The name the seeded default Color setup gets when no setup uses it.
pub const DEFAULT_COLOR_SETUP_NAME: &str = "Default";

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

/// Unknown fields are ignored, so a file a newer version wrote still loads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingSetupRecord {
    pub name: String,
    pub kind: ProcessingSetupKind,
    /// Canonical settings JSON for the editor named by `kind`.
    pub settings: serde_json::Value,
    pub created_unix_seconds: i64,
    pub updated_unix_seconds: i64,
    /// Fields a newer version wrote, kept so saving here does not drop them.
    #[serde(flatten)]
    pub other_fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingSetupsRegistry {
    pub schema_version: u32,
    pub setups: Vec<ProcessingSetupRecord>,
    /// The Color setup new color previews start from. `None` in a file
    /// written before defaults existed, which gets the seeded setup when it
    /// loads. A name that no longer matches a Color setup means the
    /// built-in pipeline.
    ///
    /// The file leaves out a mark on the setup named "Default": loading
    /// marks that setup again, and versions before 0.13, which refuse
    /// unknown fields, can still read the file.
    #[serde(default, skip_serializing_if = "marks_the_seed_name")]
    pub default_color: Option<String>,
    /// Fields a newer version wrote, kept so saving here does not drop them.
    #[serde(flatten)]
    pub other_fields: serde_json::Map<String, serde_json::Value>,
}

fn marks_the_seed_name(default_color: &Option<String>) -> bool {
    default_color
        .as_deref()
        .is_none_or(|name| same_name(name, DEFAULT_COLOR_SETUP_NAME))
}

/// Names for the seeded default Color setup after "Default".
fn other_default_color_names() -> impl Iterator<Item = String> {
    std::iter::once("Default color".to_string())
        .chain((2..).map(|number| format!("Default color {number}")))
}

impl Default for ProcessingSetupsRegistry {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            setups: Vec::new(),
            default_color: None,
            other_fields: serde_json::Map::new(),
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

/// The seeded default Color setup as PSF Guard added it: nobody has saved
/// over it, so it has no timestamps.
pub fn is_untouched_seed(setup: &ProcessingSetupRecord) -> bool {
    setup.created_unix_seconds == 0 && setup.updated_unix_seconds == 0
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

    /// The Color setup marked as the default, when the mark matches one.
    pub fn default_color_setup(&self) -> Option<&ProcessingSetupRecord> {
        self.default_color
            .as_deref()
            .and_then(|name| self.find(name))
            .filter(|setup| setup.kind == ProcessingSetupKind::Color)
    }

    /// Give a registry that has never had a default Color setup the seeded
    /// one, with `settings` as its pipeline. A Color setup the person already
    /// named "Default" becomes the default as it is. Its timestamps are zero
    /// until someone saves over it. Returns true when the registry changed;
    /// a registry with a default, even one since deleted, is left alone.
    pub fn seed_default_color(&mut self, settings: serde_json::Value) -> bool {
        if self.default_color.is_some() {
            return false;
        }
        let mut candidates = std::iter::once(DEFAULT_COLOR_SETUP_NAME.to_string())
            .chain(other_default_color_names());
        let name = loop {
            let candidate = candidates.next().expect("the candidates never end");
            match self.find(&candidate) {
                Some(setup) if setup.kind == ProcessingSetupKind::Color => {
                    self.default_color = Some(setup.name.clone());
                    return true;
                }
                Some(_) => continue,
                None => break candidate,
            }
        };
        if self.setups.len() >= MAX_SETUPS {
            return false;
        }
        self.setups.push(ProcessingSetupRecord {
            name: name.clone(),
            kind: ProcessingSetupKind::Color,
            settings,
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            other_fields: Default::default(),
        });
        self.setups.sort_by_key(|setup| setup.name.to_lowercase());
        self.default_color = Some(name);
        true
    }

    /// Mark a Color setup as the one new color previews start from.
    pub fn mark_default_color(&mut self, name: &str) -> Result<()> {
        let Some(setup) = self.find(name) else {
            bail!("No processing setup is named {name}");
        };
        if setup.kind != ProcessingSetupKind::Color {
            bail!(
                "Only a Color setup can be the default for new color previews; \"{}\" is a view setup",
                setup.name
            );
        }
        self.default_color = Some(setup.name.clone());
        Ok(())
    }

    /// Insert or replace by name (case-insensitive). Returns true when an
    /// existing setup was replaced. A replacement keeps the original creation
    /// time and the original spelling wins on case-only differences, so a
    /// re-import cannot silently fork "SHO" and "sho". The default Color
    /// setup can be replaced only by another Color setup, except that the
    /// seeded one nobody has edited moves to "Default color" to make room
    /// for a view setup named "Default", and stays the default.
    pub fn upsert(&mut self, mut record: ProcessingSetupRecord) -> Result<bool> {
        validate_setup_name(&record.name)?;
        if record.kind != ProcessingSetupKind::Color
            && let Some(marked) = self
                .default_color_setup()
                .filter(|marked| same_name(&marked.name, &record.name))
        {
            if !is_untouched_seed(marked) {
                bail!(
                    "\"{}\" is the default Color setup, so it cannot be replaced by a view setup. \
                     Save the view setup under another name.",
                    record.name
                );
            }
            let moved = other_default_color_names()
                .find(|name| self.find(name).is_none())
                .expect("the names never end");
            let seed = self
                .setups
                .iter_mut()
                .find(|setup| same_name(&setup.name, &record.name))
                .expect("the marked setup is in the list");
            seed.name = moved.clone();
            self.default_color = Some(moved);
            self.setups.sort_by_key(|setup| setup.name.to_lowercase());
        }
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
    /// The default Color setup cannot be removed while it is marked.
    pub fn remove(&mut self, name: &str) -> Result<bool> {
        if self
            .default_color_setup()
            .is_some_and(|marked| same_name(&marked.name, name))
        {
            bail!(
                "\"{name}\" is the default for new color previews. Mark another Color setup \
                 as the default before deleting it."
            );
        }
        let before = self.setups.len();
        self.setups.retain(|setup| !same_name(&setup.name, name));
        Ok(self.setups.len() != before)
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
            other_fields: Default::default(),
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
        assert!(!registry.remove("absent").unwrap());
        assert!(registry.remove("BRIGHT").unwrap());
        assert!(registry.setups.is_empty());
    }

    fn seeded() -> ProcessingSetupsRegistry {
        let mut registry = ProcessingSetupsRegistry::default();
        assert!(registry.seed_default_color(serde_json::json!({ "output_stretches": [] })));
        registry
    }

    #[test]
    fn the_built_in_pipeline_is_a_color_pipeline_for_every_channel() {
        let settings: serde_json::Value = serde_json::from_str(DEFAULT_COLOR_SETTINGS).unwrap();
        assert_eq!(settings["input_processing"], "channels");
        assert_eq!(settings["color_calibration"]["white_reference"], "solar");
        let roles = settings["input_stretches"].as_object().unwrap();
        assert_eq!(
            roles.keys().map(String::as_str).collect::<Vec<_>>(),
            ["blue", "green", "ha", "luminance", "oiii", "red", "sii"]
        );
    }

    #[test]
    fn a_registry_without_a_default_gets_one_seeded_once() {
        let mut registry = seeded();
        assert_eq!(registry.default_color.as_deref(), Some("Default"));
        let setup = registry.default_color_setup().unwrap();
        assert_eq!(setup.kind, ProcessingSetupKind::Color);
        assert_eq!(setup.updated_unix_seconds, 0);
        // Seeding again changes nothing, even after the seed is gone.
        assert!(!registry.seed_default_color(serde_json::json!({})));
        assert_eq!(registry.setups.len(), 1);
        registry
            .upsert(record("Deep SHO", ProcessingSetupKind::Color))
            .unwrap();
        registry.mark_default_color("deep sho").unwrap();
        assert!(registry.remove("Default").unwrap());
        assert!(!registry.seed_default_color(serde_json::json!({})));
        assert!(registry.find("Default").is_none());
    }

    #[test]
    fn seeding_uses_a_color_setup_already_named_default_and_steps_around_a_view_one() {
        let mut registry = ProcessingSetupsRegistry::default();
        registry
            .upsert(record("default", ProcessingSetupKind::Color))
            .unwrap();
        assert!(registry.seed_default_color(serde_json::json!({})));
        assert_eq!(registry.setups.len(), 1);
        assert_eq!(registry.default_color.as_deref(), Some("default"));
        assert_eq!(
            registry.setups[0].created_unix_seconds, 100,
            "kept as it was"
        );

        let mut registry = ProcessingSetupsRegistry::default();
        registry
            .upsert(record("Default", ProcessingSetupKind::View))
            .unwrap();
        assert!(registry.seed_default_color(serde_json::json!({})));
        assert_eq!(registry.default_color.as_deref(), Some("Default color"));
        assert_eq!(registry.setups.len(), 2);
    }

    #[test]
    fn an_old_file_loads_without_a_default_and_a_new_one_keeps_its_mark() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("processing-setups.json");
        std::fs::write(&path, r#"{ "schema_version": 1, "setups": [] }"#).unwrap();
        let mut registry = ProcessingSetupsRegistry::load(&path).unwrap();
        assert_eq!(registry.default_color, None);
        registry.seed_default_color(serde_json::json!({}));
        registry.save(&path).unwrap();
        let mut restored = ProcessingSetupsRegistry::load(&path).unwrap();
        assert!(restored.find("Default").is_some());
        restored.seed_default_color(serde_json::json!({}));
        assert!(restored.default_color_setup().is_some());
    }

    #[test]
    fn only_a_color_setup_can_be_marked_default() {
        let mut registry = seeded();
        registry
            .upsert(record("Bright", ProcessingSetupKind::View))
            .unwrap();
        assert!(registry.mark_default_color("Bright").is_err());
        assert!(registry.mark_default_color("absent").is_err());
        assert_eq!(registry.default_color.as_deref(), Some("Default"));
        registry
            .upsert(record("Deep SHO", ProcessingSetupKind::Color))
            .unwrap();
        registry.mark_default_color("DEEP sho").unwrap();
        assert_eq!(
            registry.default_color.as_deref(),
            Some("Deep SHO"),
            "stored spelling"
        );
    }

    #[test]
    fn the_default_cannot_be_deleted_or_replaced_by_a_view_setup() {
        let mut registry = seeded();
        assert!(registry.remove("default").is_err());
        // Saving a Color setup over it edits it and keeps the mark.
        assert!(registry
            .upsert(record("default", ProcessingSetupKind::Color))
            .unwrap());
        let setup = registry.default_color_setup().unwrap();
        assert_eq!(setup.name, "Default");
        assert_eq!(setup.updated_unix_seconds, 100);
        assert_eq!(setup.settings["model"]["type"], "auto-mtf");
        // Once edited, a view setup cannot take its name.
        assert!(registry
            .upsert(record("DEFAULT", ProcessingSetupKind::View))
            .is_err());
        assert!(registry.remove("default").is_err());
    }

    #[test]
    fn a_view_setup_named_default_moves_the_untouched_seed_aside() {
        let mut registry = seeded();
        registry
            .upsert(record("Default color", ProcessingSetupKind::View))
            .unwrap();
        assert!(!registry
            .upsert(record("default", ProcessingSetupKind::View))
            .unwrap());
        assert_eq!(
            registry.find("Default").unwrap().kind,
            ProcessingSetupKind::View
        );
        let seed = registry.default_color_setup().unwrap();
        assert_eq!(seed.name, "Default color 2");
        assert!(is_untouched_seed(seed));
        assert_eq!(registry.setups.len(), 3);
    }

    #[test]
    fn a_file_keeps_the_mark_on_default_only_by_loading_it_again() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("processing-setups.json");
        let mut registry = seeded();
        registry.save(&path).unwrap();
        // Versions before 0.13 refuse unknown fields: an unchanged mark is
        // not written, and loading marks "Default" again.
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(written.get("default_color").is_none(), "{written}");
        let mut restored = ProcessingSetupsRegistry::load(&path).unwrap();
        assert_eq!(restored.default_color, None);
        restored.seed_default_color(serde_json::json!({}));
        assert_eq!(restored.default_color.as_deref(), Some("Default"));
        assert_eq!(restored.setups.len(), 1);

        // Another setup marked is written.
        registry
            .upsert(record("Faint", ProcessingSetupKind::Color))
            .unwrap();
        registry.mark_default_color("faint").unwrap();
        registry.save(&path).unwrap();
        let restored = ProcessingSetupsRegistry::load(&path).unwrap();
        assert_eq!(restored.default_color.as_deref(), Some("Faint"));
    }

    #[test]
    fn a_file_from_a_newer_version_loads_and_keeps_its_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("processing-setups.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "someday": { "kept": true },
                "setups": [
                    { "name": "Bright", "kind": "view", "settings": {},
                      "created_unix_seconds": 1, "updated_unix_seconds": 1,
                      "someday_per_setup": 3 },
                ],
            }))
            .unwrap(),
        )
        .unwrap();
        let registry = ProcessingSetupsRegistry::load(&path).unwrap();
        assert_eq!(registry.setups[0].name, "Bright");
        registry.save(&path).unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["someday"]["kept"], true);
        assert_eq!(written["setups"][0]["someday_per_setup"], 3);
    }

    #[test]
    fn a_mark_that_matches_no_color_setup_does_not_block_deletes() {
        // A hand-edited file can mark a setup that is not there, or not a
        // Color setup: the built-in pipeline applies and nothing is locked.
        let mut registry = ProcessingSetupsRegistry {
            default_color: Some("Mono".into()),
            ..Default::default()
        };
        registry
            .upsert(record("Mono", ProcessingSetupKind::View))
            .unwrap();
        assert!(registry.default_color_setup().is_none());
        assert!(registry.remove("Mono").unwrap());
    }
}
