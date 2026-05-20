//! Slug helpers used to identify databases in the multi-DB server.
//!
//! Slugs are URL-safe strings, persisted in config, and used as the canonical
//! database id in API paths (`/api/db/<slug>/...`) and cache directories.
//!
//! The default slug for a freshly-added database is derived deterministically
//! from a hash of its canonical path, so re-adding the same `.sqlite` file
//! always lands on the same slug. Users may rename it to something friendly
//! like `imaging-rig` in the settings UI.

/// FNV-1a 32-bit hash. Stable across Rust versions; sufficient for an 8-hex
/// fingerprint of a path.
fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Compute the default slug for a database, given its path on disk.
///
/// Format: `db-<8 lowercase hex chars>`. The hex is FNV-1a 32-bit over the
/// canonicalized path (falling back to the as-given path if canonicalization
/// fails, e.g. the file doesn't exist yet at validation time).
pub fn compute_default_slug(db_path: &str) -> String {
    let canonical = std::fs::canonicalize(db_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| db_path.to_string());
    format!("db-{:08x}", fnv1a_32(canonical.as_bytes()))
}

/// Validate a user-supplied slug. Rules: `^[a-z0-9][a-z0-9-]{0,63}$`.
pub fn validate_slug(slug: &str) -> Result<(), String> {
    if slug.is_empty() {
        return Err("slug cannot be empty".into());
    }
    if slug.len() > 64 {
        return Err("slug must be 64 characters or fewer".into());
    }
    let mut chars = slug.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err("slug must start with a lowercase letter or digit".into());
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(format!(
                "slug may only contain lowercase letters, digits, and hyphens (saw '{}')",
                c
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_slug_is_deterministic() {
        let a = compute_default_slug("/tmp/example.sqlite");
        let b = compute_default_slug("/tmp/example.sqlite");
        assert_eq!(a, b);
        assert!(a.starts_with("db-"));
        assert_eq!(a.len(), 11); // "db-" + 8 hex
    }

    #[test]
    fn different_paths_yield_different_slugs() {
        let a = compute_default_slug("/tmp/a.sqlite");
        let b = compute_default_slug("/tmp/b.sqlite");
        assert_ne!(a, b);
    }

    #[test]
    fn validate_accepts_valid_slugs() {
        for s in ["imaging-rig", "db-a3f4b2c1", "x", "a1", "0", "abc-123-xyz"] {
            assert!(validate_slug(s).is_ok(), "should accept {:?}", s);
        }
    }

    #[test]
    fn validate_rejects_invalid_slugs() {
        for s in [
            "",
            "-leading-hyphen",
            "Has-Uppercase",
            "has_underscore",
            "has space",
            "has.dot",
        ] {
            assert!(validate_slug(s).is_err(), "should reject {:?}", s);
        }
    }

    #[test]
    fn validate_rejects_too_long() {
        let long = "a".repeat(65);
        assert!(validate_slug(&long).is_err());
        let max = "a".repeat(64);
        assert!(validate_slug(&max).is_ok());
    }
}
