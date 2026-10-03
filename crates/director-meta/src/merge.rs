//! Joining and splitting plans. Two databases that each made their own
//! project for one target become two plans; attaching one to the other
//! moves its database links over and retires it. The reverse hands one
//! database's project a plan of its own again. Only the meta store moves;
//! the rig databases are not touched, and the next activation takes the
//! attached project's rows over where they stand.

use super::*;

/// What an attach did.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Attached {
    pub into: NamedIdentity,
    /// The plan that was absorbed, as it was.
    pub absorbed: NamedIdentity,
    pub moved_links: u32,
    /// Whether the absorbed plan's framing draft became this plan's, because
    /// this plan had none.
    pub framing_taken: bool,
    /// The same for the plan draft.
    pub plan_taken: bool,
}

impl MetaStore {
    /// Move every database link of `from` onto `into` and retire `from`.
    /// `into` keeps its own drafts; it takes `from`'s only where it has
    /// none. Refused when both plans link the same database, since a rig
    /// shoots one project per plan.
    pub fn attach_project(&mut self, into: Uuid, from: Uuid) -> Result<Attached, Error> {
        valid_id(into)?;
        valid_id(from)?;
        if into == from {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let target = read_named(&tx, Kind::Project, into)?.ok_or(Error::NotFound)?;
        let absorbed = read_named(&tx, Kind::Project, from)?.ok_or(Error::NotFound)?;
        let clash: i64 = tx.query_row(
            "SELECT count(*) FROM project_catalog a JOIN project_catalog b ON a.catalog_id=b.catalog_id
             WHERE a.project_id=?1 AND b.project_id=?2",
            params![into.to_string(), from.to_string()],
            |row| row.get(0),
        )?;
        if clash > 0 {
            return Err(Error::Conflict);
        }
        let moved = tx.execute(
            "UPDATE project_catalog SET project_id=?1 WHERE project_id=?2",
            params![into.to_string(), from.to_string()],
        )?;
        let framing_taken = take_draft::<super::framing::FramingDraft>(
            &tx,
            "framing_draft",
            into,
            from,
            |draft, id| {
                draft.project_id = id;
                draft.revision = 1;
            },
            super::framing::read_draft,
        )?;
        let plan_taken = take_draft::<super::plan::PlanDraft>(
            &tx,
            "plan_draft",
            into,
            from,
            |draft, id| {
                draft.project_id = id;
                draft.revision = 1;
            },
            super::plan::read_plan,
        )?;
        super::preferences::attach_project(&tx, self.instance_id, into, from)?;
        retire(&tx, from)?;
        tx.commit()?;
        Ok(Attached {
            into: target,
            absorbed,
            moved_links: u32::try_from(moved).unwrap_or(u32::MAX),
            framing_taken,
            plan_taken,
        })
    }

    /// Give one database's project a plan of its own again, named `name`,
    /// and move its link there. The old plan keeps its drafts.
    pub fn detach_project(
        &mut self,
        from: Uuid,
        catalog: Uuid,
        source_project_guid: Uuid,
        new_id: Uuid,
        name: &str,
    ) -> Result<NamedIdentity, Error> {
        valid_id(from)?;
        valid_id(catalog)?;
        valid_id(source_project_guid)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let linked: Option<String> = tx
            .query_row(
                "SELECT project_id FROM project_catalog WHERE catalog_id=?1 AND source_project_guid=?2",
                params![catalog.to_string(), source_project_guid.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if linked.as_deref().map(parse_id).transpose()? != Some(from) {
            return Err(Error::NotFound);
        }
        valid_id(new_id)?;
        valid_name(name)?;
        if read_named(&tx, Kind::Project, new_id)?.is_some() {
            return Err(Error::Conflict);
        }
        tx.execute(
            "INSERT INTO global_project(id,name,revision) VALUES(?1,?2,1)",
            params![new_id.to_string(), name],
        )?;
        let fresh = NamedIdentity {
            id: new_id,
            name: name.into(),
            revision: 1,
        };
        tx.execute(
            "UPDATE project_catalog SET project_id=?1 WHERE catalog_id=?2 AND source_project_guid=?3",
            params![
                new_id.to_string(),
                catalog.to_string(),
                source_project_guid.to_string()
            ],
        )?;
        tx.commit()?;
        Ok(fresh)
    }
}

/// Move `from`'s draft in `table` to `into` when `into` has none; drop it
/// otherwise. Returns whether it moved.
fn take_draft<T: Serialize>(
    tx: &Connection,
    table: &str,
    into: Uuid,
    from: Uuid,
    rehome: impl Fn(&mut T, Uuid),
    read: impl Fn(&Connection, Uuid) -> Result<Option<T>, Error>,
) -> Result<bool, Error> {
    let has_own: i64 = tx.query_row(
        &format!("SELECT count(*) FROM {table} WHERE project_id=?1"),
        [into.to_string()],
        |row| row.get(0),
    )?;
    let theirs = read(tx, from)?;
    tx.execute(
        &format!("DELETE FROM {table} WHERE project_id=?1"),
        [from.to_string()],
    )?;
    match theirs {
        Some(mut draft) if has_own == 0 => {
            rehome(&mut draft, into);
            let payload = super::configuration::encode(&draft)?;
            tx.execute(
                &format!("INSERT INTO {table}(project_id,revision,payload) VALUES(?1,1,?2)"),
                params![into.to_string(), payload],
            )?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Remove a plan that has no links left, and everything that hung off it.
fn retire(tx: &Connection, project: Uuid) -> Result<(), Error> {
    let id = project.to_string();
    let left: i64 = tx.query_row(
        "SELECT count(*) FROM project_catalog WHERE project_id=?1",
        [&id],
        |row| row.get(0),
    )?;
    if left > 0 {
        return Err(Error::Conflict);
    }
    tx.execute("DELETE FROM activation WHERE project_id=?1", [&id])?;
    tx.execute("DELETE FROM project_intent WHERE project_id=?1", [&id])?;
    tx.execute("DELETE FROM framing_draft WHERE project_id=?1", [&id])?;
    tx.execute("DELETE FROM plan_draft WHERE project_id=?1", [&id])?;
    tx.execute("DELETE FROM global_project WHERE id=?1", [&id])?;
    Ok(())
}
