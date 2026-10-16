//! A person's flat group choices, kept in the catalog beside the
//! measurements ([`super::store`]):
//!
//! - `psf_guard_flat_group_override`: a split or join of one family's
//!   groups, keyed by the first flat of the session the edge comes before;
//! - `psf_guard_flat_light_choice`: a light's side of a change, or the group
//!   it takes, keyed by the light's GUID and the change.
//!
//! Every key is a stable identity (frame UUIDs, response session keys, light
//! GUIDs, change keys and windows), so the choices travel with a sync and a
//! merge of catalogs. A reset keeps its row with no choice and when it was
//! made, as a keep-out choice does, so the newer side wins a round trip and
//! an older choice from a copy never comes back.
//!
//! The tables are added the way the other flat grouping tables are: in a
//! writable catalog, without raising the calibration schema version.

use super::overrides::{
    GroupChoice, GroupOverrideRow, LightChoice, LightChoiceEntry, LightChoiceRow,
};
use anyhow::{Context, Result};
use rusqlite::types::Value;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub(crate) const TABLES_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS psf_guard_flat_group_override (
        anchor_frame        TEXT PRIMARY KEY,
        session_key         TEXT NOT NULL,
        family_key          TEXT NOT NULL,
        rig_uuid            TEXT NOT NULL,
        choice              TEXT,
        before_frame        TEXT,
        before_session_key  TEXT,
        label               TEXT NOT NULL DEFAULT '',
        chosen_at           INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS psf_guard_flat_light_choice (
        light_guid          TEXT NOT NULL,
        change_key          TEXT NOT NULL,
        rig_uuid            TEXT NOT NULL,
        change_start        INTEGER,
        change_end          INTEGER,
        choice              TEXT,
        group_frame         TEXT,
        group_session_key   TEXT,
        label               TEXT NOT NULL DEFAULT '',
        chosen_at           INTEGER NOT NULL,
        PRIMARY KEY (light_guid, change_key)
    );
"#;

const GROUP_TABLE: &str = "psf_guard_flat_group_override";
const LIGHT_TABLE: &str = "psf_guard_flat_light_choice";

const GROUP_COLUMNS: [&str; 9] = [
    "anchor_frame",
    "session_key",
    "family_key",
    "rig_uuid",
    "choice",
    "before_frame",
    "before_session_key",
    "label",
    "chosen_at",
];

const LIGHT_COLUMNS: [&str; 10] = [
    "light_guid",
    "change_key",
    "rig_uuid",
    "change_start",
    "change_end",
    "choice",
    "group_frame",
    "group_session_key",
    "label",
    "chosen_at",
];

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .unwrap_or(false)
}

/// Whether this catalog keeps a person's flat group choices.
pub fn tables_exist(conn: &Connection) -> bool {
    table_exists(conn, GROUP_TABLE) && table_exists(conn, LIGHT_TABLE)
}

/// Create the tables in a writable catalog. Safe to run twice.
pub fn add_tables(conn: &Connection) -> Result<bool> {
    if tables_exist(conn) || conn.is_readonly(rusqlite::MAIN_DB).unwrap_or(true) {
        return Ok(false);
    }
    conn.execute_batch(TABLES_SQL)
        .context("creating the flat group choice tables")?;
    Ok(true)
}

fn require_tables(conn: &Connection) -> Result<()> {
    add_tables(conn)?;
    if !tables_exist(conn) {
        anyhow::bail!(
            "this catalog has not been upgraded to keep flat group choices; open it with \
             write access once and retry"
        );
    }
    Ok(())
}

fn group_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GroupOverrideRow> {
    Ok(GroupOverrideRow {
        anchor_frame: row.get(0)?,
        session_key: row.get(1)?,
        family_key: row.get(2)?,
        rig_uuid: row.get(3)?,
        choice: row
            .get::<_, Option<String>>(4)?
            .as_deref()
            .and_then(GroupChoice::parse),
        before_frame: row.get(5)?,
        before_session_key: row.get(6)?,
        label: row.get(7)?,
        chosen_at: row.get(8)?,
    })
}

fn light_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LightChoiceRow> {
    Ok(LightChoiceRow {
        light_guid: row.get(0)?,
        change_key: row.get(1)?,
        rig_uuid: row.get(2)?,
        change_start: row.get(3)?,
        change_end: row.get(4)?,
        choice: row
            .get::<_, Option<String>>(5)?
            .as_deref()
            .and_then(LightChoice::parse),
        group_frame: row.get(6)?,
        group_session_key: row.get(7)?,
        label: row.get(8)?,
        chosen_at: row.get(9)?,
    })
}

/// Every split and join, resets included.
pub fn load_group_overrides(conn: &Connection) -> Result<Vec<GroupOverrideRow>> {
    if !table_exists(conn, GROUP_TABLE) {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(&format!(
        "SELECT {} FROM {GROUP_TABLE} ORDER BY chosen_at, anchor_frame",
        GROUP_COLUMNS.join(", ")
    ))?;
    let rows = statement
        .query_map([], group_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every light choice in force, with the file name, filter and time of its
/// light, by file name: how a light read from its file finds them.
pub fn load_light_choices(conn: &Connection) -> Result<HashMap<String, Vec<LightChoiceEntry>>> {
    let mut out: HashMap<String, Vec<LightChoiceEntry>> = HashMap::new();
    if !table_exists(conn, LIGHT_TABLE) || !light_guids_exist(conn) {
        return Ok(out);
    }
    let columns: Vec<String> = LIGHT_COLUMNS.iter().map(|c| format!("c.{c}")).collect();
    let mut statement = conn.prepare(&format!(
        "SELECT {}, ai.Id, ai.metadata, ai.filtername, ai.acquireddate
         FROM {LIGHT_TABLE} c JOIN acquiredimage ai ON ai.guid = c.light_guid
         WHERE c.choice IS NOT NULL",
        columns.join(", ")
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let choice = light_row(row)?;
        let metadata: Option<String> = row.get(LIGHT_COLUMNS.len() + 1)?;
        let Some(file_name) = metadata.as_deref().and_then(crate::utils::extract_filename) else {
            continue;
        };
        out.entry(file_name.clone())
            .or_default()
            .push(LightChoiceEntry {
                light_id: row.get(LIGHT_COLUMNS.len())?,
                file_name,
                filter: row.get(LIGHT_COLUMNS.len() + 2)?,
                at: row.get(LIGHT_COLUMNS.len() + 3)?,
                row: choice,
            });
    }
    Ok(out)
}

fn light_guids_exist(conn: &Connection) -> bool {
    conn.prepare("SELECT guid FROM acquiredimage LIMIT 0")
        .is_ok()
}

/// Every choice recorded for these lights, resets included, by GUID.
pub fn light_choices_of(conn: &Connection, guids: &[String]) -> Result<Vec<LightChoiceRow>> {
    if !table_exists(conn, LIGHT_TABLE) {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(&format!(
        "SELECT {} FROM {LIGHT_TABLE} WHERE light_guid = ?1 ORDER BY chosen_at",
        LIGHT_COLUMNS.join(", ")
    ))?;
    let mut out = Vec::new();
    for guid in guids {
        out.extend(
            statement
                .query_map([guid], light_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(out)
}

/// Record a split or join, or its reset (`choice: None`).
pub fn set_group_override(conn: &Connection, row: &GroupOverrideRow) -> Result<()> {
    require_tables(conn)?;
    conn.execute(
        &format!(
            "INSERT INTO {GROUP_TABLE} ({}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(anchor_frame) DO UPDATE SET
                session_key = excluded.session_key, family_key = excluded.family_key,
                rig_uuid = excluded.rig_uuid, choice = excluded.choice,
                before_frame = excluded.before_frame,
                before_session_key = excluded.before_session_key,
                label = excluded.label, chosen_at = excluded.chosen_at",
            GROUP_COLUMNS.join(", ")
        ),
        params![
            row.anchor_frame,
            row.session_key,
            row.family_key,
            row.rig_uuid,
            row.choice.map(GroupChoice::as_str),
            row.before_frame,
            row.before_session_key,
            row.label,
            row.chosen_at,
        ],
    )?;
    Ok(())
}

/// Record light choices, or their resets (`choice: None`).
pub fn set_light_choices(conn: &Connection, rows: &[LightChoiceRow]) -> Result<usize> {
    require_tables(conn)?;
    let mut statement = conn.prepare(&format!(
        "INSERT INTO {LIGHT_TABLE} ({}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(light_guid, change_key) DO UPDATE SET
            rig_uuid = excluded.rig_uuid, change_start = excluded.change_start,
            change_end = excluded.change_end, choice = excluded.choice,
            group_frame = excluded.group_frame, group_session_key = excluded.group_session_key,
            label = excluded.label, chosen_at = excluded.chosen_at",
        LIGHT_COLUMNS.join(", ")
    ))?;
    let mut changed = 0;
    for row in rows {
        changed += statement.execute(params![
            row.light_guid,
            row.change_key,
            row.rig_uuid,
            row.change_start,
            row.change_end,
            row.choice.map(LightChoice::as_str),
            row.group_frame,
            row.group_session_key,
            row.label,
            row.chosen_at,
        ])?;
    }
    Ok(changed)
}

/// Fold both tables into a digest, so a book, a health pass and a view
/// built from them are made again when a choice changes.
pub fn digest(conn: &Connection, hasher: &mut Sha256) -> Result<()> {
    for (table, columns, order) in [
        (GROUP_TABLE, &GROUP_COLUMNS[..], "anchor_frame"),
        (LIGHT_TABLE, &LIGHT_COLUMNS[..], "light_guid, change_key"),
    ] {
        if !table_exists(conn, table) {
            continue;
        }
        let mut statement = conn.prepare(&format!(
            "SELECT {} FROM {table} ORDER BY {order}",
            columns.join(", ")
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            for index in 0..columns.len() {
                match row.get::<_, Value>(index)? {
                    Value::Null => hasher.update(b"\x00"),
                    Value::Integer(v) => hasher.update(v.to_le_bytes()),
                    Value::Real(v) => hasher.update(v.to_le_bytes()),
                    Value::Text(v) => hasher.update(v.as_bytes()),
                    Value::Blob(v) => hasher.update(&v),
                }
                hasher.update(b"\x1f");
            }
            hasher.update(b"\x1e");
        }
        hasher.update(b"\x1d");
    }
    Ok(())
}

/// How many flat group choices a sync set or reset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChoiceSync {
    pub groups: usize,
    pub lights: usize,
}

/// Copy a person's flat group choices to another catalog: each row by its
/// stable key, the newer choice winning, a reset included. A split or join
/// comes across only when the destination holds its flat, a light choice
/// only when it holds the light: a choice about nothing there would only
/// show as orphaned.
pub fn sync(source: &Connection, destination: &rusqlite::Transaction<'_>) -> Result<ChoiceSync> {
    let mut counts = ChoiceSync::default();
    if !table_exists(source, GROUP_TABLE) && !table_exists(source, LIGHT_TABLE) {
        return Ok(counts);
    }
    destination
        .execute_batch(TABLES_SQL)
        .context("creating the flat group choice tables")?;
    if table_exists(source, GROUP_TABLE) {
        counts.groups = sync_table(
            source,
            destination,
            GROUP_TABLE,
            &GROUP_COLUMNS,
            &["anchor_frame"],
            "SELECT 1 FROM psf_guard_calibration_frame WHERE frame_uuid = ?1",
        )?;
    }
    if table_exists(source, LIGHT_TABLE) && light_guids_exist(destination) {
        counts.lights = sync_table(
            source,
            destination,
            LIGHT_TABLE,
            &LIGHT_COLUMNS,
            &["light_guid", "change_key"],
            "SELECT 1 FROM acquiredimage WHERE guid = ?1",
        )?;
    }
    Ok(counts)
}

/// One table of [`sync`]: rows keyed by `keys` (the first of which
/// `known` must find in the destination), the newer `chosen_at` winning.
fn sync_table(
    source: &Connection,
    destination: &rusqlite::Transaction<'_>,
    table: &str,
    columns: &[&str],
    keys: &[&str],
    known: &str,
) -> Result<usize> {
    let select = format!("SELECT {} FROM {table}", columns.join(", "));
    let at = columns
        .iter()
        .position(|column| *column == "chosen_at")
        .expect("chosen_at");
    let key_positions: Vec<usize> = keys
        .iter()
        .map(|key| {
            columns
                .iter()
                .position(|column| column == key)
                .expect("key")
        })
        .collect();
    let matches = keys
        .iter()
        .enumerate()
        .map(|(index, key)| format!("{key} = ?{}", index + 1))
        .collect::<Vec<_>>()
        .join(" AND ");
    let placeholders = (1..=columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = source.prepare(&select)?;
    let mut rows = statement.query([])?;
    let mut changed = 0;
    while let Some(row) = rows.next()? {
        let values = (0..columns.len())
            .map(|index| row.get::<_, Value>(index))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let key_values: Vec<&Value> = key_positions.iter().map(|p| &values[*p]).collect();
        let held = destination
            .query_row(known, [key_values[0]], |_| Ok(()))
            .optional()?
            .is_some();
        if !held {
            continue;
        }
        let existing: Option<i64> = destination
            .query_row(
                &format!("SELECT chosen_at FROM {table} WHERE {matches}"),
                rusqlite::params_from_iter(key_values.iter()),
                |row| row.get(0),
            )
            .optional()?;
        let source_at = match values[at] {
            Value::Integer(value) => value,
            _ => continue,
        };
        if existing.is_some_and(|existing| existing >= source_at) {
            continue;
        }
        destination.execute(
            &format!(
                "INSERT OR REPLACE INTO {table} ({}) VALUES ({placeholders})",
                columns.join(", ")
            ),
            rusqlite::params_from_iter(values.iter()),
        )?;
        changed += 1;
    }
    Ok(changed)
}
