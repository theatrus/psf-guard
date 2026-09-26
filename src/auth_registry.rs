//! Persistent browser users for server mode.
//!
//! This file stays separate from the database registry. Database management
//! can therefore update catalog paths without reading or replacing login
//! hashes. Tauri does not load this registry.

use anyhow::{Context, Result};
use argon2::{
    password_hash::{phc::PasswordHash, PasswordHasher, PasswordVerifier},
    Argon2,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fmt,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
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
    Ok(Argon2::default()
        .hash_password(password.as_bytes())
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

/// Every personal API token starts with this, so a leaked value is easy to
/// recognize and a request can tell a token from a session cookie.
pub const TOKEN_PREFIX: &str = "psfg_";
pub const MAX_TOKEN_LABEL_LENGTH: usize = 80;
pub const MAX_TOKEN_DAYS: u32 = 3650;

/// A personal API token for scripts and agents. The registry keeps only the
/// SHA-256 of the secret; the secret is shown once, when it is minted. A
/// token never grants more than its user has, and `read_only` narrows it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthTokenRecord {
    pub id: String,
    pub username: String,
    pub label: String,
    token_hash: String,
    #[serde(default)]
    pub read_only: bool,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds; `None` never expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

impl fmt::Debug for AuthTokenRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthTokenRecord")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("label", &self.label)
            .field("read_only", &self.read_only)
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .field("token_hash", &"[redacted]")
            .finish()
    }
}

impl AuthTokenRecord {
    /// Make a new token. Returns the secret to show once and the record to
    /// store.
    pub fn mint(
        username: &str,
        label: &str,
        read_only: bool,
        expires_in_days: Option<u32>,
    ) -> Result<(String, Self)> {
        validate_username(username)?;
        let label = normalize_token_label(label)?;
        if let Some(days) = expires_in_days
            && !(1..=MAX_TOKEN_DAYS).contains(&days)
        {
            anyhow::bail!("token expiry must be between 1 and {MAX_TOKEN_DAYS} days");
        }
        let random: [u8; 32] = rand::random();
        let secret = format!("{TOKEN_PREFIX}{}", hex_lower(&random));
        let now = unix_now();
        let record = Self {
            id: uuid::Uuid::new_v4().simple().to_string()[..16].to_string(),
            username: username.to_string(),
            label,
            token_hash: hash_token(&secret),
            read_only,
            created_at: now,
            expires_at: expires_in_days.map(|days| now + i64::from(days) * 86_400),
        };
        Ok((secret, record))
    }

    pub fn matches(&self, secret: &str) -> bool {
        constant_time_eq(self.token_hash.as_bytes(), hash_token(secret).as_bytes())
    }

    pub fn is_expired_at(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    fn validate(&self) -> Result<()> {
        if self.id.is_empty() || self.id.len() > 64 {
            anyhow::bail!("token id must be 1 to 64 bytes");
        }
        validate_username(&self.username)?;
        normalize_token_label(&self.label)?;
        if self.token_hash.len() != 64 || !self.token_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            anyhow::bail!("token '{}' has an invalid hash", self.id);
        }
        Ok(())
    }
}

pub fn hash_token(secret: &str) -> String {
    hex_lower(&Sha256::digest(secret.as_bytes()))
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (l, r)| acc | (l ^ r))
        == 0
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

pub fn normalize_token_label(label: &str) -> Result<String> {
    let label = label.trim();
    if label.is_empty() {
        anyhow::bail!("token label cannot be empty");
    }
    if label.chars().count() > MAX_TOKEN_LABEL_LENGTH {
        anyhow::bail!("token label cannot exceed {MAX_TOKEN_LABEL_LENGTH} characters");
    }
    if label.chars().any(char::is_control) {
        anyhow::bail!("token label cannot contain control characters");
    }
    Ok(label.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthRegistry {
    pub schema_version: u32,
    pub users: Vec<AuthUserRecord>,
    /// Personal API tokens. Older files have no field, which reads as none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tokens: Vec<AuthTokenRecord>,
}

impl Default for AuthRegistry {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            users: Vec::new(),
            tokens: Vec::new(),
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

    /// Remove a user and every token that belonged to them.
    pub fn remove(&mut self, username: &str) -> Result<()> {
        let old_len = self.users.len();
        self.users.retain(|user| user.username != username);
        if self.users.len() == old_len {
            anyhow::bail!("user '{username}' does not exist");
        }
        self.tokens.retain(|token| token.username != username);
        Ok(())
    }

    pub fn add_token(&mut self, token: AuthTokenRecord) -> Result<()> {
        if !self
            .users
            .iter()
            .any(|user| user.username == token.username)
        {
            anyhow::bail!("user '{}' does not exist", token.username);
        }
        if self.tokens.iter().any(|existing| existing.id == token.id) {
            anyhow::bail!("token id '{}' already exists", token.id);
        }
        self.tokens.push(token);
        Ok(())
    }

    /// Drop one token by id. `owner` restricts the removal to that user's
    /// tokens; an editor passes `None`.
    pub fn revoke_token(&mut self, id: &str, owner: Option<&str>) -> Result<AuthTokenRecord> {
        let index = self
            .tokens
            .iter()
            .position(|token| token.id == id && owner.is_none_or(|owner| token.username == owner))
            .ok_or_else(|| anyhow::anyhow!("token '{id}' does not exist"))?;
        Ok(self.tokens.remove(index))
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
        let mut token_ids = HashSet::new();
        for token in &self.tokens {
            token.validate()?;
            if !usernames.contains(token.username.as_str()) {
                anyhow::bail!(
                    "token '{}' belongs to unknown user '{}'",
                    token.id,
                    token.username
                );
            }
            if !token_ids.insert(token.id.as_str()) {
                anyhow::bail!("auth registry contains duplicate token '{}'", token.id);
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
    fn tokens_hash_expire_and_follow_their_user() {
        let mut registry = AuthRegistry::default();
        registry
            .add(
                AuthUserRecord::new("editor", AccessRole::ReadWrite, "editor-secret-1").unwrap(),
                false,
            )
            .unwrap();
        assert!(
            AuthTokenRecord::mint("nobody", "x", false, None).is_ok(),
            "minting does not know the registry; add_token checks the user"
        );
        let (secret, record) = AuthTokenRecord::mint("editor", " laptop ", true, Some(30)).unwrap();
        assert!(secret.starts_with(TOKEN_PREFIX));
        assert_eq!(secret.len(), TOKEN_PREFIX.len() + 64);
        assert_eq!(record.label, "laptop");
        assert!(record.matches(&secret));
        assert!(!record.matches(&format!("{secret}x")));
        assert!(!record.is_expired_at(record.created_at + 29 * 86_400));
        assert!(record.is_expired_at(record.created_at + 30 * 86_400));
        assert!(AuthTokenRecord::mint("editor", "", false, None).is_err());
        assert!(AuthTokenRecord::mint("editor", "x", false, Some(0)).is_err());

        let (_, stray) = AuthTokenRecord::mint("nobody", "stray", false, None).unwrap();
        assert!(registry.add_token(stray).is_err());
        let id = record.id.clone();
        registry.add_token(record.clone()).unwrap();
        assert!(registry.add_token(record).is_err(), "duplicate id");

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.json");
        registry.save(&path).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(!contents.contains(&secret), "the secret never reaches disk");
        let reloaded = AuthRegistry::load(&path).unwrap();
        assert_eq!(reloaded.tokens.len(), 1);
        assert!(reloaded.tokens[0].matches(&secret));
        assert!(format!("{:?}", reloaded.tokens[0]).contains("[redacted]"));

        let mut reloaded = reloaded;
        assert!(reloaded.revoke_token(&id, Some("someone-else")).is_err());
        reloaded.remove("editor").unwrap();
        assert!(
            reloaded.tokens.is_empty(),
            "a removed user takes its tokens"
        );
    }

    #[test]
    fn registry_without_tokens_field_still_loads_and_saves_without_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.json");
        let mut registry = AuthRegistry::default();
        registry
            .add(
                AuthUserRecord::new("editor", AccessRole::ReadWrite, "editor-secret-1").unwrap(),
                false,
            )
            .unwrap();
        registry.save(&path).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(!contents.contains("tokens"));
        assert!(AuthRegistry::load(&path).unwrap().tokens.is_empty());

        let orphan = serde_json::json!({
            "schema_version": 1,
            "users": [],
            "tokens": [{
                "id": "abc",
                "username": "ghost",
                "label": "x",
                "token_hash": "0".repeat(64),
                "created_at": 0
            }]
        });
        std::fs::write(&path, orphan.to_string()).unwrap();
        let error = AuthRegistry::load(&path).unwrap_err().to_string();
        assert!(error.contains("ghost"), "{error}");
    }

    #[test]
    fn legacy_password_hash_survives_registry_reload_and_password_reset() {
        // Generated by Argon2 0.5.3 with its defaults and salt b"legacy-salt-1234".
        const LEGACY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$bGVnYWN5LXNhbHQtMTIzNA$oiRPb73yYvhPDv1t+S72Rooz7LLuqT/YVb3QuowiYXU";
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.json");
        let fixture = serde_json::json!({
            "schema_version": 1,
            "users": [{
                "username": "legacy",
                "role": "read_only",
                "password_hash": LEGACY_HASH,
            }],
        });
        std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();

        let registry = AuthRegistry::load(&path).unwrap();
        assert!(registry.users[0].verify_password("legacy-user-password"));
        assert!(!registry.users[0].verify_password("wrong-password"));
        registry.save(&path).unwrap();

        let mut reloaded = AuthRegistry::load(&path).unwrap();
        assert_eq!(reloaded.users[0].password_hash(), LEGACY_HASH);
        reloaded.users[0]
            .set_password("replacement-user-password")
            .unwrap();
        reloaded.save(&path).unwrap();

        let updated = AuthRegistry::load(&path).unwrap();
        assert!(updated.users[0].verify_password("replacement-user-password"));
        assert!(!updated.users[0].verify_password("legacy-user-password"));
    }

    #[test]
    fn new_hashes_keep_argon2id_parameters_and_use_unique_random_salts() {
        let password = "long-view-secret";
        let first = hash_password_without_policy(password).unwrap();
        let second = hash_password_without_policy(password).unwrap();
        let first_hash = PasswordHash::new(&first).unwrap();
        let second_hash = PasswordHash::new(&second).unwrap();

        for hash in [&first_hash, &second_hash] {
            assert_eq!(hash.algorithm.as_str(), "argon2id");
            assert_eq!(hash.version, Some(19));
            assert_eq!(hash.params.as_str(), "m=19456,t=2,p=1");
            assert_eq!(hash.salt.as_ref().unwrap().len(), 16);
            assert_eq!(hash.hash.as_ref().unwrap().as_bytes().len(), 32);
        }
        assert_ne!(first_hash.salt, second_hash.salt);
        assert!(verify_password_hash(&first, password));
        assert!(verify_password_hash(&second, password));
    }

    #[test]
    fn malformed_or_incomplete_password_hashes_fail_verification() {
        for hash in [
            "",
            "not-a-password-hash",
            "$argon2id$v=19$m=19456,t=2,p=1",
            "$argon2id$v=19$m=19456,t=2,p=1$bGVnYWN5LXRlc3Qtc2FsdA",
        ] {
            assert!(!verify_password_hash(hash, "long-view-secret"));
        }
    }

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
