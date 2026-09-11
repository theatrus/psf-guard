//! Project-local exposure families shared by the image grid and stack builders.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::{
    api::ApiResponse,
    extract::DbContext,
    handlers::{require_database_management_allowed, AppError},
    state::AppState,
};

const SETTINGS_TABLE: &str = "psf_guard_project_processing";
const EXPOSURE_SPLIT_RATIO: f64 = 2.0;

type ExposureStream = Vec<(i32, Option<f64>)>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectProcessingSettings {
    pub split_exposure_groups: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExposureGroup {
    pub key: String,
    pub label: String,
    pub min_seconds: Option<f64>,
    pub max_seconds: Option<f64>,
}

pub struct ProjectExposureGroups {
    pub settings: ProjectProcessingSettings,
    pub by_image: HashMap<i32, ExposureGroup>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CatalogRevision {
    data_version: i64,
    total_changes: u64,
}

impl CatalogRevision {
    fn read(conn: &Connection) -> Result<Self, AppError> {
        Ok(Self {
            data_version: conn
                .query_row("PRAGMA data_version", [], |row| row.get(0))
                .map_err(AppError::db)?,
            total_changes: conn.total_changes(),
        })
    }
}

#[derive(Default)]
pub(crate) struct ProjectExposureGroupsCache {
    entries: HashMap<i32, (CatalogRevision, Arc<ProjectExposureGroups>)>,
}

impl ProjectExposureGroupsCache {
    /// A connection reopen starts a new data-version namespace. The caller
    /// holds the connection mutex while clearing, before another read can run.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// The caller already owns this context's connection mutex. Always take it
/// before the cache mutex, including during connection replacement.
pub fn cached_project_groups(
    ctx: &super::database_context::DatabaseContext,
    conn: &Connection,
    project_id: i32,
) -> Result<Arc<ProjectExposureGroups>, AppError> {
    let revision = CatalogRevision::read(conn)?;
    let mut cache = ctx
        .exposure_groups_cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((cached_revision, groups)) = cache.entries.get(&project_id)
        && *cached_revision == revision
    {
        return Ok(Arc::clone(groups));
    }
    let groups = Arc::new(load_project_groups(conn, project_id)?);
    // A second connection may commit while the two read queries run. Do not
    // retain that raced result under a revision which never described it.
    if CatalogRevision::read(conn)? == revision {
        cache
            .entries
            .insert(project_id, (revision, Arc::clone(&groups)));
    } else {
        cache.entries.remove(&project_id);
    }
    Ok(groups)
}

fn project_key(conn: &Connection, project_id: i32) -> Result<String, AppError> {
    let mut statement = conn
        .prepare("PRAGMA table_info(project)")
        .map_err(AppError::db)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(AppError::db)?;
    let has_guid = columns
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::db)?
        .iter()
        .any(|name| name.eq_ignore_ascii_case("guid"));
    let sql = if has_guid {
        "SELECT guid FROM project WHERE Id=?1"
    } else {
        "SELECT NULL FROM project WHERE Id=?1"
    };
    let guid: Option<String> = conn
        .query_row(sql, [project_id], |row| row.get(0))
        .optional()
        .map_err(AppError::db)?
        .ok_or(AppError::NotFound)?;
    if let Some(guid) = guid
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE lower(trim(guid))=lower(?1)",
                [guid],
                |row| row.get(0),
            )
            .map_err(AppError::db)?;
        if count == 1 {
            Ok(format!("guid:{}", guid.to_ascii_lowercase()))
        } else {
            // Corrupt duplicate GUIDs must not couple two projects' settings.
            Ok(format!("local-id:{project_id}"))
        }
    } else {
        // Legacy catalogs without GUIDs retain explicitly local settings.
        Ok(format!("local-id:{project_id}"))
    }
}

pub fn load_settings(
    conn: &Connection,
    project_id: i32,
) -> Result<ProjectProcessingSettings, AppError> {
    let key = project_key(conn, project_id)?;
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [SETTINGS_TABLE],
            |row| row.get(0),
        )
        .map_err(AppError::db)?;
    if !exists {
        return Ok(ProjectProcessingSettings::default());
    }
    let split_exposure_groups = conn
        .query_row(
            "SELECT split_exposure_groups FROM psf_guard_project_processing WHERE project_key=?1",
            [key],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(AppError::db)?
        .unwrap_or(false);
    Ok(ProjectProcessingSettings {
        split_exposure_groups,
    })
}

fn save_settings(
    conn: &mut Connection,
    project_id: i32,
    settings: ProjectProcessingSettings,
) -> Result<(), AppError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(AppError::db)?;
    let key = project_key(&tx, project_id)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS psf_guard_project_processing (
            project_key TEXT PRIMARY KEY,
            split_exposure_groups INTEGER NOT NULL CHECK(split_exposure_groups IN (0,1))
        )",
    )
    .map_err(AppError::db)?;
    tx.execute(
        "INSERT INTO psf_guard_project_processing(project_key,split_exposure_groups)
         VALUES (?1,?2) ON CONFLICT(project_key) DO UPDATE
         SET split_exposure_groups=excluded.split_exposure_groups",
        params![key, settings.split_exposure_groups],
    )
    .map_err(AppError::db)?;
    tx.commit().map_err(AppError::db)
}

pub fn exposure_seconds_from_metadata(metadata_json: &str) -> Option<f64> {
    let metadata: serde_json::Value = serde_json::from_str(metadata_json).ok()?;
    ["ExposureDuration", "ExposureTime", "EXPTIME"]
        .iter()
        .find_map(|key| {
            let value = &metadata[key];
            value
                .as_f64()
                .or_else(|| value.as_str()?.trim().parse().ok())
                .filter(|value| value.is_finite() && *value > 0.0)
        })
}

fn seconds_label(seconds: f64) -> String {
    if seconds < 0.001 {
        return format!("{seconds} s");
    }
    let value = format!("{seconds:.3}");
    format!("{} s", value.trim_end_matches('0').trim_end_matches('.'))
}

fn assign_stream(values: &mut [(i32, Option<f64>)], result: &mut HashMap<i32, ExposureGroup>) {
    values.sort_by(|a, b| {
        a.1.unwrap_or(0.0)
            .total_cmp(&b.1.unwrap_or(0.0))
            .then(a.0.cmp(&b.0))
    });
    let mut start = 0;
    while start < values.len() {
        let minimum = values[start].1;
        let mut end = start + 1;
        // Bound the complete family, not adjacent gaps: intermediate exposures
        // must not bridge short and long integrations into one family.
        while end < values.len()
            && match (minimum, values[end].1) {
                (Some(min), Some(value)) => value / min < EXPOSURE_SPLIT_RATIO,
                (None, None) => true,
                _ => false,
            }
        {
            end += 1;
        }
        let maximum = values[end - 1].1;
        // A newly arrived frame with slightly less exposure must not rename
        // an existing family and discard its resumable integration history.
        let anchor_id = values[start..end].iter().map(|(id, _)| *id).min().unwrap();
        let (key, label) = match (minimum, maximum) {
            (Some(min), Some(max)) => (
                format!("exposure-{anchor_id}"),
                if min == max {
                    seconds_label(min)
                } else {
                    format!("{} - {}", seconds_label(min), seconds_label(max))
                },
            ),
            _ => ("unknown".into(), "Unknown exposure".into()),
        };
        let group = ExposureGroup {
            key,
            label,
            min_seconds: minimum,
            max_seconds: maximum,
        };
        for (id, _) in &values[start..end] {
            result.insert(*id, group.clone());
        }
        start = end;
    }
}

/// Resolve against the full catalog population, before status/date/selection
/// filters. A one-channel rebuild therefore cannot change a frame's identity.
pub fn load_project_groups(
    conn: &Connection,
    project_id: i32,
) -> Result<ProjectExposureGroups, AppError> {
    let settings = load_settings(conn, project_id)?;
    let mut by_image = HashMap::new();
    if settings.split_exposure_groups {
        let mut statement = conn.prepare(
            "SELECT Id,targetid,COALESCE(filtername,''),metadata FROM acquiredimage WHERE projectid=?1",
        ).map_err(AppError::db)?;
        let rows = statement
            .query_map([project_id], |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, i32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(AppError::db)?;
        let mut streams: BTreeMap<(i32, String), ExposureStream> = BTreeMap::new();
        for row in rows {
            let (id, target, filter, metadata) = row.map_err(AppError::db)?;
            streams.entry((target, filter)).or_default().push((
                id,
                metadata.as_deref().and_then(exposure_seconds_from_metadata),
            ));
        }
        for stream in streams.values_mut() {
            assign_stream(stream, &mut by_image);
        }
    }
    Ok(ProjectExposureGroups { settings, by_image })
}

pub async fn get_project_settings(
    ctx: DbContext,
    Path((_db, project_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<ProjectProcessingSettings>>, AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    Ok(Json(ApiResponse::success(load_settings(
        &conn, project_id,
    )?)))
}

pub async fn update_project_settings(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db, project_id)): Path<(String, i32)>,
    Json(settings): Json<ProjectProcessingSettings>,
) -> Result<Json<ApiResponse<ProjectProcessingSettings>>, AppError> {
    require_database_management_allowed(&state)?;
    let conn = ctx.db();
    let mut conn = conn.lock().map_err(AppError::db)?;
    save_settings(&mut conn, project_id, settings)?;
    Ok(Json(ApiResponse::success(settings)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE project(Id INTEGER PRIMARY KEY,guid TEXT);
            INSERT INTO project VALUES(1,'project-one'),(2,'project-two');
            CREATE TABLE acquiredimage(Id INTEGER PRIMARY KEY,projectid INTEGER,targetid INTEGER,filtername TEXT,metadata TEXT);
            INSERT INTO acquiredimage VALUES
            (1,1,10,'R','{\"ExposureDuration\":10}'),
            (2,1,10,'R','{\"ExposureDuration\":10.01}'),
            (3,1,10,'R','{\"ExposureDuration\":19}'),
            (4,1,10,'R','{\"ExposureDuration\":30}'),
            (5,1,10,'R','{\"ExposureDuration\":300}'),
            (6,1,10,'R','{\"ExposureDuration\":0}'),
            (7,1,10,'R','bad-json'),
            (8,1,11,'R','{\"ExposureDuration\":12}'),
            (9,1,10,'G','{\"ExposureDuration\":12}'),
            (10,2,20,'R','{\"ExposureDuration\":10}');").unwrap();
        conn
    }

    #[test]
    fn reads_default_off_without_writing_schema() {
        let conn = fixture();
        let groups = load_project_groups(&conn, 1).unwrap();
        assert!(!groups.settings.split_exposure_groups);
        assert!(groups.by_image.is_empty());
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name=?1",
                [SETTINGS_TABLE],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        assert!(matches!(load_settings(&conn, 999), Err(AppError::NotFound)));
    }

    #[test]
    fn cached_groups_reuse_maps_and_refresh_after_local_changes() {
        let ctx = super::super::database_context::DatabaseContext::new_for_test(fixture());
        let db = ctx.db();
        let mut conn = db.lock().unwrap();
        let disabled = cached_project_groups(&ctx, &conn, 1).unwrap();
        assert!(Arc::ptr_eq(
            &disabled,
            &cached_project_groups(&ctx.clone(), &conn, 1).unwrap()
        ));
        save_settings(
            &mut conn,
            1,
            ProjectProcessingSettings {
                split_exposure_groups: true,
            },
        )
        .unwrap();
        let first = cached_project_groups(&ctx, &conn, 1).unwrap();
        assert!(!Arc::ptr_eq(&disabled, &first));
        assert_eq!(first.by_image[&5].min_seconds, Some(300.0));
        assert!(Arc::ptr_eq(
            &first,
            &cached_project_groups(&ctx, &conn, 1).unwrap()
        ));
        conn.execute(
            "UPDATE acquiredimage SET metadata='{\"ExposureDuration\":600}' WHERE Id=5",
            [],
        )
        .unwrap();
        let updated = cached_project_groups(&ctx, &conn, 1).unwrap();
        assert!(!Arc::ptr_eq(&first, &updated));
        assert_eq!(updated.by_image[&5].min_seconds, Some(600.0));
        save_settings(&mut conn, 1, ProjectProcessingSettings::default()).unwrap();
        assert!(cached_project_groups(&ctx, &conn, 1)
            .unwrap()
            .by_image
            .is_empty());
    }

    #[test]
    fn cached_groups_refresh_after_an_external_writer_commits() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.sqlite");
        let mut writer = Connection::open(&path).unwrap();
        writer.execute_batch("CREATE TABLE project(Id INTEGER PRIMARY KEY,guid TEXT);
            INSERT INTO project VALUES(1,'one');
            CREATE TABLE acquiredimage(Id INTEGER PRIMARY KEY,projectid INTEGER,targetid INTEGER,filtername TEXT,metadata TEXT);
            INSERT INTO acquiredimage VALUES(1,1,10,'R','{\"ExposureDuration\":30}');").unwrap();
        save_settings(
            &mut writer,
            1,
            ProjectProcessingSettings {
                split_exposure_groups: true,
            },
        )
        .unwrap();
        let ctx = super::super::database_context::DatabaseContext::new_for_test(
            Connection::open(&path).unwrap(),
        );
        let db = ctx.db();
        let conn = db.lock().unwrap();
        let first = cached_project_groups(&ctx, &conn, 1).unwrap();
        writer
            .execute(
                "UPDATE acquiredimage SET metadata='{\"ExposureDuration\":300}' WHERE Id=1",
                [],
            )
            .unwrap();
        let updated = cached_project_groups(&ctx, &conn, 1).unwrap();
        assert!(!Arc::ptr_eq(&first, &updated));
        assert_eq!(updated.by_image[&1].min_seconds, Some(300.0));
        assert!(Arc::ptr_eq(
            &updated,
            &cached_project_groups(&ctx, &conn, 1).unwrap()
        ));
    }

    #[test]
    fn bounds_families_and_isolates_unknowns_targets_filters_and_projects() {
        let mut conn = fixture();
        save_settings(
            &mut conn,
            1,
            ProjectProcessingSettings {
                split_exposure_groups: true,
            },
        )
        .unwrap();
        let groups = load_project_groups(&conn, 1).unwrap().by_image;
        assert_eq!(groups[&1], groups[&2]);
        assert_eq!(groups[&1], groups[&3]);
        assert_ne!(groups[&1].key, groups[&4].key);
        assert_ne!(groups[&4].key, groups[&5].key);
        assert_eq!(groups[&6].key, "unknown");
        assert_eq!(groups[&6], groups[&7]);
        assert_eq!(groups[&8].min_seconds, Some(12.0));
        assert_eq!(groups[&9].min_seconds, Some(12.0));
        assert!(!groups.contains_key(&10));
        assert!(load_project_groups(&conn, 2).unwrap().by_image.is_empty());
        save_settings(&mut conn, 1, ProjectProcessingSettings::default()).unwrap();
        assert!(load_project_groups(&conn, 1).unwrap().by_image.is_empty());
    }

    #[test]
    fn exact_twofold_difference_starts_a_new_family() {
        let mut groups = HashMap::new();
        assign_stream(
            &mut [
                (1, Some(0.1)),
                (2, Some(0.2)),
                (3, Some(0.399)),
                (4, Some(0.4)),
            ],
            &mut groups,
        );
        assert_ne!(groups[&1].key, groups[&2].key);
        assert_eq!(groups[&2].key, groups[&3].key);
        assert_ne!(groups[&3].key, groups[&4].key);
    }

    #[test]
    fn grouping_is_deterministic_and_keeps_key_when_a_family_grows() {
        let mut first = HashMap::new();
        let mut second = HashMap::new();
        assign_stream(
            &mut [(2, Some(300.0)), (1, Some(10.0)), (3, Some(11.0))],
            &mut first,
        );
        assign_stream(
            &mut [
                (3, Some(11.0)),
                (1, Some(10.0)),
                (4, Some(12.0)),
                (2, Some(300.0)),
            ],
            &mut second,
        );
        for id in [1, 2, 3] {
            assert_eq!(first[&id].key, second[&id].key);
        }
    }

    #[test]
    fn settings_follow_guid_and_do_not_leak_to_reused_ids() {
        let mut conn = fixture();
        save_settings(
            &mut conn,
            1,
            ProjectProcessingSettings {
                split_exposure_groups: true,
            },
        )
        .unwrap();
        conn.execute("UPDATE project SET Id=3 WHERE Id=1", [])
            .unwrap();
        assert!(load_settings(&conn, 3).unwrap().split_exposure_groups);
        conn.execute("INSERT INTO project VALUES(1,'replacement')", [])
            .unwrap();
        assert!(!load_settings(&conn, 1).unwrap().split_exposure_groups);
    }

    #[test]
    fn small_new_exposure_variations_preserve_existing_family_identity() {
        let mut before = HashMap::new();
        let mut after = HashMap::new();
        assign_stream(
            &mut [(1, Some(30.0)), (2, Some(30.1)), (3, Some(300.0))],
            &mut before,
        );
        assign_stream(
            &mut [
                (1, Some(30.0)),
                (2, Some(30.1)),
                (3, Some(300.0)),
                (4, Some(29.9)),
                (5, Some(299.9)),
            ],
            &mut after,
        );
        for id in [1, 2, 3] {
            assert_eq!(before[&id].key, after[&id].key);
        }
        assert_eq!(after[&1].min_seconds, Some(29.9));
        assert_eq!(after[&3].min_seconds, Some(299.9));
    }

    #[test]
    fn legacy_project_schema_supports_local_settings() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE project(Id INTEGER PRIMARY KEY); INSERT INTO project VALUES(1);",
        )
        .unwrap();
        save_settings(
            &mut conn,
            1,
            ProjectProcessingSettings {
                split_exposure_groups: true,
            },
        )
        .unwrap();
        assert!(load_settings(&conn, 1).unwrap().split_exposure_groups);
    }

    #[test]
    fn ambiguous_guids_keep_settings_local() {
        let mut conn = fixture();
        conn.execute("UPDATE project SET guid='project-one' WHERE Id=2", [])
            .unwrap();
        save_settings(
            &mut conn,
            1,
            ProjectProcessingSettings {
                split_exposure_groups: true,
            },
        )
        .unwrap();
        assert!(load_settings(&conn, 1).unwrap().split_exposure_groups);
        assert!(!load_settings(&conn, 2).unwrap().split_exposure_groups);
    }

    #[test]
    fn saved_settings_survive_reopen_and_read_only_access() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.sqlite");
        {
            let mut conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE project(Id INTEGER PRIMARY KEY,guid TEXT); INSERT INTO project VALUES(1,'one');").unwrap();
            save_settings(
                &mut conn,
                1,
                ProjectProcessingSettings {
                    split_exposure_groups: true,
                },
            )
            .unwrap();
        }
        let conn =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert!(load_settings(&conn, 1).unwrap().split_exposure_groups);
        assert!(matches!(load_settings(&conn, 999), Err(AppError::NotFound)));
    }

    #[test]
    fn exposures_accept_recorded_aliases_and_reject_invalid_values() {
        for json in [
            r#"{"ExposureDuration":30}"#,
            r#"{"ExposureTime":"30"}"#,
            r#"{"EXPTIME":30}"#,
            r#"{"ExposureDuration":-1,"ExposureTime":30}"#,
        ] {
            assert_eq!(exposure_seconds_from_metadata(json), Some(30.0));
        }
        for json in [
            "broken",
            "{}",
            r#"{"ExposureDuration":"NaN"}"#,
            r#"{"ExposureDuration":0}"#,
            r#"{"ExposureDuration":"inf"}"#,
        ] {
            assert_eq!(exposure_seconds_from_metadata(json), None);
        }
        assert!(serde_json::from_str::<ProjectProcessingSettings>("{}").is_err());
    }
}
