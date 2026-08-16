pub fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

/// Filter names as written by N.I.N.A. are free text; profile edits leave
/// catalogs with "Ha" beside "HA" beside "HA ". Grouping or matching on the
/// raw string silently splits one physical filter into several comparison
/// cohorts, so every filter comparison and grouping key goes through here.
/// Display keeps the raw name; only identity is normalized.
pub fn normalized_filter_name(name: &str) -> String {
    name.trim().to_ascii_uppercase()
}

/// Whether two filter names refer to the same physical filter.
pub fn filter_names_match(left: &str, right: &str) -> bool {
    normalized_filter_name(left) == normalized_filter_name(right)
}

pub fn extract_filename(metadata: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(metadata).ok()?;
    json.get("FileName").and_then(|f| f.as_str()).map(|path| {
        // Extract just the filename from the full path
        path.split(&['\\', '/'][..])
            .next_back()
            .unwrap_or(path)
            .to_string()
    })
}

#[cfg(test)]
mod filter_name_tests {
    use super::*;

    #[test]
    fn variants_of_one_filter_match() {
        assert!(filter_names_match("Ha", "HA"));
        assert!(filter_names_match(" HA ", "ha"));
        assert!(!filter_names_match("HA", "OIII"));
        assert_eq!(normalized_filter_name(" Ha "), "HA");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_string_short() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("test", 4), "test");
        assert_eq!(truncate_string("", 10), "");
    }

    #[test]
    fn test_truncate_string_exact_length() {
        assert_eq!(truncate_string("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_string_long() {
        assert_eq!(truncate_string("hello world", 8), "hello...");
        assert_eq!(
            truncate_string("this is a very long string", 10),
            "this is..."
        );
        assert_eq!(truncate_string("1234567890", 5), "12...");
    }

    #[test]
    fn test_truncate_string_edge_cases() {
        assert_eq!(truncate_string("abc", 3), "abc");
        assert_eq!(truncate_string("abcd", 3), "...");
    }

    #[test]
    fn test_extract_filename_unix_path() {
        let metadata = r#"{"FileName": "/home/user/images/test.fits"}"#;
        assert_eq!(extract_filename(metadata), Some("test.fits".to_string()));
    }

    #[test]
    fn test_extract_filename_windows_path() {
        let metadata = r#"{"FileName": "C:\\Users\\User\\Documents\\image.fit"}"#;
        assert_eq!(extract_filename(metadata), Some("image.fit".to_string()));
    }

    #[test]
    fn test_extract_filename_mixed_separators() {
        let metadata = r#"{"FileName": "C:\\Users/User\\Documents/subfolder\\final.fits"}"#;
        assert_eq!(extract_filename(metadata), Some("final.fits".to_string()));
    }

    #[test]
    fn test_extract_filename_no_path() {
        let metadata = r#"{"FileName": "simple.fits"}"#;
        assert_eq!(extract_filename(metadata), Some("simple.fits".to_string()));
    }

    #[test]
    fn test_extract_filename_missing_field() {
        let metadata = r#"{"OtherField": "value"}"#;
        assert_eq!(extract_filename(metadata), None);
    }

    #[test]
    fn test_extract_filename_invalid_json() {
        let metadata = "not json";
        assert_eq!(extract_filename(metadata), None);
    }

    #[test]
    fn test_extract_filename_empty_string() {
        let metadata = "";
        assert_eq!(extract_filename(metadata), None);
    }

    #[test]
    fn test_extract_filename_null_value() {
        let metadata = r#"{"FileName": null}"#;
        assert_eq!(extract_filename(metadata), None);
    }
}
