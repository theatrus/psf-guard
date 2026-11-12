//! Exposure templates a database repeats (#1098).
//!
//! Target Scheduler lists every template a N.I.N.A. profile has. Older
//! syncs matched a template only by GUID, so a rig gained a second "Ha"
//! each time two copies of one database had given it different GUIDs.
//!
//! Two templates repeat each other when they are in the same profile, have
//! the same name and filter (without case or surrounding spaces) and every
//! other setting is equal, camera defaults written either way (`NULL` or
//! below zero). Target Scheduler shoots a plan on either one the same way,
//! so [`merge_repeated`] moves each copy's plans to one template and deletes
//! the copy. It keeps the template most plans use, then the oldest.
//! PSF Guard runs it whenever it writes planning to a database.
//!
//! Only `exposureplan.exposureTemplateId` refers to a template, in Target
//! Scheduler's schema and in PSF Guard's tables alike.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{params, Connection};

/// One template that repeats another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedTemplate {
    /// The copy deleted.
    pub removed_id: i64,
    /// The template its plans moved to.
    pub kept_id: i64,
    pub name: String,
    /// Plans moved.
    pub plans: usize,
}

struct Row {
    id: i64,
    name: String,
    plans: u64,
    /// Every column but `Id` and `guid`, folded as the module explains.
    key: String,
}

fn integer(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(value) => Some(*value),
        Value::Real(value) if value.fract() == 0.0 => Some(*value as i64),
        _ => None,
    }
}

fn has_table(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get(0),
    )?)
}

fn read_rows(conn: &Connection) -> Result<Vec<Row>> {
    if !has_table(conn, "exposuretemplate")? {
        return Ok(Vec::new());
    }
    let columns = {
        let mut statement = conn.prepare("PRAGMA table_info(exposuretemplate)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let id_column = columns.iter().position(|c| c.eq_ignore_ascii_case("Id"));
    let Some(id_column) = id_column else {
        return Ok(Vec::new());
    };
    let mut plans: HashMap<i64, u64> = HashMap::new();
    if has_table(conn, "exposureplan")? {
        let mut statement = conn.prepare(
            "SELECT exposureTemplateId, COUNT(*) FROM exposureplan
             WHERE exposureTemplateId IS NOT NULL GROUP BY exposureTemplateId",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            if let Some(id) = integer(&row.get::<_, Value>(0)?) {
                *plans.entry(id).or_default() += row.get::<_, i64>(1)? as u64;
            }
        }
    }
    let quoted = columns
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(",");
    let mut statement = conn.prepare(&format!(
        "SELECT {quoted} FROM exposuretemplate ORDER BY Id"
    ))?;
    let mut rows = statement.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let values = (0..columns.len())
            .map(|i| row.get::<_, Value>(i))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let Some(id) = integer(&values[id_column]) else {
            continue;
        };
        let mut name = String::new();
        let mut key = Vec::new();
        for (column, value) in columns.iter().zip(&values) {
            let folded = match column.to_ascii_lowercase().as_str() {
                "id" | "guid" => continue,
                "profileid" | "name" | "filtername" => {
                    let text = match value {
                        Value::Text(text) => text.clone(),
                        _ => String::new(),
                    };
                    if column.eq_ignore_ascii_case("name") {
                        name = text.clone();
                    }
                    Value::Text(text.trim().to_lowercase())
                }
                "gain" | "offset" | "readoutmode" => {
                    Value::Integer(integer(value).filter(|v| *v >= 0).unwrap_or(-1))
                }
                "bin" => Value::Integer(integer(value).filter(|v| *v >= 1).unwrap_or(1)),
                _ => value.clone(),
            };
            key.push(format!("{column}={folded:?}"));
        }
        out.push(Row {
            id,
            name,
            plans: plans.get(&id).copied().unwrap_or(0),
            key: key.join("\u{0}"),
        });
    }
    Ok(out)
}

/// Each group of repeated templates, the one kept first.
fn repeated_groups(conn: &Connection) -> Result<Vec<Vec<Row>>> {
    let mut groups: Vec<Vec<Row>> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for row in read_rows(conn)? {
        match by_key.get(&row.key) {
            Some(&group) => groups[group].push(row),
            None => {
                by_key.insert(row.key.clone(), groups.len());
                groups.push(vec![row]);
            }
        }
    }
    groups.retain(|group| group.len() > 1);
    for group in &mut groups {
        group.sort_by_key(|row| (std::cmp::Reverse(row.plans), row.id));
    }
    Ok(groups)
}

/// Move each repeated template's plans to the one kept and delete it. Runs
/// in the caller's transaction, so a dry run rolls it back with the rest.
pub fn merge_repeated(conn: &Connection) -> Result<Vec<MergedTemplate>> {
    let mut merged = Vec::new();
    for group in repeated_groups(conn)? {
        let kept = &group[0];
        for copy in &group[1..] {
            let plans = conn.execute(
                "UPDATE exposureplan SET exposureTemplateId = ?1 WHERE exposureTemplateId = ?2",
                params![kept.id, copy.id],
            )?;
            conn.execute(
                "DELETE FROM exposuretemplate WHERE Id = ?1",
                params![copy.id],
            )?;
            merged.push(MergedTemplate {
                removed_id: copy.id,
                kept_id: kept.id,
                name: copy.name.clone(),
                plans,
            });
        }
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE exposuretemplate (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, filtername TEXT,
                 gain INTEGER, offset INTEGER, bin INTEGER, readoutmode INTEGER, defaultexposure REAL,
                 moonavoidanceenabled INTEGER, guid TEXT);
             CREATE TABLE exposureplan (Id INTEGER PRIMARY KEY, exposure REAL, exposureTemplateId INTEGER, guid TEXT);
             -- The rig's own Ha Cyg, and two copies a sync added under new GUIDs.
             INSERT INTO exposuretemplate VALUES
                 (1, 'p1', 'Ha Cyg', 'Ha', 125, 30, 1, -1, 300, 1, 'a'),
                 (2, 'p1', 'Ha Cyg ', 'HA', 125, 30, 1, NULL, 300, 1, 'b'),
                 (3, 'p1', 'Ha Cyg', 'Ha', 125, 30, 1, -1, 300, 1, 'c'),
             -- The same name with another exposure is another template.
                 (4, 'p1', 'Ha Cyg', 'Ha', 125, 30, 1, -1, 180, 1, 'd'),
             -- So is the same template in another profile.
                 (5, 'p2', 'Ha Cyg', 'Ha', 125, 30, 1, -1, 300, 1, 'e'),
             -- And one with Moon avoidance off.
                 (6, 'p1', 'Ha Cyg', 'Ha', 125, 30, 1, -1, 300, 0, 'f');
             INSERT INTO exposureplan VALUES (1, 300, 3, 'x'), (2, 300, 3, 'y'), (3, 300, 2, 'z'), (4, 180, 4, 'w');",
        )
        .unwrap();
        conn
    }

    fn column(conn: &Connection, sql: &str) -> Vec<i64> {
        conn.prepare(sql)
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn merges_copies_with_every_setting_equal_into_the_one_most_plans_use() {
        let conn = catalog();
        let merged = merge_repeated(&conn).unwrap();
        assert_eq!(
            merged
                .iter()
                .map(|m| (m.removed_id, m.kept_id, m.plans))
                .collect::<Vec<_>>(),
            [(2, 3, 1), (1, 3, 0)],
            "two plans use #3"
        );
        assert_eq!(
            column(&conn, "SELECT Id FROM exposuretemplate ORDER BY Id"),
            [3, 4, 5, 6]
        );
        assert_eq!(
            column(
                &conn,
                "SELECT exposureTemplateId FROM exposureplan ORDER BY Id"
            ),
            [3, 3, 3, 4]
        );
        // A second run finds nothing.
        assert!(merge_repeated(&conn).unwrap().is_empty());
    }

    #[test]
    fn a_database_without_scheduler_tables_has_nothing_to_merge() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(merge_repeated(&conn).unwrap().is_empty());
    }
}
