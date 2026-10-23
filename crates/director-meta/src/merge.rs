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
        let imported: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM collaboration_project WHERE project_id IN (?1,?2))",
            params![into.to_string(), from.to_string()],
            |row| row.get(0),
        )?;
        if imported {
            return Err(Error::Conflict);
        }
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
                draft.layout_revision = 1;
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
    /// and move its link there. The old plan keeps its drafts, less the work
    /// it planned for that database's rig: left in, its next activation
    /// would make the rig a second project or fight the new plan for this
    /// one. That work goes only with `drop_rig_work`; without it a plan
    /// that has some is refused with [`Error::Conflict`], so the caller can
    /// ask first. The old plan's activation record forgets the rig too, so
    /// its next activation does not read the rig as turned off and set the
    /// project it no longer holds Inactive.
    pub fn detach_project(
        &mut self,
        from: Uuid,
        catalog: Uuid,
        source_project_guid: Uuid,
        new_id: Uuid,
        name: &str,
        drop_rig_work: bool,
    ) -> Result<NamedIdentity, Error> {
        valid_id(from)?;
        valid_id(catalog)?;
        valid_id(source_project_guid)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let imported: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM collaboration_project WHERE project_id=?1)",
            [from.to_string()],
            |row| row.get(0),
        )?;
        if imported {
            return Err(Error::Conflict);
        }
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
        drop_rig(&tx, from, catalog, drop_rig_work)?;
        tx.commit()?;
        Ok(fresh)
    }

    /// The plan that links one database's project, with how many databases
    /// it links in all and whether it came from a collaboration.
    pub fn source_project_plan(
        &self,
        catalog: Uuid,
        source_project_guid: Uuid,
    ) -> Result<Option<SourcePlan>, Error> {
        let Some(plan) = self.linked_project(catalog, source_project_guid)? else {
            return Ok(None);
        };
        let Some(identity) = read_named(&self.connection, Kind::Project, plan)? else {
            return Ok(None);
        };
        let (links, imported): (i64, bool) = self.connection.query_row(
            "SELECT (SELECT count(*) FROM project_catalog WHERE project_id=?1),
                    EXISTS(SELECT 1 FROM collaboration_project WHERE project_id=?1)",
            [plan.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(Some(SourcePlan {
            plan: identity,
            links: u32::try_from(links).unwrap_or(u32::MAX),
            imported,
        }))
    }

    /// Forget one database's project after it was deleted there: its link,
    /// the plan's work for that database's rig, and the rig in the plan's
    /// activation record, so the next activation does not create the
    /// project again. The plan keeps its framing and its other links.
    /// Returns the plan, or `None` when no plan linked the project. A plan
    /// from a collaboration is refused with [`Error::Conflict`].
    pub fn forget_source_project(
        &mut self,
        catalog: Uuid,
        source_project_guid: Uuid,
    ) -> Result<Option<Uuid>, Error> {
        self.forget(catalog, source_project_guid, false)
    }

    /// [`MetaStore::forget_source_project`] for a plan deletion that stopped
    /// part way: the projects it deleted are forgotten even when the plan
    /// came from a collaboration, so the plan that stays does not list
    /// databases where its project is gone.
    pub fn forget_deleted_plan_project(
        &mut self,
        catalog: Uuid,
        source_project_guid: Uuid,
    ) -> Result<Option<Uuid>, Error> {
        self.forget(catalog, source_project_guid, true)
    }

    fn forget(
        &mut self,
        catalog: Uuid,
        source_project_guid: Uuid,
        imported_allowed: bool,
    ) -> Result<Option<Uuid>, Error> {
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
        let Some(plan) = linked.as_deref().map(parse_id).transpose()? else {
            return Ok(None);
        };
        let imported: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM collaboration_project WHERE project_id=?1)",
            [plan.to_string()],
            |row| row.get(0),
        )?;
        if imported && !imported_allowed {
            return Err(Error::Conflict);
        }
        let link = params![catalog.to_string(), source_project_guid.to_string()];
        tx.execute(
            "DELETE FROM project_profile WHERE catalog_id=?1 AND source_project_guid=?2",
            link,
        )?;
        tx.execute(
            "DELETE FROM draft_import WHERE catalog_id=?1 AND source_project_guid=?2",
            link,
        )?;
        tx.execute(
            "DELETE FROM project_catalog WHERE catalog_id=?1 AND source_project_guid=?2",
            link,
        )?;
        drop_rig(&tx, plan, catalog, true)?;
        tx.commit()?;
        Ok(Some(plan))
    }
}

impl MetaStore {
    /// Each database project a plan links: its catalog and source GUID.
    pub fn plan_links(&self, plan: Uuid) -> Result<Vec<(Uuid, Uuid)>, Error> {
        valid_id(plan)?;
        let mut statement = self.connection.prepare(
            "SELECT catalog_id, source_project_guid FROM project_catalog
             WHERE project_id=?1 ORDER BY catalog_id",
        )?;
        let links = statement
            .query_map([plan.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        links
            .iter()
            .map(|(catalog, guid)| Ok((parse_id(catalog)?, parse_id(guid)?)))
            .collect()
    }

    /// Whether a plan came from a collaboration.
    pub fn plan_from_collaboration(&self, plan: Uuid) -> Result<bool, Error> {
        valid_id(plan)?;
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM collaboration_project WHERE project_id=?1)",
            [plan.to_string()],
            |row| row.get(0),
        )?)
    }

    /// Delete a plan: its database links, preferences and place in project
    /// orders, framing and plan drafts, activation record and draft
    /// imports. The projects in the rig databases are the caller's to
    /// delete first; `expected` lists the links the caller handled. A plan
    /// from a collaboration leaves it in the same transaction, as
    /// [`MetaStore::leave_collaboration`] does, and the collaboration it
    /// left is returned. `leaving` says the caller previewed a plan from a
    /// collaboration. A plan that links a project not in `expected`, one
    /// with frames queued for or sent in a collaboration report, or one
    /// whose collaboration the caller did not expect, is refused with
    /// [`Error::Conflict`].
    pub fn delete_plan(
        &mut self,
        plan: Uuid,
        expected: &[(Uuid, Uuid)],
        leaving: bool,
    ) -> Result<Option<super::collaboration_leave::PlanCollaboration>, Error> {
        valid_id(plan)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Project, plan)?.is_none() {
            return Err(Error::NotFound);
        }
        let id = plan.to_string();
        // Reported frames keep the plan until the person leaves the
        // collaboration on purpose.
        let reported: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM collaboration_capture WHERE import_id IN
                (SELECT id FROM collaboration_import WHERE project_id=?1))",
            [&id],
            |row| row.get(0),
        )?;
        if reported {
            return Err(Error::Conflict);
        }
        let mut statement = tx.prepare(
            "SELECT catalog_id, source_project_guid FROM project_catalog WHERE project_id=?1",
        )?;
        let links = statement
            .query_map([&id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for (catalog, guid) in links {
            if !expected.contains(&(parse_id(&catalog)?, parse_id(&guid)?)) {
                return Err(Error::Conflict);
            }
        }
        let left = super::collaboration_leave::leave(&tx, plan)?;
        // A plan an import linked meanwhile was previewed as a plan of its
        // own: the person sees its collaboration before it goes.
        if left.is_some() && !leaving {
            return Err(Error::Conflict);
        }
        tx.execute(
            "DELETE FROM project_profile WHERE EXISTS(
                SELECT 1 FROM project_catalog link
                WHERE link.project_id=?1 AND link.catalog_id=project_profile.catalog_id
                  AND link.source_project_guid=project_profile.source_project_guid)",
            [&id],
        )?;
        tx.execute("DELETE FROM project_catalog WHERE project_id=?1", [&id])?;
        super::preferences::forget_project(&tx, plan)?;
        retire(&tx, plan)?;
        tx.commit()?;
        Ok(left)
    }
}

/// The plan that links a database's project, as [`MetaStore::source_project_plan`]
/// reads it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourcePlan {
    pub plan: NamedIdentity,
    /// Databases the plan links, this one included.
    pub links: u32,
    /// The plan came from a collaboration.
    pub imported: bool,
}

/// Take the rig of `catalog` out of `plan`: the work the plan has for it and
/// the rig in its activation record, so the next activation neither creates
/// a project there nor reads the rig as turned off. Nothing changes while
/// another project of the catalog is linked to the plan. With work for the
/// rig and without `drop_rig_work`, refused with [`Error::Conflict`].
fn drop_rig(tx: &Connection, plan: Uuid, catalog: Uuid, drop_rig_work: bool) -> Result<(), Error> {
    let Some(rig) = super::catalog_rig::read_binding(tx, catalog)? else {
        return Ok(());
    };
    // Another project of this catalog still linked to the plan keeps the
    // rig's work and its place in the activation record.
    let still_linked: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM project_catalog WHERE project_id=?1 AND catalog_id=?2)",
        params![plan.to_string(), catalog.to_string()],
        |row| row.get(0),
    )?;
    if still_linked {
        return Ok(());
    }
    if let Some(mut draft) = super::plan::read_plan(tx, plan)?
        && draft.contributions.iter().any(|c| c.rig_id == rig)
    {
        if !drop_rig_work {
            return Err(Error::Conflict);
        }
        draft.contributions.retain(|c| c.rig_id != rig);
        draft.revision = draft.revision.checked_add(1).ok_or(Error::Conflict)?;
        tx.execute(
            "UPDATE plan_draft SET revision=?2, payload=?3 WHERE project_id=?1",
            params![
                plan.to_string(),
                i64::try_from(draft.revision).map_err(|_| Error::Conflict)?,
                super::configuration::encode(&draft)?
            ],
        )?;
    }
    if let Some(mut activation) = super::activation::read_activation(tx, plan)? {
        let before = (activation.rigs.len(), activation.inactive_rigs.len());
        activation.rigs.retain(|r| r.rig_id != rig);
        activation.inactive_rigs.retain(|r| r.rig_id != rig);
        if before != (activation.rigs.len(), activation.inactive_rigs.len()) {
            tx.execute(
                "UPDATE activation SET payload=?2 WHERE project_id=?1",
                params![plan.to_string(), super::configuration::encode(&activation)?],
            )?;
        }
    }
    Ok(())
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
    tx.execute("DELETE FROM draft_import WHERE project_id=?1", [&id])?;
    // An intent's setup references go first: they point at the intent.
    tx.execute(
        "DELETE FROM project_intent_setup WHERE intent_id IN (SELECT id FROM project_intent WHERE project_id=?1)",
        [&id],
    )?;
    tx.execute("DELETE FROM project_intent WHERE project_id=?1", [&id])?;
    tx.execute("DELETE FROM framing_draft WHERE project_id=?1", [&id])?;
    tx.execute("DELETE FROM plan_draft WHERE project_id=?1", [&id])?;
    tx.execute("DELETE FROM global_project WHERE id=?1", [&id])?;
    Ok(())
}
