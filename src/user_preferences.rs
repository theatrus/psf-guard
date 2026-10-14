//! Each browser user's view preferences, kept on the server so they follow
//! the person to another browser or device.
//!
//! The document is small and closed: only the keys listed in [`KNOWN_KEYS`],
//! each with a type the browser's own sanitizer agrees with, and no more
//! than [`MAX_DOCUMENT_BYTES`] once written out. The file sits beside the
//! auth registry, keyed by username, and is replaced atomically on each
//! write, like the auth registry. It holds no credentials, so writing it
//! often, as the browser does when a person changes a view, never touches
//! the password file.
//!
//! Servers without user accounts, and the desktop app, do not use it: the
//! browser keeps those preferences in its own storage.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

use crate::auth_registry::validate_username;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
/// The largest preferences document one user may keep, as written JSON.
pub const MAX_DOCUMENT_BYTES: usize = 4096;
/// The largest request body the endpoint reads before parsing.
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// What a known key may hold.
#[derive(Debug, Clone, Copy)]
pub enum PreferenceKind {
    Bool,
    OneOf(&'static [&'static str]),
    Integer { min: i64, max: i64 },
}

/// The preferences the browser keeps for a person. The names and values
/// match `DisplayPreferences` in `static/src/hooks/useDisplayPreferences.ts`.
pub const KNOWN_KEYS: &[(&str, PreferenceKind)] = &[
    ("showNightChip", PreferenceKind::Bool),
    ("showAllChip", PreferenceKind::Bool),
    ("showCalibrationChip", PreferenceKind::Bool),
    ("advanceOnGrade", PreferenceKind::Bool),
    (
        "projectPickerGrouping",
        PreferenceKind::OneOf(&["activity", "database"]),
    ),
    (
        "libraryDensity",
        PreferenceKind::OneOf(&["compact", "detailed"]),
    ),
    ("imageOrder", PreferenceKind::OneOf(&["oldest", "newest"])),
    (
        "imageGrouping",
        PreferenceKind::OneOf(&["filter", "session", "date", "both"]),
    ),
    (
        "imageGroupingAllProjects",
        PreferenceKind::OneOf(&[
            "project",
            "project+filter",
            "project+date",
            "project+date+filter",
        ]),
    ),
    (
        "gridThumbnailSize",
        PreferenceKind::Integer {
            min: 150,
            max: 1200,
        },
    ),
    (
        "sequenceThumbnailSize",
        PreferenceKind::Integer {
            min: 150,
            max: 1200,
        },
    ),
];

/// Fields a newer build wrote that this one does not know, carried through
/// load and save untouched, as the auth registry does.
type UnknownFields = BTreeMap<String, Value>;

/// The known preferences out of a request, and the names of the keys it
/// dropped because this build does not know them. A known key with a value
/// of the wrong type or out of range is an error: that is a broken client,
/// not a newer one.
pub fn sanitize_preferences(
    input: &Map<String, Value>,
) -> Result<(Map<String, Value>, Vec<String>)> {
    let mut kept = Map::new();
    let mut ignored = Vec::new();
    for (key, value) in input {
        let Some((_, kind)) = KNOWN_KEYS.iter().find(|(known, _)| known == key) else {
            ignored.push(key.clone());
            continue;
        };
        let valid = match kind {
            PreferenceKind::Bool => value.is_boolean(),
            PreferenceKind::OneOf(choices) => value
                .as_str()
                .is_some_and(|choice| choices.contains(&choice)),
            PreferenceKind::Integer { min, max } => value
                .as_i64()
                .is_some_and(|number| (*min..=*max).contains(&number)),
        };
        if !valid {
            bail!("'{key}' has a value this preference cannot hold: {value}");
        }
        kept.insert(key.clone(), value.clone());
    }
    let size = serde_json::to_vec(&kept)
        .context("measuring preferences")?
        .len();
    if size > MAX_DOCUMENT_BYTES {
        bail!("preferences are limited to {MAX_DOCUMENT_BYTES} bytes; these are {size}");
    }
    ignored.sort();
    Ok((kept, ignored))
}

/// One user's saved preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredPreferences {
    pub preferences: Map<String, Value>,
    pub updated_unix_seconds: i64,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    extra: UnknownFields,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreferencesRegistry {
    pub schema_version: u32,
    #[serde(default)]
    pub users: BTreeMap<String, StoredPreferences>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    extra: UnknownFields,
}

impl Default for UserPreferencesRegistry {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            users: BTreeMap::new(),
            extra: UnknownFields::new(),
        }
    }
}

impl UserPreferencesRegistry {
    /// The preferences file beside a database registry: `user-preferences.json`
    /// next to the standard `config.json`, `<stem>.user-preferences.json`
    /// next to a custom one, as the auth registry is named.
    pub fn path_for_database_registry(database_registry_path: &Path) -> PathBuf {
        if database_registry_path
            .file_name()
            .and_then(|name| name.to_str())
            == Some("config.json")
        {
            return database_registry_path.with_file_name("user-preferences.json");
        }
        let stem = database_registry_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("config");
        database_registry_path.with_file_name(format!("{stem}.user-preferences.json"))
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("reading user preferences {}", path.display()))?;
        let registry: Self = serde_json::from_str(&contents)
            .with_context(|| format!("parsing user preferences {}", path.display()))?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        let contents = serde_json::to_vec_pretty(self).context("serializing user preferences")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("creating temporary file in {}", parent.display()))?;
        temporary
            .write_all(&contents)
            .context("writing temporary user preferences")?;
        temporary
            .write_all(b"\n")
            .context("finishing temporary user preferences")?;
        set_private_permissions(temporary.as_file(), temporary.path())?;
        temporary
            .as_file()
            .sync_all()
            .context("syncing temporary user preferences")?;
        temporary
            .persist(path)
            .map_err(|error| error.error)
            .with_context(|| format!("replacing user preferences {}", path.display()))?;
        Ok(())
    }

    pub fn get(&self, username: &str) -> Option<&StoredPreferences> {
        self.users.get(username)
    }

    /// Replace one user's preferences with an already sanitized document.
    pub fn set(
        &mut self,
        username: &str,
        preferences: Map<String, Value>,
        now_unix_seconds: i64,
    ) -> Result<&StoredPreferences> {
        validate_username(username)?;
        let (preferences, ignored) = sanitize_preferences(&preferences)?;
        if !ignored.is_empty() {
            bail!("unknown preferences: {}", ignored.join(", "));
        }
        let extra = self
            .users
            .remove(username)
            .map(|previous| previous.extra)
            .unwrap_or_default();
        self.users.insert(
            username.to_string(),
            StoredPreferences {
                preferences,
                updated_unix_seconds: now_unix_seconds,
                extra,
            },
        );
        Ok(&self.users[username])
    }

    /// Forget a user's preferences, as when the user is removed. Returns
    /// whether there were any.
    pub fn remove(&mut self, username: &str) -> bool {
        self.users.remove(username).is_some()
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            bail!(
                "unsupported user preferences schema {}; this build supports {}",
                self.schema_version,
                CURRENT_SCHEMA_VERSION
            );
        }
        for (username, stored) in &self.users {
            validate_username(username)?;
            let size = serde_json::to_vec(&stored.preferences)
                .context("measuring stored preferences")?
                .len();
            if size > MAX_DOCUMENT_BYTES {
                bail!("preferences for '{username}' exceed {MAX_DOCUMENT_BYTES} bytes");
            }
        }
        Ok(())
    }
}

/// Drop a removed user's preferences from the file beside a database
/// registry. A missing file has nothing to drop.
pub fn forget_user(database_registry_path: &Path, username: &str) -> Result<bool> {
    let path = UserPreferencesRegistry::path_for_database_registry(database_registry_path);
    if !path.exists() {
        return Ok(false);
    }
    let mut registry = UserPreferencesRegistry::load(&path)?;
    let removed = registry.remove(username);
    if removed {
        registry.save(&path)?;
    }
    Ok(removed)
}

#[cfg(unix)]
fn set_private_permissions(file: &std::fs::File, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting private permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_permissions(_file: &std::fs::File, _path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn keeps_known_keys_and_drops_the_rest() {
        let (kept, ignored) = sanitize_preferences(&map(json!({
            "imageOrder": "newest",
            "imageGrouping": "session",
            "showNightChip": false,
            "gridThumbnailSize": 450,
            "somethingNewer": 1,
            "aSecondUnknown": "x",
        })))
        .unwrap();
        assert_eq!(
            Value::Object(kept),
            json!({
                "imageOrder": "newest",
                "imageGrouping": "session",
                "showNightChip": false,
                "gridThumbnailSize": 450,
            })
        );
        assert_eq!(ignored, vec!["aSecondUnknown", "somethingNewer"]);
    }

    #[test]
    fn rejects_a_known_key_with_the_wrong_value() {
        for bad in [
            json!({ "imageOrder": "sideways" }),
            json!({ "imageOrder": 1 }),
            json!({ "showAllChip": "yes" }),
            json!({ "gridThumbnailSize": 149 }),
            json!({ "sequenceThumbnailSize": 1201 }),
            json!({ "sequenceThumbnailSize": 300.5 }),
            json!({ "imageGrouping": "project" }),
            json!({ "imageGroupingAllProjects": "filter" }),
        ] {
            assert!(sanitize_preferences(&map(bad.clone())).is_err(), "{bad}");
        }
    }

    #[test]
    fn every_known_key_fits_the_size_cap_at_once() {
        let mut every = Map::new();
        for (key, kind) in KNOWN_KEYS {
            let value = match kind {
                PreferenceKind::Bool => json!(true),
                PreferenceKind::OneOf(choices) => {
                    json!(choices.iter().max_by_key(|c| c.len()).unwrap())
                }
                PreferenceKind::Integer { max, .. } => json!(max),
            };
            every.insert((*key).to_string(), value);
        }
        let (kept, ignored) = sanitize_preferences(&every).unwrap();
        assert_eq!(kept.len(), KNOWN_KEYS.len());
        assert!(ignored.is_empty());
    }

    #[test]
    fn a_stored_document_over_the_cap_does_not_load() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("user-preferences.json");
        let huge = "x".repeat(MAX_DOCUMENT_BYTES);
        std::fs::write(
            &path,
            json!({
                "schema_version": 1,
                "users": { "alice": { "preferences": { "imageOrder": huge }, "updated_unix_seconds": 1 } },
            })
            .to_string(),
        )
        .unwrap();
        assert!(UserPreferencesRegistry::load(&path).is_err());
    }

    #[test]
    fn keeps_each_user_apart_and_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("user-preferences.json");
        let mut registry = UserPreferencesRegistry::default();
        registry
            .set("alice", map(json!({ "imageOrder": "newest" })), 10)
            .unwrap();
        registry
            .set("bob", map(json!({ "imageGrouping": "date" })), 20)
            .unwrap();
        registry.save(&path).unwrap();

        let loaded = UserPreferencesRegistry::load(&path).unwrap();
        assert_eq!(
            Value::Object(loaded.get("alice").unwrap().preferences.clone()),
            json!({ "imageOrder": "newest" })
        );
        assert_eq!(
            Value::Object(loaded.get("bob").unwrap().preferences.clone()),
            json!({ "imageGrouping": "date" })
        );
        assert_eq!(loaded.get("alice").unwrap().updated_unix_seconds, 10);
        assert!(loaded.get("carol").is_none());

        // Saving one user replaces only theirs.
        let mut loaded = loaded;
        loaded
            .set("alice", map(json!({ "imageOrder": "oldest" })), 30)
            .unwrap();
        assert_eq!(
            Value::Object(loaded.get("bob").unwrap().preferences.clone()),
            json!({ "imageGrouping": "date" })
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_and_replaced_whole() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("user-preferences.json");
        let mut registry = UserPreferencesRegistry::default();
        registry
            .set("alice", map(json!({ "libraryDensity": "detailed" })), 1)
            .unwrap();
        registry.save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // No temporary file is left beside it.
        let names: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("user-preferences.json")]
        );
    }

    #[test]
    fn refuses_another_schema_and_keeps_fields_from_a_newer_build() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("user-preferences.json");
        std::fs::write(
            &path,
            json!({ "schema_version": 2, "users": {} }).to_string(),
        )
        .unwrap();
        assert!(UserPreferencesRegistry::load(&path).is_err());

        std::fs::write(
            &path,
            json!({
                "schema_version": 1,
                "users": { "alice": { "preferences": {}, "updated_unix_seconds": 1, "pinned": [1] } },
                "later": true,
            })
            .to_string(),
        )
        .unwrap();
        let mut registry = UserPreferencesRegistry::load(&path).unwrap();
        registry
            .set("alice", map(json!({ "imageOrder": "newest" })), 2)
            .unwrap();
        registry.save(&path).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["later"], json!(true));
        assert_eq!(written["users"]["alice"]["pinned"], json!([1]));
        assert_eq!(
            written["users"]["alice"]["preferences"]["imageOrder"],
            json!("newest")
        );
    }

    #[test]
    fn names_the_file_beside_the_registry() {
        assert_eq!(
            UserPreferencesRegistry::path_for_database_registry(Path::new("/tmp/config.json")),
            Path::new("/tmp/user-preferences.json")
        );
        assert_eq!(
            UserPreferencesRegistry::path_for_database_registry(Path::new("/tmp/demo.json")),
            Path::new("/tmp/demo.user-preferences.json")
        );
    }

    #[test]
    fn forgetting_a_user_drops_only_theirs() {
        let directory = tempfile::tempdir().unwrap();
        let registry_path = directory.path().join("config.json");
        assert!(!forget_user(&registry_path, "alice").unwrap());
        let path = UserPreferencesRegistry::path_for_database_registry(&registry_path);
        let mut registry = UserPreferencesRegistry::default();
        registry
            .set("alice", map(json!({ "imageOrder": "newest" })), 1)
            .unwrap();
        registry
            .set("bob", map(json!({ "imageOrder": "oldest" })), 1)
            .unwrap();
        registry.save(&path).unwrap();
        assert!(forget_user(&registry_path, "alice").unwrap());
        let loaded = UserPreferencesRegistry::load(&path).unwrap();
        assert!(loaded.get("alice").is_none());
        assert!(loaded.get("bob").is_some());
    }
}
