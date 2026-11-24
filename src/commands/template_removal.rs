//! Exposure template removal. A person deletes a Target Scheduler exposure
//! template that no exposure plan uses. A record of the deletion keeps a
//! later sync from bringing the template back, and a sync carries it to
//! other catalogs and rigs, which delete their copy when no plan there uses
//! it. See docs/design/data-transfer.md#deleted-templates.

use crate::commands::sync::table_has_column;
use crate::renamed_lights::table_exists;
use anyhow::{Context, Result};
use rusqlite::types::Value;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Templates this catalog deleted, one row for each GUID.
pub const TABLE: &str = "psf_guard_removed_template";

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS psf_guard_removed_template (
    id INTEGER PRIMARY KEY,
    template_guid TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    profile_id TEXT,
    removed_at INTEGER NOT NULL,
    origin TEXT NOT NULL DEFAULT 'local',
    settings_key TEXT
)";

/// Create the table. Safe to call repeatedly.
pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)
        .context("creating the removed template table")
}

/// A GUID other catalogs can match: a UUID, trimmed and in lower case.
pub(crate) fn sync_guid(guid: Option<&str>) -> Option<String> {
    let guid = guid?.trim();
    uuid::Uuid::parse_str(guid)
        .ok()
        .map(|_| guid.to_ascii_lowercase())
}

/// Deletion records that still stand: a template with the GUID is not
/// back in the catalog. Activation can make a template again with a
/// library template's GUID, and that template must not count as deleted.
pub(crate) const STANDING: &str = "lower(trim(template_guid)) NOT IN
    (SELECT lower(trim(guid)) FROM exposuretemplate WHERE guid IS NOT NULL)";

/// The columns of the database's `exposuretemplate` table.
pub(crate) fn template_columns(conn: &Connection) -> Result<Vec<String>> {
    let mut statement = conn.prepare("PRAGMA table_info(exposuretemplate)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(columns)
}

/// `SELECT` list for `columns`, quoted.
pub(crate) fn select_list(columns: &[String]) -> String {
    columns
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The settings that identify a template whatever its GUID
/// ([`crate::template_repeats::identical_template_key`]) over `columns`.
pub(crate) fn settings_key(columns: &[String], values: &[Value]) -> String {
    let columns: Vec<&str> = columns.iter().map(String::as_str).collect();
    crate::template_repeats::identical_template_key(&columns, values)
}

fn settings_key_of(conn: &Connection, id: i64) -> Result<Option<String>> {
    let columns = template_columns(conn)?;
    let values = conn
        .query_row(
            &format!(
                "SELECT {} FROM exposuretemplate WHERE Id = ?1",
                select_list(&columns)
            ),
            [id],
            |row| {
                (0..columns.len())
                    .map(|index| row.get::<_, Value>(index))
                    .collect::<rusqlite::Result<Vec<_>>>()
            },
        )
        .optional()?;
    Ok(values.map(|values| settings_key(&columns, &values)))
}

/// The templates this catalog deleted and does not have again.
#[derive(Debug, Default)]
pub(crate) struct Removed {
    /// Their GUIDs, lower case.
    pub guids: HashSet<String>,
    /// Their settings keys, to the record's GUID, for deletions no live
    /// template repeats. Two copies of one Target Scheduler database can
    /// give one template different GUIDs; this finds the other copy's.
    pub keys: HashMap<String, String>,
}

pub(crate) fn removed_templates(conn: &Connection) -> Result<Removed> {
    if !table_exists(conn, TABLE) {
        return Ok(Removed::default());
    }
    let mut statement = conn.prepare(&format!(
        "SELECT lower(template_guid), settings_key FROM {TABLE} WHERE {STANDING}"
    ))?;
    let mut removed = Removed::default();
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })? {
        let (guid, key) = row?;
        if let Some(key) = key {
            removed.keys.insert(key, guid.clone());
        }
        removed.guids.insert(guid);
    }
    if !removed.keys.is_empty() {
        let columns = template_columns(conn)?;
        let mut live = conn.prepare(&format!(
            "SELECT {} FROM exposuretemplate",
            select_list(&columns)
        ))?;
        let mut rows = live.query([])?;
        while let Some(row) = rows.next()? {
            let values = (0..columns.len())
                .map(|index| row.get::<_, Value>(index))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            removed.keys.remove(&settings_key(&columns, &values));
        }
    }
    Ok(removed)
}

/// GUIDs of the templates this catalog deleted and does not have again,
/// lower case. Empty when none was ever deleted.
pub fn removed_template_guids(conn: &Connection) -> Result<HashSet<String>> {
    Ok(removed_templates(conn)?.guids)
}

fn record(
    tx: &Transaction,
    guid: &str,
    name: &str,
    profile_id: Option<&str>,
    removed_at: i64,
    origin: &str,
    settings_key: Option<&str>,
) -> Result<()> {
    ensure_schema(tx)?;
    tx.execute(
        &format!(
            "INSERT INTO {TABLE} (template_guid, name, profile_id, removed_at, origin, settings_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(template_guid) DO UPDATE SET name = excluded.name,
                 profile_id = excluded.profile_id,
                 removed_at = max(removed_at, excluded.removed_at),
                 origin = excluded.origin,
                 settings_key = excluded.settings_key"
        ),
        params![guid, name, profile_id, removed_at, origin, settings_key],
    )?;
    Ok(())
}

/// Move a deletion record to the GUID another copy of the template has, so
/// it reaches that copy too. Nothing changes when a record already has it.
pub(crate) fn adopt(tx: &Transaction, guid: &str, other: &str) -> Result<()> {
    tx.execute(
        &format!(
            "UPDATE {TABLE} SET template_guid = ?2 WHERE lower(template_guid) = ?1
             AND NOT EXISTS (SELECT 1 FROM {TABLE} WHERE lower(template_guid) = ?2)"
        ),
        [guid, other],
    )?;
    Ok(())
}

/// Drop the deletion record of a template that came back.
pub(crate) fn forget(tx: &Transaction, guid: &str) -> Result<()> {
    if table_exists(tx, TABLE) {
        tx.execute(
            &format!("DELETE FROM {TABLE} WHERE lower(template_guid) = ?1"),
            [guid],
        )?;
    }
    Ok(())
}

/// A template [`remove`] deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedTemplate {
    pub id: i64,
    pub name: String,
    /// Whether syncs carry the deletion: only a template with a UUID GUID
    /// can be matched in another catalog.
    pub synced: bool,
}

/// Why [`remove`] deleted nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No template has this Id.
    Missing,
    /// Exposure plans use the template.
    InUse { name: String, plans: i64 },
}

/// Delete the template with this Id when no exposure plan uses it, and
/// record the deletion when it has a GUID.
pub fn remove(conn: &mut Connection, id: i64) -> Result<Result<RemovedTemplate, Refusal>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some((name, guid, profile_id)) = tx
        .query_row(
            "SELECT name, guid, profileId FROM exposuretemplate WHERE Id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(Err(Refusal::Missing));
    };
    let plans: i64 = tx.query_row(
        "SELECT COUNT(*) FROM exposureplan WHERE exposureTemplateId = ?1",
        [id],
        |row| row.get(0),
    )?;
    if plans > 0 {
        return Ok(Err(Refusal::InUse { name, plans }));
    }
    let key = settings_key_of(&tx, id)?;
    tx.execute("DELETE FROM exposuretemplate WHERE Id = ?1", [id])?;
    let guid = sync_guid(guid.as_deref());
    if let Some(guid) = &guid {
        record(
            &tx,
            guid,
            &name,
            profile_id.as_deref(),
            crate::commands::project_removal::unix_now(),
            "local",
            key.as_deref(),
        )?;
    }
    tx.commit()?;
    Ok(Ok(RemovedTemplate {
        id,
        name,
        synced: guid.is_some(),
    }))
}

/// A deletion recorded in a sync's source catalog.
#[derive(Debug, Clone)]
struct SourceDeletion {
    name: String,
    removed_at: i64,
}

/// The deletions a sync's source catalog recorded, by GUID. A bundle may
/// carry only some columns, and a source without the table deleted none.
/// A deletion without a date is dated `now`.
fn source_deletions(src: &Connection, now: i64) -> Result<BTreeMap<String, SourceDeletion>> {
    if !table_exists(src, TABLE) || !table_has_column(src, TABLE, "template_guid") {
        return Ok(BTreeMap::new());
    }
    let column = |name: &'static str| {
        if table_has_column(src, TABLE, name) {
            name
        } else {
            "NULL"
        }
    };
    let mut statement = src.prepare(&format!(
        "SELECT template_guid, {}, {} FROM {TABLE}",
        column("name"),
        column("removed_at"),
    ))?;
    let mut deletions = BTreeMap::<String, SourceDeletion>::new();
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<i64>>(2)?,
        ))
    })? {
        let (guid, name, removed_at) = row?;
        let Some(guid) = sync_guid(guid.as_deref()) else {
            continue;
        };
        let removed_at = removed_at.unwrap_or(now);
        let entry = deletions.entry(guid).or_insert(SourceDeletion {
            name: name.clone().unwrap_or_default(),
            removed_at,
        });
        entry.removed_at = entry.removed_at.max(removed_at);
    }
    Ok(deletions)
}

fn plans(count: i64) -> String {
    if count == 1 {
        "1 exposure plan here uses it".into()
    } else {
        format!("{count} exposure plans here use it")
    }
}

/// Delete the templates the source catalog deleted, when no exposure plan
/// in the destination uses them. Run after the plans are written, so a
/// plan the source moved to another template counts against that one.
///
/// A template the source has again is left alone. `record` writes the
/// deletion into the destination (a PSF Guard catalog), so a later sync
/// from a rig does not bring the template back; a rig's Target Scheduler
/// database gets no PSF Guard table. `previewed`, when set, lists the only
/// GUIDs this run may delete. Returns the GUIDs deleted.
pub(crate) fn apply_source_deletions(
    src: &Connection,
    tx: &Transaction,
    record_deletions: bool,
    previewed: Option<&HashSet<String>>,
    changes: &mut Vec<String>,
) -> Result<Vec<String>> {
    let now = crate::commands::project_removal::unix_now();
    let deletions = source_deletions(src, now)?;
    if deletions.is_empty() {
        return Ok(Vec::new());
    }
    let mut live_statement = src.prepare("SELECT guid FROM exposuretemplate")?;
    let live: HashSet<String> = live_statement
        .query_map([], |row| row.get::<_, Option<String>>(0))?
        .filter_map(|guid| guid.map(|guid| sync_guid(guid.as_deref())).transpose())
        .collect::<rusqlite::Result<_>>()?;
    let mut find = tx
        .prepare("SELECT Id, name, profileId FROM exposuretemplate WHERE lower(trim(guid)) = ?1")?;
    let mut deleted = Vec::new();
    for (guid, deletion) in deletions {
        if live.contains(&guid) {
            continue;
        }
        let found = find
            .query_map([&guid], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let (id, name, profile_id) = match found.as_slice() {
            [] => continue,
            [(id, name, profile_id)] => (
                *id,
                name.clone().unwrap_or_else(|| deletion.name.clone()),
                profile_id.clone(),
            ),
            _ => {
                changes.push(format!(
                    "skip exposuretemplate \"{}\": deleted in the source catalog, but {} templates here have its GUID",
                    deletion.name,
                    found.len()
                ));
                continue;
            }
        };
        let keep = |reason: String| {
            format!("keep exposuretemplate \"{name}\": deleted in the source catalog, but {reason}")
        };
        let used: i64 = tx.query_row(
            "SELECT COUNT(*) FROM exposureplan WHERE exposureTemplateId = ?1",
            [id],
            |row| row.get(0),
        )?;
        if used > 0 {
            changes.push(keep(plans(used)));
            continue;
        }
        if previewed.is_some_and(|previewed| !previewed.contains(&guid)) {
            changes.push(keep("it was not in the preview; preview again".into()));
            continue;
        }
        let key = settings_key_of(tx, id)?;
        tx.execute("DELETE FROM exposuretemplate WHERE Id = ?1", [id])?;
        if record_deletions {
            // Dated when the person deleted it, on that catalog's clock.
            record(
                tx,
                &guid,
                &name,
                profile_id.as_deref(),
                deletion.removed_at,
                "sync",
                key.as_deref(),
            )?;
        }
        changes.push(format!(
            "delete exposuretemplate \"{name}\": deleted in the source catalog"
        ));
        deleted.push(guid);
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: &str = "6F1C1B9E-9A7D-4C55-8E57-0B5C3E1D2A10";

    fn catalog() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE exposuretemplate (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, guid TEXT);
             CREATE TABLE exposureplan (Id INTEGER PRIMARY KEY, exposureTemplateId INTEGER, guid TEXT);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn removes_an_unused_template_and_records_its_guid() {
        let mut conn = catalog();
        conn.execute(
            "INSERT INTO exposuretemplate VALUES (1, 'p', 'Ha 300', ?1)",
            [GUID],
        )
        .unwrap();
        let removed = remove(&mut conn, 1).unwrap().unwrap();
        assert_eq!(
            removed,
            RemovedTemplate {
                id: 1,
                name: "Ha 300".into(),
                synced: true
            }
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM exposuretemplate", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            removed_template_guids(&conn).unwrap(),
            HashSet::from([GUID.to_ascii_lowercase()])
        );
    }

    #[test]
    fn refuses_a_template_a_plan_uses_and_an_unknown_id() {
        let mut conn = catalog();
        conn.execute_batch(
            "INSERT INTO exposuretemplate VALUES (1, 'p', 'Ha 300', NULL);
             INSERT INTO exposureplan VALUES (1, 1, NULL), (2, 1, NULL);",
        )
        .unwrap();
        assert_eq!(
            remove(&mut conn, 1).unwrap(),
            Err(Refusal::InUse {
                name: "Ha 300".into(),
                plans: 2
            })
        );
        assert_eq!(remove(&mut conn, 9).unwrap(), Err(Refusal::Missing));
    }

    const L: &str = "11111111-1111-4111-8111-111111111111";
    const HA: &str = "22222222-2222-4222-8222-222222222222";

    /// A Target Scheduler database with one project, target and plan. The
    /// plan uses L; Ha is unused.
    fn scheduler() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch(&format!(
            r#"INSERT INTO project (Id, profileId, name, state, guid) VALUES (1, 'p', 'M31', 1, 'pg-m31');
               INSERT INTO target (Id, name, active, epochcode, projectid, guid) VALUES (10, 'M31', 1, 0, 1, 'tg-m31');
               INSERT INTO exposuretemplate (Id, profileId, name, filtername, guid) VALUES
                 (1, 'p', 'L', 'L', '{L}'), (2, 'p', 'Ha', 'Ha', '{HA}');
               INSERT INTO exposureplan (Id, profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, guid)
                 VALUES (100, 'p', 300, 10, 0, 0, 10, 1, 'eg-l');"#
        ))
        .unwrap();
        conn
    }

    fn templates(conn: &Connection) -> Vec<String> {
        let mut statement = conn
            .prepare("SELECT name FROM exposuretemplate ORDER BY name")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn pull(dry_run: bool) -> crate::commands::sync::PullOptions {
        crate::commands::sync::PullOptions {
            dry_run,
            with_image_data: false,
            project_filter: None,
            deletions: Default::default(),
        }
    }

    fn has(changes: &[String], line: &str) -> bool {
        changes.iter().any(|change| change == line)
    }

    /// The rig still has a template deleted here: a pull leaves it out
    /// until a plan from the rig uses it, and then it comes back for good.
    #[test]
    fn a_pull_leaves_out_a_template_deleted_here_until_a_plan_needs_it() {
        use crate::commands::sync::sync_pull;
        let mut local = scheduler();
        remove(&mut local, 2).unwrap().unwrap();
        let rig = scheduler();

        let summary = sync_pull(&rig, &local, &pull(false)).unwrap();
        assert_eq!(templates(&local), ["L"]);
        assert!(
            has(
                &summary.changes,
                "leave out exposuretemplate \"Ha\": deleted in this catalog"
            ),
            "{:?}",
            summary.changes
        );

        rig.execute(
            "INSERT INTO exposureplan (Id, profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, guid)
             VALUES (101, 'p', 300, 10, 0, 0, 10, 2, 'eg-ha')",
            [],
        )
        .unwrap();
        let summary = sync_pull(&rig, &local, &pull(false)).unwrap();
        assert_eq!(templates(&local), ["Ha", "L"]);
        assert!(has(
            &summary.changes,
            "bring back exposuretemplate \"Ha\": deleted in this catalog, but a plan from the source uses it"
        ));
        assert_eq!(
            local
                .query_row(
                    "SELECT t.name FROM exposureplan p JOIN exposuretemplate t ON t.Id = p.exposureTemplateId WHERE p.guid = 'eg-ha'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "Ha"
        );
        assert!(removed_template_guids(&local).unwrap().is_empty());
    }

    /// The automatic capture import leaves a deleted template out without
    /// a change line for it on every run.
    #[test]
    fn the_capture_import_leaves_a_deleted_template_out_quietly() {
        let mut local = scheduler();
        remove(&mut local, 2).unwrap().unwrap();
        let rig = scheduler();
        let tx = local.transaction().unwrap();
        let summary =
            crate::commands::sync::sync_arrivals(&rig, &tx, &HashSet::new(), &Default::default())
                .unwrap();
        tx.commit().unwrap();
        assert_eq!(templates(&local), ["L"]);
        assert!(
            !summary.changes.iter().any(|change| change.contains("Ha")),
            "{:?}",
            summary.changes
        );
    }

    /// A pull from a catalog that deleted a template deletes the copy here
    /// when no plan here uses it, and records the deletion with the
    /// source's date, so the rig does not bring it back.
    #[test]
    fn a_pull_deletes_a_template_the_source_catalog_deleted() {
        use crate::commands::sync::sync_pull;
        let mut source = scheduler();
        remove(&mut source, 2).unwrap().unwrap();
        source
            .execute(&format!("UPDATE {TABLE} SET removed_at = 1700000000"), [])
            .unwrap();

        let local = scheduler();
        let preview = sync_pull(&source, &local, &pull(true)).unwrap();
        assert_eq!(preview.exposuretemplate.deleted, 1);
        assert_eq!(preview.deleted_template_guids, [HA]);
        assert_eq!(templates(&local), ["Ha", "L"], "a dry run deletes nothing");

        let summary = sync_pull(&source, &local, &pull(false)).unwrap();
        assert_eq!(templates(&local), ["L"]);
        assert!(has(
            &summary.changes,
            "delete exposuretemplate \"Ha\": deleted in the source catalog"
        ));
        assert_eq!(
            local
                .query_row(
                    &format!("SELECT removed_at, origin FROM {TABLE}"),
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .unwrap(),
            (1_700_000_000, "sync".to_string())
        );
        let rig = scheduler();
        sync_pull(&rig, &local, &pull(false)).unwrap();
        assert_eq!(templates(&local), ["L"], "the rig does not bring it back");

        // A plan here keeps the template.
        let local = scheduler();
        local
            .execute(
                "INSERT INTO exposureplan (Id, profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, guid)
                 VALUES (101, 'p', 300, 10, 0, 0, 10, 2, 'eg-local')",
                [],
            )
            .unwrap();
        let summary = sync_pull(&source, &local, &pull(false)).unwrap();
        assert_eq!(templates(&local), ["Ha", "L"]);
        assert!(has(
            &summary.changes,
            "keep exposuretemplate \"Ha\": deleted in the source catalog, but 1 exposure plan here uses it"
        ));
    }

    /// A source that has the template again does not mean to delete it,
    /// and an apply deletes only what its preview listed.
    #[test]
    fn a_live_template_or_one_missing_from_the_preview_stays() {
        use crate::commands::sync::sync_pull;
        let mut source = scheduler();
        remove(&mut source, 2).unwrap().unwrap();
        let local = scheduler();
        let mut options = pull(false);
        options.deletions.previewed = Some(HashSet::new());
        let summary = sync_pull(&source, &local, &options).unwrap();
        assert_eq!(templates(&local), ["Ha", "L"]);
        assert!(has(
            &summary.changes,
            "keep exposuretemplate \"Ha\": deleted in the source catalog, but it was not in the preview; preview again"
        ));

        source
            .execute(
                &format!("INSERT INTO exposuretemplate (Id, profileId, name, filtername, guid) VALUES (2, 'p', 'Ha', 'Ha', '{HA}')"),
                [],
            )
            .unwrap();
        let summary = sync_pull(&source, &local, &pull(false)).unwrap();
        assert_eq!(templates(&local), ["Ha", "L"]);
        assert_eq!(summary.exposuretemplate.deleted, 0);
    }

    /// A planning push into a rig's Target Scheduler database deletes the
    /// template there and writes no PSF Guard table.
    #[test]
    fn a_planning_push_deletes_the_template_on_the_rig_without_a_record() {
        use crate::commands::sync::{sync_planning, PlanningOptions, SyncDestination};
        let mut source = scheduler();
        remove(&mut source, 2).unwrap().unwrap();
        let rig = scheduler();
        let summary = sync_planning(
            &source,
            &rig,
            &PlanningOptions {
                dry_run: false,
                project_filter: None,
                destination: SyncDestination::Scheduler,
                deletions: Default::default(),
            },
        )
        .unwrap();
        assert_eq!(summary.exposuretemplate.deleted, 1);
        assert_eq!(templates(&rig), ["L"]);
        assert!(!table_exists(&rig, TABLE));
    }

    /// The rig's copy of a template can have another GUID (two copies of
    /// one Target Scheduler database fill GUIDs in apart). A pull finds it
    /// by its settings and leaves it out, and the record takes its GUID so
    /// the next export deletes it on the rig.
    #[test]
    fn a_copy_under_another_guid_stays_out_and_the_record_takes_its_guid() {
        use crate::commands::sync::sync_pull;
        const RIG_HA: &str = "55555555-5555-4555-8555-555555555555";
        let mut local = scheduler();
        remove(&mut local, 2).unwrap().unwrap();
        let rig = scheduler();
        rig.execute(
            &format!("UPDATE exposuretemplate SET guid = '{RIG_HA}' WHERE Id = 2"),
            [],
        )
        .unwrap();
        let summary = sync_pull(&rig, &local, &pull(false)).unwrap();
        assert_eq!(templates(&local), ["L"], "{:?}", summary.changes);
        assert_eq!(
            removed_template_guids(&local).unwrap(),
            HashSet::from([RIG_HA.to_string()])
        );
        // A template with other settings and its own GUID is another
        // template, and arrives.
        rig.execute(
            "INSERT INTO exposuretemplate (Id, profileId, name, filtername, gain, guid)
             VALUES (3, 'p', 'Ha', 'Ha', 120, '66666666-6666-4666-8666-666666666666')",
            [],
        )
        .unwrap();
        sync_pull(&rig, &local, &pull(false)).unwrap();
        assert_eq!(templates(&local), ["Ha", "L"]);
    }

    /// Templates belong to no project: a sync limited to some projects
    /// leaves the source's template deletions for a full one.
    #[test]
    fn a_sync_limited_to_some_projects_deletes_no_template() {
        use crate::commands::sync::sync_pull;
        let mut source = scheduler();
        remove(&mut source, 2).unwrap().unwrap();
        let local = scheduler();
        let mut options = pull(false);
        options.project_filter = Some("M31".into());
        let summary = sync_pull(&source, &local, &options).unwrap();
        assert_eq!(summary.exposuretemplate.deleted, 0);
        assert_eq!(templates(&local), ["Ha", "L"]);
    }

    /// A deletion record sent without a date is dated when it arrives, not
    /// 1970.
    #[test]
    fn a_record_without_a_date_is_dated_on_arrival() {
        use crate::commands::sync::sync_pull;
        let source = scheduler();
        source
            .execute_batch(&format!(
                "DELETE FROM exposuretemplate WHERE Id = 2;
                 CREATE TABLE {TABLE} (template_guid TEXT, name TEXT);
                 INSERT INTO {TABLE} VALUES ('{HA}', 'Ha');"
            ))
            .unwrap();
        let local = scheduler();
        sync_pull(&source, &local, &pull(false)).unwrap();
        let removed_at: i64 = local
            .query_row(&format!("SELECT removed_at FROM {TABLE}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(removed_at > 1_700_000_000, "{removed_at}");
    }

    /// Activation can make a template again with the deleted one's GUID
    /// (a library template's). The record then no longer counts.
    #[test]
    fn a_template_made_again_with_the_guid_is_not_deleted() {
        let mut conn = scheduler();
        remove(&mut conn, 2).unwrap().unwrap();
        assert_eq!(removed_template_guids(&conn).unwrap().len(), 1);
        conn.execute(
            &format!("INSERT INTO exposuretemplate (Id, profileId, name, filtername, guid) VALUES (3, 'p', 'Ha', 'Ha', '{}')", HA.to_ascii_uppercase()),
            [],
        )
        .unwrap();
        assert!(removed_template_guids(&conn).unwrap().is_empty());
    }

    #[test]
    fn a_template_without_a_guid_is_deleted_without_a_record() {
        let mut conn = catalog();
        conn.execute(
            "INSERT INTO exposuretemplate VALUES (1, 'p', 'Old', '')",
            [],
        )
        .unwrap();
        assert!(!remove(&mut conn, 1).unwrap().unwrap().synced);
        assert!(removed_template_guids(&conn).unwrap().is_empty());
    }
}
