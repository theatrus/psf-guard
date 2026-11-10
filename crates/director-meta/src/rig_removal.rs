//! Delete a rig and what the store keeps about it, when its database leaves
//! PSF Guard. Plans stay: each loses this rig's part, and any other rig keeps
//! its own. There is no undo, so work the rig still has open refuses it.

use super::*;

/// Why a rig cannot be removed yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalBlocker {
    /// The plugin holds an assignment nobody has released or closed.
    OpenWork,
}

/// What removing a rig deletes, for the person to review first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RigRemoval {
    pub rig: NamedIdentity,
    /// The plans that lose this rig, by name.
    pub plans: Vec<String>,
    /// The Director plugins that must pair again, by name.
    pub plugins: Vec<String>,
    pub blocker: Option<RemovalBlocker>,
    /// The rig has collaboration records, which can hold reports not sent
    /// yet. Its records stay, so only the database leaves PSF Guard.
    pub collaboration: bool,
}

impl MetaStore {
    /// What [`MetaStore::remove_rig`] would delete. `None` when the catalog
    /// has no rig.
    pub fn rig_removal(&self, catalog: Uuid) -> Result<Option<RigRemoval>, Error> {
        valid_id(catalog)?;
        let Some(rig) = super::catalog_rig::read_binding(&self.connection, catalog)? else {
            return Ok(None);
        };
        review(&self.connection, catalog, rig).map(Some)
    }

    /// Delete the catalog's rig. [`Error::Conflict`] when it has a blocker
    /// or collaboration records. `None` when the catalog has no rig.
    pub fn remove_rig(&mut self, catalog: Uuid) -> Result<Option<RigRemoval>, Error> {
        valid_id(catalog)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(rig) = super::catalog_rig::read_binding(&tx, catalog)? else {
            return Ok(None);
        };
        let removal = review(&tx, catalog, rig)?;
        if removal.blocker.is_some() || removal.collaboration {
            return Err(Error::Conflict);
        }
        // Rows reference each other across tables; the checks run at commit,
        // so a row this missed stops the removal instead of being left behind.
        tx.pragma_update(None, "defer_foreign_keys", true)?;
        for project in plans_of(&tx, catalog, rig)? {
            drop_from_plan(&tx, project, rig)?;
        }
        let rig_id = rig.to_string();
        let catalog_id = catalog.to_string();
        tx.execute(
            "DELETE FROM equipment_report WHERE client_id IN
                (SELECT id FROM director_client WHERE rig_id=?1)",
            [&rig_id],
        )?;
        tx.execute(
            "DELETE FROM project_intent_setup WHERE setup_id IN
                (SELECT id FROM rig_setup WHERE rig_id=?1)",
            [&rig_id],
        )?;
        for table in [
            "night_availability_window",
            "night_availability",
            "night_availability_scope",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE scope='rig' AND scope_id=?1"),
                [&rig_id],
            )?;
        }
        tx.execute(
            "DELETE FROM observing_preferences WHERE scope='rig' AND scope_id=?1",
            [&rig_id],
        )?;
        tx.execute(
            "DELETE FROM project_profile WHERE catalog_id=?1",
            [&catalog_id],
        )?;
        for table in tables_with(&tx, "rig_id")? {
            tx.execute(
                &format!("DELETE FROM \"{table}\" WHERE rig_id=?1"),
                [&rig_id],
            )?;
        }
        for table in tables_with(&tx, "catalog_id")? {
            tx.execute(
                &format!("DELETE FROM \"{table}\" WHERE catalog_id=?1"),
                [&catalog_id],
            )?;
        }
        tx.execute("DELETE FROM catalog WHERE id=?1", [&catalog_id])?;
        tx.execute("DELETE FROM rig WHERE id=?1", [&rig_id])?;
        tx.commit()?;
        if let Some(liveness) = &self.liveness {
            liveness.rig_removed(rig);
        }
        Ok(Some(removal))
    }
}

fn review(conn: &Connection, catalog: Uuid, rig: Uuid) -> Result<RigRemoval, Error> {
    let identity = read_named(conn, Kind::Rig, rig)?.ok_or(Error::CorruptDatabase)?;
    let id = rig.to_string();
    let mut plans = Vec::new();
    for project in plans_of(conn, catalog, rig)? {
        plans.push(
            read_named(conn, Kind::Project, project)?
                .ok_or(Error::CorruptDatabase)?
                .name,
        );
    }
    plans.sort();
    let mut statement =
        conn.prepare("SELECT name FROM director_client WHERE rig_id=?1 ORDER BY name")?;
    let plugins = statement
        .query_map([&id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    let open: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM execution_allocation a WHERE a.rig_id=?1 AND NOT EXISTS(
            SELECT 1 FROM workload_history h
            WHERE h.allocation_id=a.allocation_id AND json_extract(h.payload,'$.released')=1))",
        [&id],
        |row| row.get(0),
    )?;
    let collaborating: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM collaboration_connection WHERE rig_id=?1)",
        [&id],
        |row| row.get(0),
    )?;
    Ok(RigRemoval {
        rig: identity,
        plans,
        plugins,
        blocker: open.then_some(RemovalBlocker::OpenWork),
        collaboration: collaborating,
    })
}

/// Every plan with something of this rig's: a linked project, a part in the
/// plan, a place in the framing, or a place in the last activation.
fn plans_of(conn: &Connection, catalog: Uuid, rig: Uuid) -> Result<Vec<Uuid>, Error> {
    let mut found = std::collections::BTreeSet::new();
    let mut linked = conn.prepare("SELECT project_id FROM project_catalog WHERE catalog_id=?1")?;
    for id in linked.query_map([catalog.to_string()], |row| row.get::<_, String>(0))? {
        found.insert(parse_id(&id?)?);
    }
    for table in ["plan_draft", "framing_draft", "activation"] {
        let mut rows = conn.prepare(&format!("SELECT project_id FROM {table}"))?;
        let ids = rows
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in ids {
            let project = parse_id(&id)?;
            let holds = match table {
                "plan_draft" => super::plan::read_plan(conn, project)?
                    .is_some_and(|plan| plan.contributions.iter().any(|c| c.rig_id == rig)),
                "framing_draft" => super::framing::read_draft(conn, project)?.is_some_and(|f| {
                    f.panel_rig_id == Some(rig)
                        || f.shown_rig_ids.contains(&rig)
                        || f.rig_framings.iter().any(|r| r.rig_id == rig)
                }),
                _ => super::activation::read_activation(conn, project)?.is_some_and(|a| {
                    a.rigs.iter().any(|r| r.rig_id == rig)
                        || a.inactive_rigs.iter().any(|r| r.rig_id == rig)
                }),
            };
            if holds {
                found.insert(project);
            }
        }
    }
    Ok(found.into_iter().collect())
}

/// Take the rig out of one plan's draft, framing and activation record. A
/// page open on the old revision is refused on save, as for any other edit.
fn drop_from_plan(conn: &Connection, project: Uuid, rig: Uuid) -> Result<(), Error> {
    let id = project.to_string();
    if let Some(mut plan) = super::plan::read_plan(conn, project)? {
        let before = plan.contributions.len();
        plan.contributions.retain(|c| c.rig_id != rig);
        if plan.contributions.len() != before {
            plan.revision = plan.revision.checked_add(1).ok_or(Error::Conflict)?;
            write(conn, "plan_draft", &id, plan.revision, &plan)?;
        }
    }
    if let Some(mut framing) = super::framing::read_draft(conn, project)? {
        let holds = framing.panel_rig_id == Some(rig)
            || framing.shown_rig_ids.contains(&rig)
            || framing.rig_framings.iter().any(|r| r.rig_id == rig);
        if holds {
            // The panel size stays: it was copied so the framing reads the
            // same when its rig changes or goes.
            if framing.panel_rig_id == Some(rig) {
                framing.panel_rig_id = None;
            }
            framing.shown_rig_ids.retain(|shown| *shown != rig);
            framing.rig_framings.retain(|r| r.rig_id != rig);
            framing.revision = framing.revision.checked_add(1).ok_or(Error::Conflict)?;
            write(conn, "framing_draft", &id, framing.revision, &framing)?;
        }
    }
    if let Some(mut activation) = super::activation::read_activation(conn, project)? {
        activation.rigs.retain(|r| r.rig_id != rig);
        activation.inactive_rigs.retain(|r| r.rig_id != rig);
        activation.pushes.retain(|p| p.rig_id != rig);
        if activation.rigs.is_empty() && activation.inactive_rigs.is_empty() {
            conn.execute("DELETE FROM activation WHERE project_id=?1", [&id])?;
        } else {
            activation.revision = activation.revision.checked_add(1).ok_or(Error::Conflict)?;
            write(conn, "activation", &id, activation.revision, &activation)?;
        }
    }
    Ok(())
}

fn write<T: Serialize>(
    conn: &Connection,
    table: &str,
    project: &str,
    revision: u64,
    value: &T,
) -> Result<(), Error> {
    conn.execute(
        &format!("UPDATE {table} SET revision=?2, payload=?3 WHERE project_id=?1"),
        params![
            project,
            i64::try_from(revision).map_err(|_| Error::Conflict)?,
            super::configuration::encode(value)?
        ],
    )?;
    Ok(())
}

/// The store's tables with this column.
fn tables_with(conn: &Connection, column: &str) -> Result<Vec<String>, Error> {
    let mut statement = conn.prepare(
        "SELECT m.name FROM sqlite_master m JOIN pragma_table_info(m.name) c
         WHERE m.type='table' AND c.name=?1 ORDER BY m.name",
    )?;
    let tables = statement
        .query_map([column], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(tables)
}
