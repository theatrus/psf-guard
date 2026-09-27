//! Reviewed adoption of a registered database as a rig, without rewriting history.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogRig {
    pub catalog: CatalogIdentity,
    pub rig: NamedIdentity,
}

impl MetaStore {
    pub fn catalog_rig(&self, catalog: Uuid) -> Result<Option<CatalogRig>, Error> {
        valid_id(catalog)?;
        let Some(rig) = read_binding(&self.connection, catalog)? else {
            return Ok(None);
        };
        Ok(Some(CatalogRig {
            catalog: super::catalog::read_catalog(&self.connection, catalog)?
                .ok_or(Error::CorruptDatabase)?,
            rig: read_named(&self.connection, Kind::Rig, rig)?.ok_or(Error::CorruptDatabase)?,
        }))
    }

    /// Validate and roll back the same transaction used by Apply. An unambiguous
    /// prototype rig keeps its ID so immutable setup/intent references survive.
    pub fn preview_catalog_rig(
        &mut self,
        catalog: CatalogIdentity,
        name: &str,
        require_new: bool,
    ) -> Result<CatalogRig, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = bind_on(&tx, catalog, name, require_new)?;
        tx.rollback()?;
        Ok(result)
    }

    /// Publish the source's durable lineage before committing its rig binding.
    /// If that final commit fails, the caller retains the lineage for exact retry.
    pub fn bind_catalog_rig_after(
        &mut self,
        catalog: CatalogIdentity,
        name: &str,
        require_new: bool,
        finalize_catalog: impl FnOnce() -> Result<(), Error>,
    ) -> Result<CatalogRig, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = bind_on(&tx, catalog, name, require_new)?;
        finalize_catalog()?;
        tx.commit()?;
        Ok(result)
    }
}

pub(crate) fn read_binding(conn: &Connection, catalog: Uuid) -> Result<Option<Uuid>, Error> {
    conn.query_row(
        "SELECT rig_id FROM catalog_rig WHERE catalog_id=?1",
        [catalog.to_string()],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .as_deref()
    .map(parse_id)
    .transpose()
}

pub(crate) fn check_mapping(conn: &Connection, catalog: Uuid, rig: Uuid) -> Result<(), Error> {
    if read_binding(conn, catalog)?.is_some_and(|bound| bound != rig) {
        return Err(Error::Conflict);
    }
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM catalog_rig WHERE rig_id=?1 AND catalog_id!=?2)",
        params![rig.to_string(), catalog.to_string()],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(Error::Conflict);
    }
    Ok(())
}

fn bind_on(
    conn: &Connection,
    catalog: CatalogIdentity,
    name: &str,
    require_new: bool,
) -> Result<CatalogRig, Error> {
    valid_name(name)?;
    if require_new && super::catalog::read_catalog(conn, catalog.id)?.is_some() {
        return Err(Error::Conflict);
    }
    register_catalog_on(conn, catalog)?;
    let mut statement =
        conn.prepare("SELECT DISTINCT rig_id FROM catalog_profile WHERE catalog_id=?1 LIMIT 2")?;
    let legacy = statement
        .query_map([catalog.id.to_string()], |r| r.get::<_, String>(0))?
        .map(|row| parse_id(&row?))
        .collect::<Result<Vec<_>, Error>>()?;
    if legacy.len() > 1 {
        return Err(Error::Conflict);
    }
    let bound = read_binding(conn, catalog.id)?;
    if bound
        .zip(legacy.first().copied())
        .is_some_and(|(a, b)| a != b)
    {
        return Err(Error::Conflict);
    }
    let id = bound
        .or_else(|| legacy.first().copied())
        .unwrap_or(catalog.id);
    check_mapping(conn, catalog.id, id)?;
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM catalog_profile WHERE rig_id=?1 AND catalog_id!=?2)",
        params![id.to_string(), catalog.id.to_string()],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(Error::Conflict);
    }
    let rig = if bound.is_some() || !legacy.is_empty() {
        read_named(conn, Kind::Rig, id)?.ok_or(Error::CorruptDatabase)?
    } else {
        // Do not claim an unrelated prototype identity merely because UUIDs match.
        if read_named(conn, Kind::Rig, id)?.is_some() {
            return Err(Error::Conflict);
        }
        conn.execute(
            "INSERT INTO rig(id,name,revision) VALUES(?1,?2,1)",
            params![id.to_string(), name],
        )?;
        NamedIdentity {
            id,
            name: name.into(),
            revision: 1,
        }
    };
    if bound.is_none() {
        conn.execute(
            "INSERT INTO catalog_rig(catalog_id,rig_id) VALUES(?1,?2)",
            params![catalog.id.to_string(), id.to_string()],
        )?;
    }
    Ok(CatalogRig { catalog, rig })
}
