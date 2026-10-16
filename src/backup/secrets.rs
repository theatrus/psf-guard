//! Credentials PSF Guard keeps in the clear, and their removal from a
//! backup.
//!
//! A backup leaves them out unless a person asks to keep them:
//!
//! - outgoing peer keys (`peers[].token` in the registry), which grant
//!   sync access to another PSF Guard;
//! - the collaboration credentials file, left out whole because a file
//!   with its tokens blanked would not load;
//! - `token`, `password`, `secret` and `api_key` values in the server's
//!   TOML file, such as `[[remote_sync]] token`. Each becomes
//!   [`PLACEHOLDER`], which is too short to pass as a token, so a restored
//!   server says what is missing rather than accepting a known key.
//!
//! Hashes stay: browser account passwords and API tokens (`auth.json`) and
//! remote upload keys are stored only as salted hashes, and a restore
//! without them would lose every account.

use super::FileRole;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;

/// What a removed value becomes in the TOML file.
pub const PLACEHOLDER: &str = "REDACTED";

const TOML_SECRET_KEYS: &[&str] = &["token", "password", "secret", "api_key"];

/// Remove the secrets from one file's bytes. Returns the new bytes and what
/// was removed, in words; other files come back unchanged.
pub fn redact(role: FileRole, bytes: &[u8]) -> Result<(Vec<u8>, Vec<String>)> {
    match role {
        FileRole::Registry => {
            let mut value: serde_json::Value =
                serde_json::from_slice(bytes).context("reading the registry")?;
            let count = blank_json_tokens(&mut value);
            if count == 0 {
                return Ok((bytes.to_vec(), Vec::new()));
            }
            let mut text = serde_json::to_vec_pretty(&value)?;
            text.push(b'\n');
            Ok((text, vec![plural(count, "peer key")]))
        }
        FileRole::ServerConfig => {
            let text = std::str::from_utf8(bytes).context("reading the server config")?;
            let mut document: toml_edit::DocumentMut =
                text.parse().context("reading the server config")?;
            let count = blank_toml_item(document.as_item_mut());
            if count == 0 {
                return Ok((bytes.to_vec(), Vec::new()));
            }
            Ok((
                document.to_string().into_bytes(),
                vec![plural(count, "server token")],
            ))
        }
        _ => Ok((bytes.to_vec(), Vec::new())),
    }
}

/// What [`redact`] would remove from the file at `path`, for a plan.
pub fn would_redact(role: FileRole, path: &Path) -> Vec<String> {
    match role {
        FileRole::Registry | FileRole::ServerConfig => std::fs::read(path)
            .ok()
            .and_then(|bytes| redact(role, &bytes).ok())
            .map(|(_, removed)| removed)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn plural(count: usize, what: &str) -> String {
    if count == 1 {
        format!("1 {what}")
    } else {
        format!("{count} {what}s")
    }
}

/// Blank every non-empty `token` string below `peers`. Returns how many.
fn blank_json_tokens(value: &mut serde_json::Value) -> usize {
    let Some(peers) = value
        .get_mut("peers")
        .and_then(|peers| peers.as_array_mut())
    else {
        return 0;
    };
    let mut count = 0;
    for peer in peers {
        if let Some(token) = peer.get_mut("token")
            && token.as_str().is_some_and(|token| !token.is_empty())
        {
            *token = serde_json::Value::String(String::new());
            count += 1;
        }
    }
    count
}

fn blank_toml_item(item: &mut toml_edit::Item) -> usize {
    match item {
        toml_edit::Item::Table(table) => blank_toml_table(table),
        toml_edit::Item::ArrayOfTables(tables) => tables.iter_mut().map(blank_toml_table).sum(),
        toml_edit::Item::Value(value) => blank_toml_value(value),
        toml_edit::Item::None => 0,
    }
}

fn blank_toml_table(table: &mut toml_edit::Table) -> usize {
    let mut count = 0;
    for (key, item) in table.iter_mut() {
        if is_secret_key(key.get()) && item.as_str().is_some_and(|text| !text.is_empty()) {
            *item = toml_edit::value(PLACEHOLDER);
            count += 1;
        } else {
            count += blank_toml_item(item);
        }
    }
    count
}

fn blank_toml_value(value: &mut toml_edit::Value) -> usize {
    match value {
        toml_edit::Value::InlineTable(table) => {
            let mut count = 0;
            for (key, value) in table.iter_mut() {
                if is_secret_key(key.get()) && value.as_str().is_some_and(|text| !text.is_empty()) {
                    *value = toml_edit::Value::from(PLACEHOLDER);
                    count += 1;
                } else {
                    count += blank_toml_value(value);
                }
            }
            count
        }
        toml_edit::Value::Array(array) => array.iter_mut().map(blank_toml_value).sum(),
        _ => 0,
    }
}

fn is_secret_key(key: &str) -> bool {
    TOML_SECRET_KEYS.contains(&key)
}

/// The secrets PSF Guard holds now, for Settings to say what "include
/// secrets" would keep.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SecretsSummary {
    pub peer_keys: usize,
    pub collaboration_credentials: bool,
    pub server_config_tokens: usize,
}

impl SecretsSummary {
    pub fn any(&self) -> bool {
        self.peer_keys > 0 || self.collaboration_credentials || self.server_config_tokens > 0
    }
}

pub fn summary(locations: &super::Locations) -> SecretsSummary {
    let mut summary = SecretsSummary::default();
    let count = |removed: Vec<String>| -> usize {
        removed
            .first()
            .and_then(|line| line.split_whitespace().next())
            .and_then(|number| number.parse().ok())
            .unwrap_or(0)
    };
    for (role, _, path) in locations.meta_files() {
        match role {
            FileRole::Registry => summary.peer_keys = count(would_redact(role, &path)),
            FileRole::ServerConfig => {
                summary.server_config_tokens = count(would_redact(role, &path))
            }
            FileRole::CollaborationCredentials => {
                summary.collaboration_credentials = path.is_file()
            }
            _ => {}
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_keys_are_blanked_and_everything_else_kept() {
        let registry = br#"{
            "schema_version": 2,
            "databases": [{"id": "a", "name": "A", "db_path": "/a.sqlite",
              "remote_image_upload": {"token_salt": "s", "token_sha256": "h"}}],
            "peers": [
              {"id": "scope", "name": "Scope", "base_url": "https://x", "token": "psfg-secret-key"},
              {"id": "none", "name": "None", "base_url": "https://y", "token": ""}
            ],
            "future_field": {"kept": true}
        }"#;
        let (bytes, removed) = redact(FileRole::Registry, registry).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("psfg-secret-key"));
        assert!(text.contains("future_field"), "unknown fields survive");
        assert!(text.contains("token_sha256"), "hashes stay");
        assert_eq!(removed, vec!["1 peer key"]);
    }

    #[test]
    fn server_tokens_become_a_placeholder_too_short_to_use() {
        let config = r#"# my server
[server]
port = 3000

[[remote_sync]]
database = "rig"
token = "a-remote-sync-token-long-enough"

[[remote_upload]]
database = "rig"
image_dir = "/images"
token_file = "/run/secrets/upload"
"#;
        let (bytes, removed) = redact(FileRole::ServerConfig, config.as_bytes()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("a-remote-sync-token-long-enough"));
        assert!(text.contains("token = \"REDACTED\""));
        assert!(text.contains("# my server"), "comments survive");
        assert!(text.contains("token_file = \"/run/secrets/upload\""));
        assert_eq!(removed, vec!["1 server token"]);
        assert!(PLACEHOLDER.len() < crate::db_registry::RemoteImageUploadConfig::MIN_TOKEN_LENGTH);
    }

    #[test]
    fn a_file_without_secrets_comes_back_byte_for_byte() {
        let registry = br#"{"schema_version": 2, "databases": []}"#;
        let (bytes, removed) = redact(FileRole::Registry, registry).unwrap();
        assert_eq!(bytes, registry);
        assert!(removed.is_empty());
        let (bytes, _) = redact(FileRole::Auth, b"{\"users\": []}").unwrap();
        assert_eq!(bytes, b"{\"users\": []}");
    }
}
