//! Catalog-aware directory templates for remote image intake.
//!
//! Detection is deliberately a Settings-time operation. Upload requests only
//! render the persisted result and never walk an observatory image share.

use anyhow::{Context, Result};
use rusqlite::OpenFlags;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::directory_tree::DirectoryTree;
use crate::server::database_context::open_scheduler_connection_with_flags;

pub const MAX_DIRECTORY_TEMPLATE_BYTES: usize = 512;
const MAX_DIRECTORY_TEMPLATE_COMPONENTS: usize = 16;
const MAX_LAYOUT_SAMPLES: i64 = 2_000;

pub const DIRECTORY_TEMPLATE_TOKENS: &[&str] = &[
    "%TARGET%",
    "%PROJECT%",
    "%DATE%",
    "%NIGHT%",
    "%YEAR%",
    "%TYPE%",
    "%FILTER%",
    "%TELESCOPE%",
    "%CAMERA%",
    "%EXPOSURE%",
    "%GAIN%",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedDirectoryLayout {
    pub template: String,
    pub samples: usize,
}

#[derive(Debug)]
struct CatalogLayoutSample {
    metadata_path: String,
    project: String,
    target: String,
    filter: String,
    capture_time: Option<i64>,
}

/// Validate a relative directory-only template. The uploaded basename is
/// always appended separately by the server.
pub fn validate_directory_template(template: &str) -> Result<()> {
    let template = template.trim();
    if template.is_empty() {
        anyhow::bail!("remote upload directory template cannot be empty");
    }
    if template.len() > MAX_DIRECTORY_TEMPLATE_BYTES {
        anyhow::bail!(
            "remote upload directory template cannot exceed {MAX_DIRECTORY_TEMPLATE_BYTES} bytes"
        );
    }
    if template.starts_with(['/', '\\']) || template.contains(':') {
        anyhow::bail!("remote upload directory template must be a relative path");
    }

    let components = template.split(['/', '\\']).collect::<Vec<_>>();
    if components.len() > MAX_DIRECTORY_TEMPLATE_COMPONENTS
        || components
            .iter()
            .any(|component| component.is_empty() || matches!(*component, "." | ".."))
    {
        anyhow::bail!("remote upload directory template contains an invalid path component");
    }
    if template.chars().any(char::is_control) {
        anyhow::bail!("remote upload directory template cannot contain control characters");
    }

    for component in components {
        let mut remaining = component;
        while let Some(start) = remaining.find('%') {
            let after_start = &remaining[start + 1..];
            let Some(end) = after_start.find('%') else {
                anyhow::bail!("remote upload directory template contains an incomplete token");
            };
            let token = &remaining[start..start + end + 2];
            if !DIRECTORY_TEMPLATE_TOKENS.contains(&token) {
                anyhow::bail!("unknown remote upload directory token {token}");
            }
            remaining = &remaining[start + end + 2..];
        }
    }

    if !template.contains("%TARGET%") || !template.contains("%TYPE%") {
        anyhow::bail!("remote upload directory template must include %TARGET% and %TYPE%");
    }
    Ok(())
}

/// Scan the selected receive root and match catalog rows to real files. A
/// layout is returned only when one pattern has a unique lead; mixed catalogs
/// keep the operator's explicit fallback preset.
pub fn detect_catalog_directory_layout(
    database_path: &str,
    receive_root: &Path,
) -> Result<Option<DetectedDirectoryLayout>> {
    let started = std::time::Instant::now();
    let connection = open_scheduler_connection_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("opening catalog {} for layout detection", database_path))?;
    let samples = catalog_layout_samples(&connection)?;
    if samples.is_empty() {
        return Ok(None);
    }

    let tree = DirectoryTree::build(receive_root).with_context(|| {
        format!(
            "scanning remote upload receive directory {}",
            receive_root.display()
        )
    })?;
    let mut patterns = HashMap::<String, usize>::new();
    let mut matched = 0usize;
    for sample in &samples {
        let Some(path) = unambiguous_sample_path(&tree, receive_root, sample) else {
            continue;
        };
        let templates = templates_from_sample(receive_root, &path, sample);
        if templates.is_empty() {
            continue;
        }
        for template in templates {
            *patterns.entry(template).or_default() += 1;
        }
        matched += 1;
    }

    let mut ranked = patterns.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let detected = ranked.first().and_then(|(template, count)| {
        let runner_up = ranked.get(1).map(|(_, count)| *count).unwrap_or(0);
        (*count > runner_up).then(|| DetectedDirectoryLayout {
            template: template.clone(),
            samples: *count,
        })
    });
    // A single night's catalog can agree unanimously on a folder that is a
    // date the detector could not tie to any frame. Adopting that would file
    // every future upload under one fixed night (#399), so a template that
    // still carries a literal date is no layout at all.
    let detected = detected.filter(|layout| {
        let literal_date = layout
            .template
            .split('/')
            .any(|component| split_embedded_iso_date(component).is_some());
        if literal_date {
            tracing::warn!(
                root = %receive_root.display(),
                template = %layout.template,
                "Ignored a detected remote upload layout that contains a literal date"
            );
        }
        !literal_date
    });

    tracing::info!(
        root = %receive_root.display(),
        catalog_rows = samples.len(),
        matched_rows = matched,
        elapsed_ms = started.elapsed().as_millis(),
        template = detected.as_ref().map(|layout| layout.template.as_str()).unwrap_or("preset"),
        "Scanned remote image catalog layout"
    );
    Ok(detected)
}

fn catalog_layout_samples(connection: &rusqlite::Connection) -> Result<Vec<CatalogLayoutSample>> {
    let mut statement = connection.prepare(
        "SELECT ai.metadata, p.name, t.name, IFNULL(ai.filtername, ''), ai.acquireddate
         FROM acquiredimage ai
         JOIN project p ON p.Id = ai.projectId
         JOIN target t ON t.Id = ai.targetId
         WHERE ai.metadata IS NOT NULL
         ORDER BY ai.Id DESC
         LIMIT ?1",
    )?;
    let rows = statement.query_map([MAX_LAYOUT_SAMPLES], |row| {
        let metadata: String = row.get(0)?;
        Ok((
            metadata,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<i64>>(4)?,
        ))
    })?;
    let mut samples = Vec::new();
    for row in rows {
        let (metadata, project, target, filter, acquired_date) = row?;
        let metadata = serde_json::from_str::<serde_json::Value>(&metadata).ok();
        let Some(metadata_path) = metadata
            .as_ref()
            .and_then(|value| metadata_text(value, "FileName"))
            .map(str::to_string)
        else {
            continue;
        };
        samples.push(CatalogLayoutSample {
            metadata_path,
            project,
            target,
            filter,
            capture_time: acquired_date.or_else(|| {
                metadata
                    .as_ref()
                    .and_then(|value| metadata_text(value, "ExposureStartTime"))
                    .and_then(crate::commands::import::headers::parse_fits_datetime)
            }),
        });
    }
    Ok(samples)
}

fn unambiguous_sample_path(
    tree: &DirectoryTree,
    root: &Path,
    sample: &CatalogLayoutSample,
) -> Option<PathBuf> {
    let filename = sample.metadata_path.rsplit(['/', '\\']).next()?;
    let paths = tree.find_file(filename)?;
    let sanitized_target =
        super::remote_upload::upload_directory_component(&sample.target, "Unknown Target");
    let mut candidates = paths
        .iter()
        .filter(|path| {
            let Ok(relative) = path.strip_prefix(root) else {
                return false;
            };
            let components = relative_components(relative);
            components.len() >= 3
                && components.iter().any(|component| {
                    component.eq_ignore_ascii_case(&sample.target)
                        || component.eq_ignore_ascii_case(&sanitized_target)
                })
                && components
                    .iter()
                    .any(|component| component.eq_ignore_ascii_case("LIGHT"))
                && !components.iter().any(|component| {
                    matches!(
                        component.to_ascii_lowercase().as_str(),
                        "reject" | "rejected" | "processed" | "masters" | "master"
                    )
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    (candidates.len() == 1).then(|| candidates.remove(0))
}

fn templates_from_sample(root: &Path, path: &Path, sample: &CatalogLayoutSample) -> Vec<String> {
    let Ok(relative) = path.strip_prefix(root) else {
        return Vec::new();
    };
    let Some(parent) = relative.parent() else {
        return Vec::new();
    };
    let components = relative_components(parent);
    let (capture_date, observing_night) = sample_capture_dates(path, sample);
    let mut variants = vec![(Vec::<String>::new(), false, false)];

    for component in &components {
        let target_match = component.eq_ignore_ascii_case(&sample.target)
            || component.eq_ignore_ascii_case(&super::remote_upload::upload_directory_component(
                &sample.target,
                "Unknown Target",
            ));
        let project_match = component.eq_ignore_ascii_case(&sample.project)
            || component.eq_ignore_ascii_case(&super::remote_upload::upload_directory_component(
                &sample.project,
                "Unknown Project",
            ));
        let replacements = if target_match && project_match {
            vec![
                ("%TARGET%".to_string(), true, false),
                ("%PROJECT%".to_string(), false, false),
            ]
        } else if target_match {
            vec![("%TARGET%".to_string(), true, false)]
        } else if component.eq_ignore_ascii_case("LIGHT") {
            vec![("%TYPE%".to_string(), false, true)]
        } else if !sample.filter.trim().is_empty()
            && (component.eq_ignore_ascii_case(&sample.filter)
                || component.eq_ignore_ascii_case(
                    &super::remote_upload::upload_directory_component(&sample.filter, "NONE"),
                ))
        {
            vec![("%FILTER%".to_string(), false, false)]
        } else if project_match {
            vec![("%PROJECT%".to_string(), false, false)]
        } else if matching_observing_year(component, observing_night.as_deref()) {
            vec![("%YEAR%".to_string(), false, false)]
        } else if let Some((prefix, date, suffix)) = split_embedded_iso_date(component) {
            // The date may be the whole folder name or sit inside one, as in
            // N.I.N.A.'s `NIGHT_2025-12-14`. Either way it is the capture
            // date, the observing night, or a literal that stays literal.
            let mut dates = Vec::new();
            if capture_date.as_deref() == Some(date) {
                dates.push((format!("{prefix}%DATE%{suffix}"), false, false));
            }
            if observing_night.as_deref() == Some(date) {
                dates.push((format!("{prefix}%NIGHT%{suffix}"), false, false));
            }
            if dates.is_empty() {
                dates.push((component.to_string(), false, false));
            }
            dates
        } else {
            // A four-digit directory can be a manually named season. Preserve
            // it unless the catalog supplies evidence for a date transform.
            vec![(component.to_string(), false, false)]
        };

        let mut next = Vec::new();
        for (parts, has_target, has_type) in variants {
            for (replacement, marks_target, marks_type) in &replacements {
                let mut expanded = parts.clone();
                expanded.push(replacement.clone());
                next.push((
                    expanded,
                    has_target || *marks_target,
                    has_type || *marks_type,
                ));
            }
        }
        variants = next;
    }

    variants
        .into_iter()
        .filter_map(|(parts, has_target, has_type)| {
            let template = parts.join("/");
            (has_target && has_type && validate_directory_template(&template).is_ok())
                .then_some(template)
        })
        .collect()
}

fn sample_capture_dates(
    path: &Path,
    sample: &CatalogLayoutSample,
) -> (Option<String>, Option<String>) {
    let frame = crate::commands::import::headers::read_frame_meta(path);
    let header_date = frame
        .date_local
        .as_deref()
        .or(frame.date_obs.as_deref())
        .and_then(|value| value.get(..10))
        .filter(|value| is_iso_date(value))
        .map(str::to_string);
    let timestamp = frame
        .date_local
        .as_deref()
        .or(frame.date_obs.as_deref())
        .and_then(crate::commands::import::headers::parse_fits_datetime)
        .or(frame.timestamp)
        .or(sample.capture_time);
    let Some(timestamp) =
        timestamp.and_then(|value| chrono::DateTime::<chrono::Utc>::from_timestamp(value, 0))
    else {
        return (header_date, None);
    };
    (
        header_date.or_else(|| Some(timestamp.format("%Y-%m-%d").to_string())),
        Some(
            (timestamp - chrono::Duration::hours(12))
                .format("%Y-%m-%d")
                .to_string(),
        ),
    )
}

fn metadata_text<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value
        .as_object()?
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .and_then(|(_, value)| value.as_str())
}

fn relative_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// `prefix`, the first `YYYY-MM-DD` inside `component`, and `suffix` — so a
/// folder like `NIGHT_2025-12-14` can become `NIGHT_%NIGHT%`. `None` when the
/// component holds no calendar date.
fn split_embedded_iso_date(component: &str) -> Option<(&str, &str, &str)> {
    let bytes = component.as_bytes();
    (0..bytes.len().saturating_sub(9)).find_map(|start| {
        let end = start + 10;
        component
            .get(start..end)
            .filter(|candidate| is_iso_date(candidate))
            .map(|date| (&component[..start], date, &component[end..]))
    })
}

fn is_iso_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

fn matching_observing_year(component: &str, observing_night: Option<&str>) -> bool {
    component.len() == 4
        && component.bytes().all(|byte| byte.is_ascii_digit())
        && observing_night.and_then(|date| date.get(..4)) == Some(component)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_catalog(path: &Path) -> rusqlite::Connection {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE project (Id INTEGER PRIMARY KEY, name TEXT NOT NULL);
                 CREATE TABLE target (
                     Id INTEGER PRIMARY KEY,
                     projectId INTEGER NOT NULL,
                     name TEXT NOT NULL
                 );
                 CREATE TABLE acquiredimage (
                     Id INTEGER PRIMARY KEY,
                     projectId INTEGER NOT NULL,
                     targetId INTEGER NOT NULL,
                     filtername TEXT,
                     acquireddate INTEGER,
                     metadata TEXT NOT NULL
                 );",
            )
            .unwrap();
        connection
    }

    fn add_catalog_image(
        connection: &rusqlite::Connection,
        id: i64,
        project: &str,
        target: &str,
        filter: &str,
        filename: &str,
        capture_time: i64,
    ) {
        connection
            .execute(
                "INSERT OR IGNORE INTO project (Id, name) VALUES (?1, ?2)",
                rusqlite::params![id, project],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO target (Id, projectId, name) VALUES (?1, ?1, ?2)",
                rusqlite::params![id, target],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO acquiredimage
                     (Id, projectId, targetId, filtername, metadata, acquireddate)
                 VALUES (?1, ?1, ?1, ?2, ?3, ?4)",
                rusqlite::params![
                    id,
                    filter,
                    serde_json::json!({ "FileName": format!("D:\\remote\\{filename}") })
                        .to_string(),
                    capture_time,
                ],
            )
            .unwrap();
    }

    #[test]
    fn validates_only_safe_allowlisted_templates() {
        for template in [
            "%YEAR%/%TARGET%/%NIGHT%/%TYPE%",
            "%TARGET%/%DATE%/%TYPE%/%FILTER%",
            "%PROJECT%/%TARGET%/%TYPE%/%EXPOSURE%s_G%GAIN%",
        ] {
            validate_directory_template(template).unwrap();
        }
        for template in [
            "../%TARGET%/%TYPE%",
            "C:/%TARGET%/%TYPE%",
            "/%TARGET%/%TYPE%",
            "%TARGET%/%UNKNOWN%/%TYPE%",
            "%TARGET%/LIGHT",
        ] {
            assert!(validate_directory_template(template).is_err(), "{template}");
        }
    }

    #[test]
    fn recognizes_catalog_date_components() {
        assert!(is_iso_date("2026-08-30"));
        assert!(!is_iso_date("2026-13-30"));
        assert!(matching_observing_year("2026", Some("2026-08-30")));
        assert!(!matching_observing_year("2026", Some("2025-12-31")));
        assert_eq!(
            split_embedded_iso_date("NIGHT_2025-12-14"),
            Some(("NIGHT_", "2025-12-14", ""))
        );
        assert_eq!(
            split_embedded_iso_date("2025-12-14_session"),
            Some(("", "2025-12-14", "_session"))
        );
        assert_eq!(split_embedded_iso_date("LIGHT"), None);
        assert_eq!(split_embedded_iso_date("2025-13-40"), None);
    }

    #[test]
    fn a_date_inside_a_folder_name_becomes_a_token() {
        // #399: N.I.N.A.'s `NIGHT_$$DATEMINUS12$$` folders came out of
        // detection as `NIGHT_2025-12-14`, a literal, so a whole catalog's
        // uploads would have been filed under one night forever.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("receive");
        let db_path = temp.path().join("scheduler.sqlite");
        let connection = create_catalog(&db_path);
        for (id, night, captured, filename) in [
            (1, "2025-12-14", "2025-12-15T04:00:00", "cres-001.fits"),
            (2, "2025-12-15", "2025-12-16T04:00:00", "cres-002.fits"),
        ] {
            let directory = root
                .join("ZWO ASI2600MM Pro")
                .join("Crescent Nebula")
                .join(format!("NIGHT_{night}"))
                .join("O")
                .join("LIGHT");
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(filename), b"fixture").unwrap();
            add_catalog_image(
                &connection,
                id,
                "Crescent Nebula",
                "Crescent Nebula",
                "O",
                filename,
                crate::commands::import::headers::parse_fits_datetime(captured).unwrap(),
            );
        }
        drop(connection);
        let detected = detect_catalog_directory_layout(
            db_path.to_str().unwrap(),
            &dunce::canonicalize(root).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            detected.template,
            "ZWO ASI2600MM Pro/%TARGET%/NIGHT_%NIGHT%/%FILTER%/%TYPE%"
        );
        assert_eq!(detected.samples, 2);
    }

    #[test]
    fn a_layout_that_still_carries_a_literal_date_is_refused() {
        // Every frame agrees on a folder that is a date, but not the frames'
        // date: one night's catalog named after the previous evening, say.
        // Unanimous or not, a fixed date is not a layout.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("receive");
        let db_path = temp.path().join("scheduler.sqlite");
        let connection = create_catalog(&db_path);
        for (id, filename) in [(1, "a-001.fits"), (2, "a-002.fits")] {
            let directory = root.join("M31").join("2025-12-01").join("LIGHT");
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(filename), b"fixture").unwrap();
            add_catalog_image(
                &connection,
                id,
                "M31",
                "M31",
                "L",
                filename,
                crate::commands::import::headers::parse_fits_datetime("2025-12-15T04:00:00")
                    .unwrap(),
            );
        }
        drop(connection);
        let detected = detect_catalog_directory_layout(
            db_path.to_str().unwrap(),
            &dunce::canonicalize(root).unwrap(),
        )
        .unwrap();
        assert!(detected.is_none(), "got {detected:?}");
    }

    #[test]
    fn detects_the_observing_night_tree_from_real_catalog_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("_Source");
        let db_path = temp.path().join("scheduler.sqlite");
        let connection = create_catalog(&db_path);
        for (id, target, night, captured, filename) in [
            (
                1,
                "Pinwheel Galaxy",
                "2026-04-06",
                "2026-04-07T04:00:00",
                "m101-001.fits",
            ),
            (
                2,
                "Whirlpool Galaxy",
                "2026-04-07",
                "2026-04-08T04:00:00",
                "m51-001.fits",
            ),
        ] {
            let directory = root.join("2026").join(target).join(night).join("LIGHT");
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(filename), b"fixture").unwrap();
            add_catalog_image(
                &connection,
                id,
                target,
                target,
                "L",
                filename,
                crate::commands::import::headers::parse_fits_datetime(captured).unwrap(),
            );
        }
        drop(connection);

        let detected = detect_catalog_directory_layout(
            db_path.to_str().unwrap(),
            &dunce::canonicalize(root).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(detected.template, "%YEAR%/%TARGET%/%NIGHT%/%TYPE%");
        assert_eq!(detected.samples, 2);
    }

    #[test]
    fn detects_capture_date_without_guessing_night() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("_Source");
        let db_path = temp.path().join("scheduler.sqlite");
        let connection = create_catalog(&db_path);
        let filename = "m101-date.fits";
        let directory = root
            .join("2026")
            .join("Pinwheel Galaxy")
            .join("2026-04-07")
            .join("LIGHT");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(filename), b"fixture").unwrap();
        add_catalog_image(
            &connection,
            1,
            "Pinwheel Galaxy",
            "Pinwheel Galaxy",
            "L",
            filename,
            crate::commands::import::headers::parse_fits_datetime("2026-04-07T04:00:00").unwrap(),
        );
        drop(connection);

        let detected = detect_catalog_directory_layout(
            db_path.to_str().unwrap(),
            &dunce::canonicalize(root).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(detected.template, "%YEAR%/%TARGET%/%DATE%/%TYPE%");
    }

    #[test]
    fn preserves_a_manually_named_season_that_differs_from_capture_year() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("_Source");
        let db_path = temp.path().join("scheduler.sqlite");
        let connection = create_catalog(&db_path);
        let filename = "season.fits";
        let directory = root
            .join("2025")
            .join("M 31")
            .join("2026-01-02")
            .join("LIGHT");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(filename), b"fixture").unwrap();
        add_catalog_image(
            &connection,
            1,
            "M 31",
            "M 31",
            "L",
            filename,
            crate::commands::import::headers::parse_fits_datetime("2026-01-03T04:00:00").unwrap(),
        );
        drop(connection);

        let detected = detect_catalog_directory_layout(
            db_path.to_str().unwrap(),
            &dunce::canonicalize(root).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(detected.template, "2025/%TARGET%/%NIGHT%/%TYPE%");
    }

    #[test]
    fn detects_sanitized_project_target_and_filter_components() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("_Source");
        let db_path = temp.path().join("scheduler.sqlite");
        let connection = create_catalog(&db_path);
        let filename = "sanitized.fits";
        let directory = root
            .join("Project_One")
            .join("Target_One")
            .join("2026-04-06")
            .join("LIGHT")
            .join("Ha_OIII");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(filename), b"fixture").unwrap();
        add_catalog_image(
            &connection,
            1,
            "Project/One",
            "Target/One",
            "Ha/OIII",
            filename,
            crate::commands::import::headers::parse_fits_datetime("2026-04-07T04:00:00").unwrap(),
        );
        drop(connection);

        let detected = detect_catalog_directory_layout(
            db_path.to_str().unwrap(),
            &dunce::canonicalize(root).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            detected.template,
            "%PROJECT%/%TARGET%/%NIGHT%/%TYPE%/%FILTER%"
        );
    }

    #[test]
    fn identical_project_and_target_levels_leave_the_preset_in_control() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("_Source");
        let db_path = temp.path().join("scheduler.sqlite");
        let connection = create_catalog(&db_path);
        let filename = "m31-001.fits";
        let directory = root
            .join("M 31")
            .join("M 31")
            .join("2026-04-06")
            .join("LIGHT");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(filename), b"fixture").unwrap();
        add_catalog_image(
            &connection,
            1,
            "M 31",
            "M 31",
            "L",
            filename,
            crate::commands::import::headers::parse_fits_datetime("2026-04-07T04:00:00").unwrap(),
        );
        drop(connection);

        assert!(
            detect_catalog_directory_layout(
                db_path.to_str().unwrap(),
                &dunce::canonicalize(root).unwrap(),
            )
            .unwrap()
            .is_none(),
            "ambiguous project and target levels must not infer two target directories"
        );
    }

    #[test]
    fn ambiguous_evening_dates_leave_the_preset_in_control() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("_Source");
        let db_path = temp.path().join("scheduler.sqlite");
        let connection = create_catalog(&db_path);
        let filename = "m101-evening.fits";
        let directory = root
            .join("2026")
            .join("Pinwheel Galaxy")
            .join("2026-04-07")
            .join("LIGHT");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(filename), b"fixture").unwrap();
        add_catalog_image(
            &connection,
            1,
            "Pinwheel Galaxy",
            "Pinwheel Galaxy",
            "L",
            filename,
            crate::commands::import::headers::parse_fits_datetime("2026-04-07T20:00:00").unwrap(),
        );
        drop(connection);

        assert!(detect_catalog_directory_layout(
            db_path.to_str().unwrap(),
            &dunce::canonicalize(root).unwrap(),
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn an_empty_catalog_uses_the_preset_without_scanning_the_root() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("scheduler.sqlite");
        drop(create_catalog(&db_path));
        let missing_root = temp.path().join("does-not-exist");

        assert!(
            detect_catalog_directory_layout(db_path.to_str().unwrap(), &missing_root)
                .unwrap()
                .is_none()
        );
    }
}
