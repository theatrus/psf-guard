//! Confirmed source links. Names, slugs and source row numbers are not identities.
//! The host verifies source evidence before calling; links grant no execution rights.

use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectMapping {
    pub catalog_id: Uuid,
    pub source_project_guid: Uuid,
    pub source_profile_id: String,
    pub project_id: Uuid,
    pub rig_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MappingPage {
    pub items: Vec<ProjectMapping>,
    pub next_after: Option<Uuid>,
}

impl MetaStore {
    pub fn catalog_identity(&self, id: Uuid) -> Result<Option<CatalogIdentity>, Error> {
        read_catalog(&self.connection, id)
    }

    pub fn linked_rig(&self, catalog: Uuid, profile: &str) -> Result<Option<Uuid>, Error> {
        valid_id(catalog)?;
        validate_profile(profile)?;
        read_rig_link(&self.connection, catalog, profile)
    }

    /// Link one source project and its profile atomically to existing identities.
    /// Retries are idempotent; moving a confirmed link requires a future explicit
    /// reassignment workflow, not an upsert. No identity is created implicitly.
    pub fn link_catalog_project(&mut self, mapping: &ProjectMapping) -> Result<(), Error> {
        self.link_catalog_projects(std::slice::from_ref(mapping))
    }

    /// Apply a bounded batch in one transaction, including conflicts within the
    /// batch itself. A failed entry rolls back every new link in this batch.
    pub fn link_catalog_projects(&mut self, mappings: &[ProjectMapping]) -> Result<(), Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::link_catalog_projects_on(&tx, mappings)?;
        tx.commit()?;
        Ok(())
    }

    /// Exercise exactly the adoption transaction, then roll it back. This uses a
    /// short metadata write transaction but commits no catalog or mapping rows.
    /// The host separately validates source evidence through a read-only handle.
    pub fn preview_catalog_adoption(
        &mut self,
        catalog: CatalogIdentity,
        mappings: &[ProjectMapping],
        require_new: bool,
    ) -> Result<(), Error> {
        self.start_catalog_adoption(catalog, mappings, require_new)?
            .rollback()?;
        Ok(())
    }

    /// Register lineage and every confirmed mapping in one metadata transaction.
    /// The catalog file's identity must already be durable; retrying after a
    /// coordinator failure must retain that identity, not mint another catalog.
    pub fn adopt_catalog_projects(
        &mut self,
        catalog: CatalogIdentity,
        mappings: &[ProjectMapping],
    ) -> Result<(), Error> {
        self.adopt_catalog_projects_after(catalog, mappings, false, || Ok(()))
    }

    /// Hold the coordinator writer through catalog finalization, then commit
    /// every mapping. A failed finalizer rolls metadata back. If the final meta
    /// commit fails, the host must retain the catalog's durable ID for retry.
    pub fn adopt_catalog_projects_after(
        &mut self,
        catalog: CatalogIdentity,
        mappings: &[ProjectMapping],
        require_new: bool,
        finalize_catalog: impl FnOnce() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let tx = self.start_catalog_adoption(catalog, mappings, require_new)?;
        finalize_catalog()?;
        tx.commit()?;
        Ok(())
    }

    fn start_catalog_adoption(
        &mut self,
        catalog: CatalogIdentity,
        mappings: &[ProjectMapping],
        require_new: bool,
    ) -> Result<rusqlite::Transaction<'_>, Error> {
        if mappings
            .iter()
            .any(|mapping| mapping.catalog_id != catalog.id)
        {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if require_new && read_catalog(&tx, catalog.id)?.is_some() {
            return Err(Error::Conflict);
        }
        register_catalog_on(&tx, catalog)?;
        Self::link_catalog_projects_on(&tx, mappings)?;
        Ok(tx)
    }

    fn link_catalog_projects_on(tx: &Connection, mappings: &[ProjectMapping]) -> Result<(), Error> {
        if !(1..=256).contains(&mappings.len()) {
            return Err(Error::InvalidInput);
        }
        for mapping in mappings {
            validate_mapping(mapping)?;
        }
        for mapping in mappings {
            if read_catalog(tx, mapping.catalog_id)?.is_none()
                || read_named(tx, Kind::Project, mapping.project_id)?.is_none()
                || read_named(tx, Kind::Rig, mapping.rig_id)?.is_none()
            {
                return Err(Error::NotFound);
            }
            let old_project: Option<String> = tx.query_row(
            "SELECT project_id FROM project_catalog WHERE catalog_id=?1 AND source_project_guid=?2",
            params![mapping.catalog_id.to_string(), mapping.source_project_guid.to_string()],
            |row| row.get(0),
        ).optional()?;
            if old_project
                .as_deref()
                .map(parse_id)
                .transpose()?
                .is_some_and(|id| id != mapping.project_id)
            {
                return Err(Error::Conflict);
            }
            let old_rig = read_rig_link(tx, mapping.catalog_id, &mapping.source_profile_id)?;
            if old_rig.is_some_and(|id| id != mapping.rig_id) {
                return Err(Error::Conflict);
            }
            let old_profile: Option<String> = tx.query_row(
            "SELECT source_profile_id FROM project_profile WHERE catalog_id=?1 AND source_project_guid=?2",
            params![mapping.catalog_id.to_string(), mapping.source_project_guid.to_string()],
            |row| row.get(0),
        ).optional()?;
            if let Some(profile) = &old_profile {
                validate_profile(profile).map_err(|_| Error::CorruptDatabase)?;
                if old_project.is_none() {
                    return Err(Error::CorruptDatabase);
                }
                // Do not repair an orphan by assigning it to the caller's proposed rig.
                if read_rig_link(tx, mapping.catalog_id, profile)?.is_none() {
                    return Err(Error::CorruptDatabase);
                }
            }
            if old_profile
                .as_ref()
                .is_some_and(|id| id != &mapping.source_profile_id)
            {
                return Err(Error::Conflict);
            }
            if old_project.is_none() {
                tx.execute("INSERT INTO project_catalog(catalog_id,source_project_guid,project_id) VALUES(?1,?2,?3)",
                params![mapping.catalog_id.to_string(), mapping.source_project_guid.to_string(), mapping.project_id.to_string()])?;
            }
            if old_rig.is_none() {
                tx.execute(
                "INSERT INTO catalog_profile(catalog_id,source_profile_id,rig_id) VALUES(?1,?2,?3)",
                params![
                    mapping.catalog_id.to_string(),
                    mapping.source_profile_id,
                    mapping.rig_id.to_string()
                ],
            )?;
            }
            if old_profile.is_none() {
                tx.execute("INSERT INTO project_profile(catalog_id,source_project_guid,source_profile_id) VALUES(?1,?2,?3)",
                params![mapping.catalog_id.to_string(), mapping.source_project_guid.to_string(), mapping.source_profile_id])?;
            }
        }
        Ok(())
    }

    /// Complete mappings only. Legacy project-only links remain available via
    /// linked_project but do not imply a rig until explicitly confirmed.
    pub fn catalog_project_mappings(
        &self,
        catalog: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<MappingPage, Error> {
        valid_id(catalog)?;
        if let Some(after) = after {
            valid_id(after)?;
        }
        if !(1..=256).contains(&limit) {
            return Err(Error::InvalidInput);
        }
        let tx = self.connection.unchecked_transaction()?;
        if read_catalog(&tx, catalog)?.is_none() {
            return Err(Error::NotFound);
        }
        let mut statement = tx.prepare(
            "SELECT p.source_project_guid,p.source_profile_id,j.project_id,r.rig_id
             FROM project_profile p
             LEFT JOIN project_catalog j ON j.catalog_id=p.catalog_id AND j.source_project_guid=p.source_project_guid
             LEFT JOIN catalog_profile r ON r.catalog_id=p.catalog_id AND r.source_profile_id=p.source_profile_id
             WHERE p.catalog_id=?1 AND p.source_project_guid>?2
             ORDER BY p.source_project_guid LIMIT ?3"
        )?;
        let mut rows = statement.query(params![
            catalog.to_string(),
            after.map(|id| id.to_string()).unwrap_or_default(),
            (limit + 1) as i64
        ])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            let project: Option<String> = row.get(2)?;
            let rig: Option<String> = row.get(3)?;
            let mapping = ProjectMapping {
                catalog_id: catalog,
                source_project_guid: parse_id(&row.get::<_, String>(0)?)?,
                source_profile_id: row.get(1)?,
                project_id: parse_id(project.as_deref().ok_or(Error::CorruptDatabase)?)?,
                rig_id: parse_id(rig.as_deref().ok_or(Error::CorruptDatabase)?)?,
            };
            validate_mapping(&mapping).map_err(|_| Error::CorruptDatabase)?;
            if read_named(&tx, Kind::Rig, mapping.rig_id)?.is_none()
                || read_named(&tx, Kind::Project, mapping.project_id)?.is_none()
            {
                return Err(Error::CorruptDatabase);
            }
            items.push(mapping);
        }
        let next_after = (items.len() > limit).then(|| items[limit - 1].source_project_guid);
        items.truncate(limit);
        drop(rows);
        drop(statement);
        tx.commit()?;
        Ok(MappingPage { items, next_after })
    }
}

fn read_catalog(conn: &Connection, id: Uuid) -> Result<Option<CatalogIdentity>, Error> {
    valid_id(id)?;
    let origin: Option<String> = conn
        .query_row(
            "SELECT origin_instance_id FROM catalog WHERE id=?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    origin
        .map(|origin| {
            Ok(CatalogIdentity {
                id,
                origin_instance_id: parse_id(&origin)?,
            })
        })
        .transpose()
}

fn read_rig_link(conn: &Connection, catalog: Uuid, profile: &str) -> Result<Option<Uuid>, Error> {
    let rig: Option<String> = conn
        .query_row(
            "SELECT rig_id FROM catalog_profile WHERE catalog_id=?1 AND source_profile_id=?2",
            params![catalog.to_string(), profile],
            |row| row.get(0),
        )
        .optional()?;
    let rig = rig.map(|id| parse_id(&id)).transpose()?;
    if let Some(id) = rig
        && (read_named(conn, Kind::Rig, id)?.is_none() || read_catalog(conn, catalog)?.is_none())
    {
        return Err(Error::CorruptDatabase);
    }
    Ok(rig)
}

fn validate_mapping(mapping: &ProjectMapping) -> Result<(), Error> {
    for id in [
        mapping.catalog_id,
        mapping.source_project_guid,
        mapping.project_id,
        mapping.rig_id,
    ] {
        valid_id(id)?;
    }
    validate_profile(&mapping.source_profile_id)
}

fn validate_profile(profile: &str) -> Result<(), Error> {
    // Opaque source identity: do not trim, lowercase, or guess UUID formatting.
    if profile.len() > 512 || profile.trim().is_empty() || profile.chars().any(char::is_control) {
        return Err(Error::InvalidInput);
    }
    Ok(())
}
