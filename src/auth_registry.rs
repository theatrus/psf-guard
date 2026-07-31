//! Persistent browser users for server mode.
//!
//! This file stays separate from the database registry. Database management
//! can therefore update catalog paths without reading or replacing login
//! hashes. Tauri does not load this registry.

use anyhow::{Context, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fmt,
    io::Write,
    path::{Path, PathBuf},
};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
pub const MIN_PASSWORD_LENGTH: usize = 12;
pub const MAX_PASSWORD_LENGTH: usize = 1024;
pub const MAX_EMAIL_LENGTH: usize = 254;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessRole {
    ReadOnly,
    ReadWrite,
}

impl fmt::Display for AccessRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthUserRecord {
    pub username: String,
    pub role: AccessRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    password_hash: String,
}

impl fmt::Debug for AuthUserRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthUserRecord")
            .field("username", &self.username)
            .field("role", &self.role)
            .field("email", &self.email)
            .field("password_hash", &"[redacted]")
            .finish()
    }
}

impl AuthUserRecord {
    pub fn new(username: &str, role: AccessRole, password: &str) -> Result<Self> {
        Self::new_with_email(username, role, None, password)
    }

    pub fn new_with_email(
        username: &str,
        role: AccessRole,
        email: Option<&str>,
        password: &str,
    ) -> Result<Self> {
        validate_username(username)?;
        validate_password(password)?;
        let password_hash = hash_password_without_policy(password)?;
        Ok(Self {
            username: username.to_string(),
            role,
            email: normalize_email(email)?,
            password_hash,
        })
    }

    pub(crate) fn password_hash(&self) -> &str {
        &self.password_hash
    }

    pub fn set_password(&mut self, password: &str) -> Result<()> {
        validate_password(password)?;
        self.password_hash = hash_password_without_policy(password)?;
        Ok(())
    }

    pub fn set_email(&mut self, email: Option<&str>) -> Result<()> {
        self.email = normalize_email(email)?;
        Ok(())
    }

    #[cfg(test)]
    fn verify_password(&self, password: &str) -> bool {
        verify_password_hash(&self.password_hash, password)
    }

    fn validate(&self) -> Result<()> {
        validate_username(&self.username)?;
        normalize_email(self.email.as_deref())?;
        let hash = PasswordHash::new(&self.password_hash).map_err(|error| {
            anyhow::anyhow!("invalid password hash for '{}': {error}", self.username)
        })?;
        if !hash.algorithm.as_str().starts_with("argon2") {
            anyhow::bail!("password hash for '{}' is not Argon2", self.username);
        }
        Ok(())
    }
}

pub(crate) fn hash_password_without_policy(password: &str) -> Result<String> {
    let salt = SaltString::encode_b64(uuid::Uuid::new_v4().as_bytes())
        .map_err(|error| anyhow::anyhow!("creating password salt: {error}"))?;
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| anyhow::anyhow!("hashing password: {error}"))?
        .to_string())
}

pub(crate) fn verify_password_hash(password_hash: &str, password: &str) -> bool {
    let Ok(hash) = PasswordHash::new(password_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthRegistry {
    pub schema_version: u32,
    pub users: Vec<AuthUserRecord>,
}

impl Default for AuthRegistry {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            users: Vec::new(),
        }
    }
}

impl AuthRegistry {
    pub fn path_for_database_registry(database_registry_path: &Path) -> PathBuf {
        if database_registry_path
            .file_name()
            .and_then(|name| name.to_str())
            == Some("config.json")
        {
            return database_registry_path.with_file_name("auth.json");
        }
        let stem = database_registry_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("config");
        database_registry_path.with_file_name(format!("{stem}.auth.json"))
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("reading auth registry {}", path.display()))?;
        let registry: Self = serde_json::from_str(&contents)
            .with_context(|| format!("parsing auth registry {}", path.display()))?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        let contents = serde_json::to_vec_pretty(self).context("serializing auth registry")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("creating temporary file in {}", parent.display()))?;
        temporary
            .write_all(&contents)
            .context("writing temporary auth registry")?;
        temporary
            .write_all(b"\n")
            .context("finishing temporary auth registry")?;
        set_private_permissions(temporary.as_file(), temporary.path())?;
        temporary
            .as_file()
            .sync_all()
            .context("syncing temporary auth registry")?;
        temporary
            .persist(path)
            .map_err(|error| error.error)
            .with_context(|| format!("replacing auth registry {}", path.display()))?;
        Ok(())
    }

    pub fn add(&mut self, user: AuthUserRecord, replace: bool) -> Result<()> {
        if let Some(existing) = self
            .users
            .iter_mut()
            .find(|existing| existing.username == user.username)
        {
            if !replace {
                anyhow::bail!(
                    "user '{}' already exists; pass --replace to update it",
                    user.username
                );
            }
            *existing = user;
        } else {
            self.users.push(user);
        }
        self.users
            .sort_by(|left, right| left.username.cmp(&right.username));
        Ok(())
    }

    pub fn remove(&mut self, username: &str) -> Result<()> {
        let old_len = self.users.len();
        self.users.retain(|user| user.username != username);
        if self.users.len() == old_len {
            anyhow::bail!("user '{username}' does not exist");
        }
        Ok(())
    }

    pub fn find_mut(&mut self, username: &str) -> Option<&mut AuthUserRecord> {
        self.users.iter_mut().find(|user| user.username == username)
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported auth registry schema {}; this build supports {}",
                self.schema_version,
                CURRENT_SCHEMA_VERSION
            );
        }
        let mut usernames = HashSet::new();
        for user in &self.users {
            user.validate()?;
            if !usernames.insert(user.username.as_str()) {
                anyhow::bail!("auth registry contains duplicate user '{}'", user.username);
            }
        }
        Ok(())
    }
}

pub fn validate_username(username: &str) -> Result<()> {
    if username.is_empty() || username != username.trim() {
        anyhow::bail!("username must be non-empty and cannot start or end with whitespace");
    }
    if username.len() > 128 {
        anyhow::bail!("username cannot exceed 128 bytes");
    }
    if username
        .chars()
        .any(|character| matches!(character, ':' | '/' | '\\') || character.is_control())
    {
        anyhow::bail!("username cannot contain ':', '/', '\\', or control characters");
    }
    Ok(())
}

pub fn validate_password(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD_LENGTH || password.len() > MAX_PASSWORD_LENGTH {
        anyhow::bail!(
            "password must be at least {MIN_PASSWORD_LENGTH} characters and no more than \
             {MAX_PASSWORD_LENGTH} bytes"
        );
    }
    if password.chars().any(char::is_control) {
        anyhow::bail!("password cannot contain control characters");
    }
    Ok(())
}

pub fn normalize_email(email: Option<&str>) -> Result<Option<String>> {
    let Some(email) = email.map(str::trim).filter(|email| !email.is_empty()) else {
        return Ok(None);
    };
    if email.len() > MAX_EMAIL_LENGTH {
        anyhow::bail!("email cannot exceed {MAX_EMAIL_LENGTH} bytes");
    }
    if email.chars().any(char::is_whitespace) || email.chars().any(char::is_control) {
        anyhow::bail!("email cannot contain whitespace or control characters");
    }
    let Some((local, domain)) = email.split_once('@') else {
        anyhow::bail!("email must contain one '@'");
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        anyhow::bail!("email must contain one '@' with text on both sides");
    }
    Ok(Some(email.to_string()))
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

    #[test]
    fn hashes_round_trip_without_exposing_passwords() {
        let user = AuthUserRecord::new_with_email(
            "viewer",
            AccessRole::ReadOnly,
            Some(" viewer@example.com "),
            "long-view-secret",
        )
        .unwrap();
        assert!(user.verify_password("long-view-secret"));
        assert!(!user.verify_password("wrong-secret-value"));
        assert_eq!(user.email.as_deref(), Some("viewer@example.com"));
        let json = serde_json::to_string(&user).unwrap();
        assert!(!json.contains("long-view-secret"));
        assert!(!format!("{user:?}").contains(user.password_hash()));
    }

    #[test]
    fn email_is_optional_and_rejects_malformed_values() {
        assert_eq!(normalize_email(None).unwrap(), None);
        assert_eq!(normalize_email(Some("  ")).unwrap(), None);
        assert!(normalize_email(Some("missing-at.example.com")).is_err());
        assert!(normalize_email(Some("two@@example.com")).is_err());
        assert!(normalize_email(Some("space @example.com")).is_err());
    }

    #[test]
    fn custom_database_registry_gets_distinct_auth_name() {
        assert_eq!(
            AuthRegistry::path_for_database_registry(Path::new("/tmp/config.json")),
            Path::new("/tmp/auth.json")
        );
        assert_eq!(
            AuthRegistry::path_for_database_registry(Path::new("/tmp/demo.json")),
            Path::new("/tmp/demo.auth.json")
        );
    }

    #[test]
    fn save_load_add_replace_and_remove() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.json");
        let mut registry = AuthRegistry::default();
        registry
            .add(
                AuthUserRecord::new("viewer", AccessRole::ReadOnly, "long-view-secret").unwrap(),
                false,
            )
            .unwrap();
        assert!(registry
            .add(
                AuthUserRecord::new("viewer", AccessRole::ReadWrite, "long-edit-secret",).unwrap(),
                false,
            )
            .is_err());
        registry
            .add(
                AuthUserRecord::new("viewer", AccessRole::ReadWrite, "long-edit-secret").unwrap(),
                true,
            )
            .unwrap();
        registry.save(&path).unwrap();

        let mut loaded = AuthRegistry::load(&path).unwrap();
        assert_eq!(loaded.users.len(), 1);
        assert_eq!(loaded.users[0].role, AccessRole::ReadWrite);
        assert!(loaded.users[0].verify_password("long-edit-secret"));
        loaded.remove("viewer").unwrap();
        assert!(loaded.remove("viewer").is_err());
    }
}
