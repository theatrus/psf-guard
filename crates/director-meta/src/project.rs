//! Immutable project intent, validated against explicitly stored rig setups.
//! No active-plan pointer, assignments, progress, or acquisition permissions.

use super::configuration::{decode, encode, read_setup, snapshot_ids, SnapshotIds};
use super::*;
use psf_guard_director_core::{
    project::{BoundProject, Project},
    MAX_REQUEST_BYTES,
};
use std::collections::{BTreeMap, BTreeSet};

impl MetaStore {
    pub fn register_project_intent(&mut self, project: &Project) -> Result<(), Error> {
        let bound = BoundProject::new(project.clone()).map_err(|_| Error::InvalidInput)?;
        let (id, owner, setups) = identities(project).map_err(|_| Error::InvalidInput)?;
        let payload = encode(project)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Project, owner)?.is_none() {
            return Err(Error::NotFound);
        }
        if let Some(existing) = read_project(&tx, id)? {
            if existing != *project {
                return Err(Error::Conflict);
            }
        } else {
            validate_setups(&tx, &bound, &setups)?;
            tx.execute(
                "INSERT INTO project_intent(id,project_id,payload) VALUES(?1,?2,?3)",
                params![id.to_string(), owner.to_string(), payload],
            )?;
            for setup in setups {
                tx.execute(
                    "INSERT INTO project_intent_setup(intent_id,setup_id) VALUES(?1,?2)",
                    params![id.to_string(), setup.to_string()],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn project_intent(&self, id: Uuid) -> Result<Option<Project>, Error> {
        // Keep payload, reference inventory and immutable setup reads on one snapshot.
        let tx = self.connection.unchecked_transaction()?;
        let result = read_project(&tx, id)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn project_intent_ids(
        &self,
        project: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<SnapshotIds, Error> {
        snapshot_ids(
            &self.connection,
            "SELECT id FROM project_intent WHERE project_id=?1 AND id>?2 ORDER BY id LIMIT ?3",
            project,
            after,
            limit,
        )
    }
}

fn identities(project: &Project) -> Result<(Uuid, Uuid, BTreeSet<Uuid>), Error> {
    let id = parse_id(&project.id)?;
    let owner = parse_id(&project.project_id)?;
    let mut setups = BTreeSet::new();
    for contribution in &project.contributions {
        parse_id(&contribution.rig_id)?;
        setups.insert(parse_id(&contribution.setup_id)?);
    }
    Ok((id, owner, setups))
}

fn validate_setups(
    conn: &Connection,
    project: &BoundProject,
    ids: &BTreeSet<Uuid>,
) -> Result<(), Error> {
    let mut setups = BTreeMap::new();
    for id in ids {
        setups.insert(
            id.to_string(),
            read_setup(conn, *id)?.ok_or(Error::NotFound)?,
        );
    }
    for contribution in &project.snapshot().contributions {
        let setup = setups
            .get(&contribution.setup_id)
            .ok_or(Error::InvalidInput)?;
        project
            .resolve(
                &contribution.id,
                &setup.id.to_string(),
                &setup.configuration,
            )
            .map_err(|_| Error::InvalidInput)?;
    }
    Ok(())
}

fn read_project(conn: &Connection, id: Uuid) -> Result<Option<Project>, Error> {
    valid_id(id)?;
    let row: Option<(String, Vec<u8>)> = conn
        .query_row(
            "SELECT project_id,substr(CAST(payload AS BLOB),1,?2) FROM project_intent WHERE id=?1",
            params![id.to_string(), (MAX_REQUEST_BYTES + 1) as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(owner, payload)| {
        let project: Project = decode(payload)?;
        let bound = BoundProject::new(project.clone()).map_err(|_| Error::CorruptDatabase)?;
        let (stored_id, stored_owner, setups) = identities(&project)?;
        if id != stored_id || stored_owner != parse_id(&owner)?
            || read_named(conn, Kind::Project, stored_owner)?.is_none()
        {
            return Err(Error::CorruptDatabase);
        }
        let mut statement = conn.prepare("SELECT setup_id FROM project_intent_setup WHERE intent_id=?1 ORDER BY setup_id LIMIT 257")?;
        let rows = statement.query_map([id.to_string()], |r| r.get::<_, String>(0))?;
        let mut stored_setups = BTreeSet::new();
        for row in rows { stored_setups.insert(parse_id(&row?)?); }
        if stored_setups != setups {
            return Err(Error::CorruptDatabase);
        }
        validate_setups(conn, &bound, &setups).map_err(|error| match error {
            Error::Sqlite(_) => error,
            _ => Error::CorruptDatabase,
        })?;
        Ok(project)
    }).transpose()
}
