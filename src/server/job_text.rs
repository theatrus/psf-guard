//! Words for job progress, shared by each job's status route and the
//! header's job rows, so a page and the header say the same thing.

/// A count with thousands separators, as `en-US` writes it: `1,867`.
pub(crate) fn grouped(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `one` for a count of 1, `many` otherwise.
pub(crate) fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        one.to_string()
    } else {
        many.to_string()
    }
}

/// Join the non-empty parts with ` · `.
pub(crate) fn joined<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 0–100 for `done` of `total`, or `None` before the total is known.
pub(crate) fn percent(done: usize, total: usize) -> Option<f64> {
    (total > 0).then(|| (done as f64 / total as f64 * 100.0).min(100.0))
}

/// [`percent`] for byte counts.
pub(crate) fn percent_u64(done: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| (done as f64 / total as f64 * 100.0).min(100.0))
}

/// `Checking files 812 of 2,140`, or `Checking files…` before the total
/// is known.
pub(crate) fn stage_count(verb: &str, done: usize, total: usize) -> String {
    if total == 0 {
        return format!("{verb}…");
    }
    format!("{verb} {} of {}", grouped(done.min(total)), grouped(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_as_en_us_writes_them() {
        assert_eq!(grouped(1867), "1,867");
        assert_eq!(grouped(999), "999");
        assert_eq!(stage_count("Merging", 37, 203), "Merging 37 of 203");
        assert_eq!(stage_count("Merging", 0, 0), "Merging…");
        assert_eq!(percent(1, 4), Some(25.0));
        assert_eq!(percent(1, 0), None);
        assert_eq!(joined(["a", "", "b"]), "a · b");
    }
}
