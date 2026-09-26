//! Coordinator identities, never TS catalog tables or local execution evidence.
//! Opening is explicit: no startup hook, path discovery, catalog import, or HTTP.

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{fmt, path::Path, time::Duration};
pub use uuid::Uuid;
mod storage;

#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    InvalidInput,
    ForeignDatabase,
    UnsupportedSchema,
    CorruptDatabase,
    Conflict,
    NotFound,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(_) => f.write_str("Director meta database operation failed"),
            Self::Io(_) => f.write_str("Director meta file operation failed"),
            other => write!(f, "Director meta: {other:?}"),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NamedIdentity {
    pub id: Uuid,
    pub name: String,
    pub revision: u64,
}

/// The originating catalog identity is not a URL slug, file name or telescope.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogIdentity {
    pub id: Uuid,
    pub origin_instance_id: Uuid,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectLink {
    pub project_id: Uuid,
    pub catalog_id: Uuid,
    pub source_project_guid: Uuid,
}

/// Local blocking storage. Hosts must not hold this across network operations.
/// A rig is independent of a catalog; neither is inferred from display names.
pub struct MetaStore {
    connection: Connection,
    instance_id: Uuid,
}

impl MetaStore {
    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    /// Caller-minted stable IDs make retries explicit. Same ID and different
    /// content is a conflict; duplicate names intentionally remain independent.
    pub fn create_project(&mut self, id: Uuid, name: &str) -> Result<NamedIdentity, Error> {
        self.create_named(Kind::Project, id, name)
    }
    pub fn create_rig(&mut self, id: Uuid, name: &str) -> Result<NamedIdentity, Error> {
        self.create_named(Kind::Rig, id, name)
    }
    pub fn project(&self, id: Uuid) -> Result<Option<NamedIdentity>, Error> {
        read_named(&self.connection, Kind::Project, id)
    }
    pub fn rig(&self, id: Uuid) -> Result<Option<NamedIdentity>, Error> {
        read_named(&self.connection, Kind::Rig, id)
    }
    pub fn rename_project(
        &mut self,
        id: Uuid,
        revision: u64,
        name: &str,
    ) -> Result<NamedIdentity, Error> {
        self.rename(Kind::Project, id, revision, name)
    }
    pub fn rename_rig(
        &mut self,
        id: Uuid,
        revision: u64,
        name: &str,
    ) -> Result<NamedIdentity, Error> {
        self.rename(Kind::Rig, id, revision, name)
    }

    pub fn register_catalog(&mut self, catalog: CatalogIdentity) -> Result<(), Error> {
        valid_id(catalog.id)?;
        valid_id(catalog.origin_instance_id)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT origin_instance_id FROM catalog WHERE id=?1",
                [catalog.id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(origin) = existing {
            if parse_id(&origin)? != catalog.origin_instance_id {
                return Err(Error::Conflict);
            }
        } else {
            tx.execute(
                "INSERT INTO catalog(id,origin_instance_id) VALUES(?1,?2)",
                params![
                    catalog.id.to_string(),
                    catalog.origin_instance_id.to_string()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Link a source's GUID explicitly. A source project cannot silently move to
    /// another global project; that will require a separately reviewed workflow.
    pub fn link_project(&mut self, link: ProjectLink) -> Result<(), Error> {
        valid_id(link.project_id)?;
        valid_id(link.catalog_id)?;
        valid_id(link.source_project_guid)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Project, link.project_id)?.is_none()
            || !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM catalog WHERE id=?1)",
                [link.catalog_id.to_string()],
                |row| row.get::<_, bool>(0),
            )?
        {
            return Err(Error::NotFound);
        }
        let old: Option<String> = tx.query_row("SELECT project_id FROM project_catalog WHERE catalog_id=?1 AND source_project_guid=?2",
            params![link.catalog_id.to_string(), link.source_project_guid.to_string()], |row| row.get(0)).optional()?;
        if let Some(project) = old {
            if parse_id(&project)? != link.project_id {
                return Err(Error::Conflict);
            }
        } else {
            tx.execute("INSERT INTO project_catalog(project_id,catalog_id,source_project_guid) VALUES(?1,?2,?3)",
                params![link.project_id.to_string(), link.catalog_id.to_string(), link.source_project_guid.to_string()])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn linked_project(
        &self,
        catalog: Uuid,
        source_project: Uuid,
    ) -> Result<Option<Uuid>, Error> {
        valid_id(catalog)?;
        valid_id(source_project)?;
        let id: Option<String> = self.connection.query_row(
            "SELECT project_id FROM project_catalog WHERE catalog_id=?1 AND source_project_guid=?2",
            params![catalog.to_string(), source_project.to_string()], |row| row.get(0)).optional()?;
        id.map(|value| parse_id(&value)).transpose()
    }

    fn create_named(&mut self, kind: Kind, id: Uuid, name: &str) -> Result<NamedIdentity, Error> {
        valid_id(id)?;
        valid_name(name)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = if let Some(record) = read_named(&tx, kind, id)? {
            if record.name != name {
                return Err(Error::Conflict);
            }
            record
        } else {
            tx.execute(
                &format!(
                    "INSERT INTO {}(id,name,revision) VALUES(?1,?2,1)",
                    kind.table()
                ),
                params![id.to_string(), name],
            )?;
            NamedIdentity {
                id,
                name: name.into(),
                revision: 1,
            }
        };
        tx.commit()?;
        Ok(result)
    }

    fn rename(
        &mut self,
        kind: Kind,
        id: Uuid,
        expected: u64,
        name: &str,
    ) -> Result<NamedIdentity, Error> {
        valid_name(name)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut record = read_named(&tx, kind, id)?.ok_or(Error::NotFound)?;
        if record.revision != expected {
            return Err(Error::Conflict);
        }
        if record.name != name {
            record.revision = record
                .revision
                .checked_add(1)
                .filter(|v| *v <= i64::MAX as u64)
                .ok_or(Error::Conflict)?;
            tx.execute(
                &format!(
                    "UPDATE {} SET name=?1,revision=?2 WHERE id=?3",
                    kind.table()
                ),
                params![
                    name,
                    i64::try_from(record.revision).map_err(|_| Error::Conflict)?,
                    id.to_string()
                ],
            )?;
            record.name = name.into();
        }
        tx.commit()?;
        Ok(record)
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Project,
    Rig,
}
impl Kind {
    fn table(self) -> &'static str {
        match self {
            Self::Project => "global_project",
            Self::Rig => "rig",
        }
    }
}
fn valid_id(id: Uuid) -> Result<(), Error> {
    if id.is_nil() {
        Err(Error::InvalidInput)
    } else {
        Ok(())
    }
}
fn parse_id(value: &str) -> Result<Uuid, Error> {
    let id = Uuid::parse_str(value).map_err(|_| Error::CorruptDatabase)?;
    if id.is_nil() || id.to_string() != value {
        return Err(Error::CorruptDatabase);
    }
    Ok(id)
}
fn valid_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > 512
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        Err(Error::InvalidInput)
    } else {
        Ok(())
    }
}
fn read_named(conn: &Connection, kind: Kind, id: Uuid) -> Result<Option<NamedIdentity>, Error> {
    valid_id(id)?;
    let record: Option<(String, i64)> = conn
        .query_row(
            &format!("SELECT name,revision FROM {} WHERE id=?1", kind.table()),
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    record
        .map(|(name, revision)| {
            valid_name(&name).map_err(|_| Error::CorruptDatabase)?;
            if revision <= 0 {
                return Err(Error::CorruptDatabase);
            }
            let revision = revision as u64;
            Ok(NamedIdentity { id, name, revision })
        })
        .transpose()
}
