//! Worker tokens: one per worker, minted in Settings › Workers. The secret
//! is shown once and stored only as its SHA-256.

use crate::db_registry::WorkerTokenRecord;
use crate::remote_work::protocol::TOKEN_PREFIX;

/// The longest worker name.
pub const MAX_NAME_LENGTH: usize = 64;

/// A new token for the worker `name`: the secret, and the record the
/// registry keeps.
pub fn mint(name: &str) -> Result<(String, WorkerTokenRecord), String> {
    let name = normalize_name(name)?;
    let random: [u8; 32] = rand::random();
    let secret = format!("{TOKEN_PREFIX}{}", crate::remote_work::hex(&random));
    let id_bytes: [u8; 8] = rand::random();
    Ok((
        secret.clone(),
        WorkerTokenRecord {
            id: crate::remote_work::hex(&id_bytes),
            name,
            token_hash: crate::auth_registry::hash_token(&secret),
            created_at: crate::auth_registry::unix_now(),
        },
    ))
}

/// A worker name, trimmed: one to [`MAX_NAME_LENGTH`] characters with no
/// control characters.
pub fn normalize_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a name for the worker".into());
    }
    if name.chars().count() > MAX_NAME_LENGTH {
        return Err(format!(
            "A worker name can be at most {MAX_NAME_LENGTH} characters"
        ));
    }
    if name.chars().any(char::is_control) {
        return Err("A worker name cannot contain control characters".into());
    }
    Ok(name.to_string())
}

/// Compare two hashes in time that does not depend on where they differ.
pub fn same_hash(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |acc, (l, r)| acc | (l ^ r))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_token_matches_only_its_own_hash() {
        let (secret, record) = mint("  studio pc ").unwrap();
        assert!(secret.starts_with(TOKEN_PREFIX));
        assert_eq!(record.name, "studio pc");
        assert!(same_hash(
            &record.token_hash,
            &crate::auth_registry::hash_token(&secret)
        ));
        let (other, _) = mint("other").unwrap();
        assert!(!same_hash(
            &record.token_hash,
            &crate::auth_registry::hash_token(&other)
        ));
        assert!(!record.token_hash.contains(&secret[TOKEN_PREFIX.len()..]));
    }

    #[test]
    fn names_are_required_short_and_printable() {
        assert!(mint("").is_err());
        assert!(mint("   ").is_err());
        assert!(mint(&"x".repeat(MAX_NAME_LENGTH + 1)).is_err());
        assert!(mint("a\nb").is_err());
        assert!(mint(&"x".repeat(MAX_NAME_LENGTH)).is_ok());
    }
}
