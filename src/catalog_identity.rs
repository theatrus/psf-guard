//! Explicit identity adoption for a PSF Guard-managed catalog destination.
//! Ordinary opens, discovery and source-side TS sync must never call adoption.

use psf_guard_director_meta::CatalogIdentity;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::fmt;
use uuid::Uuid;

#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    InvalidIdentity,
    InvalidRecord,
    UnsupportedVersion,
    Conflict,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(_) => f.write_str("Catalog identity database operation failed"),
            Self::InvalidIdentity => f.write_str("Catalog identity must contain non-nil UUIDs"),
            Self::InvalidRecord => f.write_str("Invalid catalog identity record"),
            Self::UnsupportedVersion => f.write_str("Unsupported catalog identity version"),
            Self::Conflict => f.write_str("Catalog already has a different identity"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// Read without creating or repairing anything. Missing means not adopted.
/// Call inside the host's read transaction when combining this with discovery.
pub fn read(connection: &Connection) -> Result<Option<CatalogIdentity>, Error> {
    let kind: Option<String> = connection.query_row(
        "SELECT type FROM main.sqlite_schema WHERE name='psf_guard_catalog_identity' COLLATE NOCASE",
        [], |row| row.get(0),
    ).optional()?;
    match kind.as_deref() {
        None => return Ok(None),
        Some("table") => {}
        _ => return Err(Error::InvalidRecord),
    }
    // Qualify main explicitly: a temporary table must not shadow catalog identity.
    let mut statement = connection.prepare(
        "SELECT singleton,format,schema_version,catalog_id,origin_instance_id
         FROM main.psf_guard_catalog_identity LIMIT 2",
    )?;
    let mut rows = statement.query([])?;
    let row = rows.next()?.ok_or(Error::InvalidRecord)?;
    if row.get::<_, i64>(0).map_err(|_| Error::InvalidRecord)? != 1
        || row.get::<_, String>(1).map_err(|_| Error::InvalidRecord)? != "psf-guard-catalog"
    {
        return Err(Error::InvalidRecord);
    }
    if row.get::<_, i64>(2).map_err(|_| Error::InvalidRecord)? != 1 {
        return Err(Error::UnsupportedVersion);
    }
    let parse = |index| -> Result<Uuid, Error> {
        let text: String = row.get(index).map_err(|_| Error::InvalidRecord)?;
        let id = Uuid::parse_str(&text).map_err(|_| Error::InvalidRecord)?;
        if id.is_nil() || id.to_string() != text {
            return Err(Error::InvalidRecord);
        }
        Ok(id)
    };
    let identity = CatalogIdentity {
        id: parse(3)?,
        origin_instance_id: parse(4)?,
    };
    if rows.next()?.is_some() {
        return Err(Error::InvalidRecord);
    }
    Ok(Some(identity))
}

/// Adopt only after explicit operator confirmation of destination and preview.
/// The caller must validate source evidence in this same transaction, preferably
/// IMMEDIATE. Commit the outer transaction to make adoption durable. Reuse the
/// exact proposed IDs on retry; neither origin nor identity can be reassigned.
/// A savepoint ensures an error leaves no partial table even if the host commits.
pub fn adopt(
    transaction: &mut Transaction<'_>,
    proposed: CatalogIdentity,
) -> Result<CatalogIdentity, Error> {
    if proposed.id.is_nil() || proposed.origin_instance_id.is_nil() {
        return Err(Error::InvalidIdentity);
    }
    let savepoint = transaction.savepoint()?;
    if let Some(existing) = read(&savepoint)? {
        return if existing == proposed {
            savepoint.commit()?;
            Ok(existing)
        } else {
            Err(Error::Conflict)
        };
    }
    savepoint.execute_batch(
        "CREATE TABLE main.psf_guard_catalog_identity (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            format TEXT NOT NULL CHECK(format='psf-guard-catalog'),
            schema_version INTEGER NOT NULL CHECK(schema_version>=1),
            catalog_id TEXT NOT NULL,
            origin_instance_id TEXT NOT NULL
        ) STRICT;",
    )?;
    savepoint.execute(
        "INSERT INTO main.psf_guard_catalog_identity VALUES(1,'psf-guard-catalog',1,?1,?2)",
        params![
            proposed.id.to_string(),
            proposed.origin_instance_id.to_string()
        ],
    )?;
    savepoint.commit()?;
    Ok(proposed)
}

#[cfg(test)]
mod tests;
