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
//! Every read-modify-write goes through [`update_file`], which holds an
//! advisory lock on a `.lock` file beside it, so the server and a CLI
//! `users remove` never lose each other's change.
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
    Integer {
        min: i64,
        max: i64,
    },
    /// A number from `min` to `max` in whole multiples of `step`.
    Number {
        min: f64,
        max: f64,
        step: f64,
    },
    /// A cut-off that may be off: `null`, or a number above zero and no
    /// more than `max`.
    Limit {
        max: f64,
    },
    /// A count cut-off that may be off: `null`, or a whole number from
    /// `min` to `max`.
    CountLimit {
        min: i64,
        max: i64,
    },
}

/// The most a scoring penalty may scale its evidence's built-in hit, as
/// `PenaltyScales` clamps it in `sequence_analysis`.
pub const MAX_SCORING_PENALTY: f64 = 2.0;
/// Penalties move in tenths, as the Scoring control's sliders do.
pub const SCORING_PENALTY_STEP: f64 = 0.1;
/// The highest Max HFR default, in pixels.
pub const MAX_HFR_REJECT_LIMIT: f64 = 100.0;
/// The highest Min stars default. A star count is whole.
pub const MAX_STAR_COUNT_REJECT_LIMIT: i64 = 1_000_000;

/// Within float noise of a whole multiple of `step`.
fn on_step(value: f64, step: f64) -> bool {
    let steps = value / step;
    (steps - steps.round()).abs() < 1e-6
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
        "timeMarks",
        PreferenceKind::OneOf(&["nights-hours", "nights", "off"]),
    ),
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
    // The scoring defaults each user kept before the server held one copy
    // (`server::scoring_defaults`). Still accepted, so an older browser's
    // save is not refused, and read once when a page copies an editor's
    // values to the server. See `SCORING_PREFERENCE_KEYS` in
    // `static/src/utils/scoring.ts`.
    (
        "scoringPenaltySatellite",
        PreferenceKind::Number {
            min: 0.0,
            max: MAX_SCORING_PENALTY,
            step: SCORING_PENALTY_STEP,
        },
    ),
    (
        "scoringPenaltyPointing",
        PreferenceKind::Number {
            min: 0.0,
            max: MAX_SCORING_PENALTY,
            step: SCORING_PENALTY_STEP,
        },
    ),
    (
        "scoringPenaltyTemporal",
        PreferenceKind::Number {
            min: 0.0,
            max: MAX_SCORING_PENALTY,
            step: SCORING_PENALTY_STEP,
        },
    ),
    (
        "scoringPenaltyHfrRise",
        PreferenceKind::Number {
            min: 0.0,
            max: MAX_SCORING_PENALTY,
            step: SCORING_PENALTY_STEP,
        },
    ),
    (
        "scoringPenaltyStarLoss",
        PreferenceKind::Number {
            min: 0.0,
            max: MAX_SCORING_PENALTY,
            step: SCORING_PENALTY_STEP,
        },
    ),
    (
        "scoringHfrRejectAbove",
        PreferenceKind::Limit {
            max: MAX_HFR_REJECT_LIMIT,
        },
    ),
    (
        "scoringStarCountRejectBelow",
        PreferenceKind::CountLimit {
            min: 1,
            max: MAX_STAR_COUNT_REJECT_LIMIT,
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
    let (kept, ignored) = sanitize(input, false)?;
    check_document_size(&kept)?;
    Ok((kept, ignored))
}

/// The known changes out of a PATCH, and the names of the keys it dropped.
/// A known key set to `null` stays in the result as `null`: it resets that
/// preference to the browser's default. Values are checked as in
/// [`sanitize_preferences`]; the merged document's size is checked when it
/// is applied.
pub fn sanitize_preference_changes(
    input: &Map<String, Value>,
) -> Result<(Map<String, Value>, Vec<String>)> {
    sanitize(input, true)
}

fn sanitize(
    input: &Map<String, Value>,
    allow_reset: bool,
) -> Result<(Map<String, Value>, Vec<String>)> {
    let mut kept = Map::new();
    let mut ignored = Vec::new();
    for (key, value) in input {
        let Some((_, kind)) = KNOWN_KEYS.iter().find(|(known, _)| known == key) else {
            ignored.push(key.clone());
            continue;
        };
        let valid = (allow_reset && value.is_null())
            || match kind {
                PreferenceKind::Bool => value.is_boolean(),
                PreferenceKind::OneOf(choices) => value
                    .as_str()
                    .is_some_and(|choice| choices.contains(&choice)),
                PreferenceKind::Integer { min, max } => value
                    .as_i64()
                    .is_some_and(|number| (*min..=*max).contains(&number)),
                PreferenceKind::Number { min, max, step } => value.as_f64().is_some_and(|number| {
                    (*min..=*max).contains(&number) && on_step(number - min, *step)
                }),
                PreferenceKind::Limit { max } => {
                    value.is_null()
                        || value
                            .as_f64()
                            .is_some_and(|number| number > 0.0 && number <= *max)
                }
                PreferenceKind::CountLimit { min, max } => {
                    value.is_null()
                        || value
                            .as_i64()
                            .is_some_and(|number| (*min..=*max).contains(&number))
                }
            };
        if !valid {
            bail!("'{key}' has a value this preference cannot hold: {value}");
        }
        kept.insert(key.clone(), value.clone());
    }
    ignored.sort();
    Ok((kept, ignored))
}

/// A document past [`MAX_DOCUMENT_BYTES`]: the request's fault, not the
/// server's.
#[derive(Debug)]
pub struct DocumentTooLarge {
    pub size: usize,
}

impl std::fmt::Display for DocumentTooLarge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "preferences are limited to {MAX_DOCUMENT_BYTES} bytes; these are {}",
            self.size
        )
    }
}

impl std::error::Error for DocumentTooLarge {}

fn check_document_size(preferences: &Map<String, Value>) -> Result<()> {
    let size = serde_json::to_vec(preferences)
        .context("measuring preferences")?
        .len();
    if size > MAX_DOCUMENT_BYTES {
        return Err(DocumentTooLarge { size }.into());
    }
    Ok(())
}

/// One user's saved preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredPreferences {
    pub preferences: Map<String, Value>,
    pub updated_unix_seconds: i64,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    extra: UnknownFields,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// Merge changes into one user's preferences: each key given replaces
    /// that preference, and a key set to `null` drops it, so the browser's
    /// default applies. Keys left out are kept, so two browsers that change
    /// different preferences never undo each other; for one key, the last
    /// change wins.
    pub fn patch(
        &mut self,
        username: &str,
        changes: Map<String, Value>,
        now_unix_seconds: i64,
    ) -> Result<&StoredPreferences> {
        validate_username(username)?;
        let (changes, ignored) = sanitize_preference_changes(&changes)?;
        if !ignored.is_empty() {
            bail!("unknown preferences: {}", ignored.join(", "));
        }
        let (mut preferences, extra) = match self.users.get(username) {
            Some(previous) => (previous.preferences.clone(), previous.extra.clone()),
            None => (Map::new(), UnknownFields::new()),
        };
        for (key, value) in changes {
            if value.is_null() {
                preferences.remove(&key);
            } else {
                preferences.insert(key, value);
            }
        }
        check_document_size(&preferences)?;
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

    /// Forget the preferences of every user `exists` does not know, as
    /// after a user was removed while this file could not be written.
    /// Returns the names dropped.
    pub fn retain_users(&mut self, exists: impl Fn(&str) -> bool) -> Vec<String> {
        let gone: Vec<String> = self
            .users
            .keys()
            .filter(|username| !exists(username))
            .cloned()
            .collect();
        for username in &gone {
            self.users.remove(username);
        }
        gone
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

/// Read, change, and write back the preferences file at `path` while
/// holding an advisory lock on `<path>.lock`, so a server and a CLI command
/// that both change it never lose each other's write. The file is only
/// written when `change` left it different.
///
/// The server also serializes its own writers with a mutex; the lock is for
/// other processes. A process that ignores the lock still never sees a torn
/// file, since every save is an atomic rename.
pub fn update_file<T>(
    path: &Path,
    change: impl FnOnce(&mut UserPreferencesRegistry) -> Result<T>,
) -> Result<T> {
    let _lock = lock_file(path)?;
    let mut registry = UserPreferencesRegistry::load(path)?;
    let before = registry.clone();
    let result = change(&mut registry)?;
    if registry != before {
        registry.save(path)?;
    }
    Ok(result)
}

fn lock_file(path: &Path) -> Result<std::fs::File> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    let lock_path = path.with_file_name(name);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    file.lock()
        .with_context(|| format!("locking {}", lock_path.display()))?;
    // Released when the file closes.
    Ok(file)
}

/// Drop a removed user's preferences from the file beside a database
/// registry. A missing file has nothing to drop.
pub fn forget_user(database_registry_path: &Path, username: &str) -> Result<bool> {
    let path = UserPreferencesRegistry::path_for_database_registry(database_registry_path);
    if !path.exists() {
        return Ok(false);
    }
    update_file(&path, |registry| Ok(registry.remove(username)))
}

/// Drop the preferences of every user `exists` does not know from the file
/// beside a database registry, as the server does when it starts. Returns
/// the names dropped.
pub fn forget_unknown_users(
    database_registry_path: &Path,
    exists: impl Fn(&str) -> bool,
) -> Result<Vec<String>> {
    let path = UserPreferencesRegistry::path_for_database_registry(database_registry_path);
    if !path.exists() {
        return Ok(Vec::new());
    }
    update_file(&path, |registry| Ok(registry.retain_users(exists)))
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
    fn a_patch_resets_or_sets_a_scoring_default() {
        // A null resets any scoring default, a penalty included; the browser
        // then reads it as built in, which for a limit is off.
        let (changes, ignored) = sanitize_preference_changes(&map(json!({
            "scoringPenaltySatellite": null,
            "scoringHfrRejectAbove": null,
            "scoringStarCountRejectBelow": 505,
        })))
        .unwrap();
        assert!(ignored.is_empty());
        assert_eq!(
            Value::Object(changes),
            json!({
                "scoringPenaltySatellite": null,
                "scoringHfrRejectAbove": null,
                "scoringStarCountRejectBelow": 505,
            })
        );
        for bad in [
            json!({ "scoringPenaltyPointing": 0.25 }),
            json!({ "scoringStarCountRejectBelow": 12.5 }),
            json!({ "scoringHfrRejectAbove": 0 }),
        ] {
            assert!(
                sanitize_preference_changes(&map(bad.clone())).is_err(),
                "{bad}"
            );
        }

        let mut registry = UserPreferencesRegistry::default();
        registry
            .set(
                "alice",
                map(json!({ "scoringHfrRejectAbove": 3, "scoringPenaltyTemporal": 0.5 })),
                1,
            )
            .unwrap();
        let (changes, _) =
            sanitize_preference_changes(&map(json!({ "scoringHfrRejectAbove": null }))).unwrap();
        registry.patch("alice", changes, 2).unwrap();
        assert_eq!(
            Value::Object(registry.get("alice").unwrap().preferences.clone()),
            json!({ "scoringPenaltyTemporal": 0.5 })
        );
    }

    #[test]
    fn checks_the_scoring_defaults() {
        // Penalties from 0 to 2 in tenths; Max HFR above zero up to its
        // cap; Min stars a whole count; either limit may be off.
        let (kept, ignored) = sanitize_preferences(&map(json!({
            "scoringPenaltySatellite": 0,
            "scoringPenaltyPointing": 1.5,
            "scoringPenaltyTemporal": 0.3,
            "scoringHfrRejectAbove": 2.55,
            "scoringStarCountRejectBelow": null,
            "scoringPenaltyGuiding": 1,
        })))
        .unwrap();
        assert_eq!(
            Value::Object(kept),
            json!({
                "scoringPenaltySatellite": 0,
                "scoringPenaltyPointing": 1.5,
                "scoringPenaltyTemporal": 0.3,
                "scoringHfrRejectAbove": 2.55,
                "scoringStarCountRejectBelow": null,
            })
        );
        assert_eq!(ignored, vec!["scoringPenaltyGuiding"]);

        // Each end of each range is a value it may hold.
        let (kept, _) = sanitize_preferences(&map(json!({
            "scoringPenaltySatellite": 2,
            "scoringHfrRejectAbove": MAX_HFR_REJECT_LIMIT,
            "scoringStarCountRejectBelow": 1,
        })))
        .unwrap();
        assert_eq!(kept.len(), 3);
        let (kept, _) = sanitize_preferences(&map(json!({
            "scoringStarCountRejectBelow": MAX_STAR_COUNT_REJECT_LIMIT,
        })))
        .unwrap();
        assert_eq!(kept.len(), 1);

        for bad in [
            json!({ "scoringPenaltySatellite": -0.1 }),
            json!({ "scoringPenaltyPointing": 2.1 }),
            json!({ "scoringPenaltyPointing": 0.25 }),
            json!({ "scoringPenaltyPointing": 1.05 }),
            json!({ "scoringPenaltyTemporal": "1" }),
            json!({ "scoringPenaltyTemporal": null }),
            json!({ "scoringPenaltySatellite": true }),
            json!({ "scoringHfrRejectAbove": 0 }),
            json!({ "scoringHfrRejectAbove": -2.5 }),
            json!({ "scoringHfrRejectAbove": 100.5 }),
            json!({ "scoringHfrRejectAbove": "off" }),
            json!({ "scoringStarCountRejectBelow": 12.5 }),
            json!({ "scoringStarCountRejectBelow": 0 }),
            json!({ "scoringStarCountRejectBelow": 1_000_001 }),
            json!({ "scoringStarCountRejectBelow": [500] }),
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
                PreferenceKind::Number { max, .. } => json!(max),
                // A third prints with every digit a double has.
                PreferenceKind::Limit { max } => json!(max / 3.0),
                PreferenceKind::CountLimit { max, .. } => json!(max),
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
    fn a_patch_changes_only_the_keys_it_names_and_null_resets_one() {
        let mut registry = UserPreferencesRegistry::default();
        registry
            .set(
                "alice",
                map(json!({ "imageOrder": "newest", "imageGrouping": "session", "showAllChip": false })),
                1,
            )
            .unwrap();
        let stored = registry
            .patch(
                "alice",
                map(json!({ "gridThumbnailSize": 450, "imageGrouping": null })),
                2,
            )
            .unwrap();
        assert_eq!(
            Value::Object(stored.preferences.clone()),
            json!({ "imageOrder": "newest", "showAllChip": false, "gridThumbnailSize": 450 })
        );
        assert_eq!(stored.updated_unix_seconds, 2);

        // A first patch starts an empty document; resetting a key nobody
        // set is no error.
        let stored = registry
            .patch(
                "bob",
                map(json!({ "libraryDensity": "detailed", "imageOrder": null })),
                3,
            )
            .unwrap();
        assert_eq!(
            Value::Object(stored.preferences.clone()),
            json!({ "libraryDensity": "detailed" })
        );
        assert_eq!(
            Value::Object(registry.get("alice").unwrap().preferences.clone()),
            json!({ "imageOrder": "newest", "showAllChip": false, "gridThumbnailSize": 450 })
        );
    }

    #[test]
    fn a_patch_is_checked_like_a_document() {
        let mut registry = UserPreferencesRegistry::default();
        registry
            .set("alice", map(json!({ "imageOrder": "newest" })), 1)
            .unwrap();
        for bad in [
            json!({ "imageOrder": "sideways" }),
            json!({ "gridThumbnailSize": 5000 }),
            json!({ "showAllChip": "no" }),
            json!({ "notAPreference": 1 }),
        ] {
            assert!(
                registry.patch("alice", map(bad.clone()), 2).is_err(),
                "{bad}"
            );
        }
        assert!(registry.patch("../alice", map(json!({})), 2).is_err());
        assert_eq!(registry.get("alice").unwrap().updated_unix_seconds, 1);

        // Changes keep the unknown keys apart: a PATCH may carry a newer
        // build's keys, which are named, not kept.
        let (changes, ignored) =
            sanitize_preference_changes(&map(json!({ "imageOrder": null, "later": 1 }))).unwrap();
        assert_eq!(Value::Object(changes), json!({ "imageOrder": null }));
        assert_eq!(ignored, vec!["later"]);
        // A whole document may not reset a key with null.
        assert!(sanitize_preferences(&map(json!({ "imageOrder": null }))).is_err());
    }

    #[test]
    fn a_patch_keeps_fields_from_a_newer_build() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("user-preferences.json");
        std::fs::write(
            &path,
            json!({
                "schema_version": 1,
                "users": { "alice": { "preferences": { "imageOrder": "newest" }, "updated_unix_seconds": 1, "pinned": [1] } },
            })
            .to_string(),
        )
        .unwrap();
        update_file(&path, |registry| {
            registry.patch("alice", map(json!({ "showNightChip": false })), 2)?;
            Ok(())
        })
        .unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["users"]["alice"]["pinned"], json!([1]));
        assert_eq!(
            written["users"]["alice"]["preferences"],
            json!({ "imageOrder": "newest", "showNightChip": false })
        );
    }

    #[test]
    fn retaining_known_users_drops_the_rest() {
        let mut registry = UserPreferencesRegistry::default();
        for name in ["alice", "bob", "carol"] {
            registry
                .set(name, map(json!({ "imageOrder": "newest" })), 1)
                .unwrap();
        }
        let gone = registry.retain_users(|name| name != "bob");
        assert_eq!(gone, vec!["bob"]);
        assert!(registry.get("bob").is_none());
        assert!(registry.get("alice").is_some() && registry.get("carol").is_some());
        assert!(registry.retain_users(|_| true).is_empty());
    }

    #[test]
    fn forgetting_unknown_users_rewrites_the_file_only_when_one_goes() {
        let directory = tempfile::tempdir().unwrap();
        let registry_path = directory.path().join("config.json");
        assert!(forget_unknown_users(&registry_path, |_| false)
            .unwrap()
            .is_empty());
        let path = UserPreferencesRegistry::path_for_database_registry(&registry_path);
        let mut registry = UserPreferencesRegistry::default();
        for name in ["alice", "bob"] {
            registry
                .set(name, map(json!({ "imageOrder": "newest" })), 1)
                .unwrap();
        }
        registry.save(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(forget_unknown_users(&registry_path, |_| true)
            .unwrap()
            .is_empty());
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
        assert_eq!(
            forget_unknown_users(&registry_path, |name| name == "alice").unwrap(),
            vec!["bob"]
        );
        let loaded = UserPreferencesRegistry::load(&path).unwrap();
        assert!(loaded.get("alice").is_some());
        assert!(loaded.get("bob").is_none());
    }

    #[test]
    fn writers_holding_the_file_lock_never_lose_a_change() {
        // Each thread stands in for another process: no shared mutex, only
        // the lock file. Every patch names a different user and key, so a
        // lost update would show as a missing one.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("user-preferences.json");
        let keys = [
            ("showNightChip", json!(false)),
            ("showAllChip", json!(false)),
            ("showCalibrationChip", json!(false)),
            ("advanceOnGrade", json!(false)),
            ("libraryDensity", json!("detailed")),
            ("imageOrder", json!("newest")),
        ];
        std::thread::scope(|scope| {
            for user in ["alice", "bob"] {
                for (key, value) in &keys {
                    let path = &path;
                    scope.spawn(move || {
                        update_file(path, |registry| {
                            registry.patch(user, map(json!({ *key: value.clone() })), 1)?;
                            Ok(())
                        })
                        .unwrap();
                    });
                }
            }
        });
        let loaded = UserPreferencesRegistry::load(&path).unwrap();
        for user in ["alice", "bob"] {
            let preferences = &loaded.get(user).unwrap().preferences;
            for (key, value) in &keys {
                assert_eq!(preferences.get(*key), Some(value), "{user} {key}");
            }
        }
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
