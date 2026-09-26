use super::*;
use rusqlite::backup::{Backup, StepResult};
use tempfile::NamedTempFile;

const APPLICATION_ID: i32 = 0x50474d44;
const SCHEMA_VERSION: i32 = 3;

impl MetaStore {
    /// Publish a complete database at a new path. Never adopt an existing empty
    /// file, TS catalog, or another store. The parent directory must exist.
    pub fn create(path: &Path) -> Result<Self, Error> {
        let file = staging(path)?;
        let mut conn = Connection::open(file.path())?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TABLE meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1), instance_id TEXT NOT NULL);
             CREATE TABLE global_project(id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0));
             CREATE TABLE rig(id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0));
             CREATE TABLE catalog(id TEXT PRIMARY KEY NOT NULL, origin_instance_id TEXT NOT NULL);
             CREATE TABLE project_catalog(
                catalog_id TEXT NOT NULL REFERENCES catalog(id),
                source_project_guid TEXT NOT NULL,
                project_id TEXT NOT NULL REFERENCES global_project(id),
                PRIMARY KEY(catalog_id,source_project_guid));
             CREATE INDEX project_catalog_project ON project_catalog(project_id);"
        )?;
        tx.execute(
            "INSERT INTO meta VALUES(1,?1)",
            [Uuid::new_v4().to_string()],
        )?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
        create_configuration_tables(&tx)?;
        create_project_tables(&tx)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        conn.close().map_err(|(_, error)| Error::Sqlite(error))?;
        publish(file, path)?;
        Self::open(path)
    }

    /// Open only a recognized existing store. Future schema versions are not
    /// rewritten or opened with downgraded semantics.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let mut conn = connect(path, false)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let instance_id = validate(&tx)?;
        let version: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < SCHEMA_VERSION {
            if tx.prepare("PRAGMA foreign_key_check")?.exists([])? {
                return Err(Error::CorruptDatabase);
            }
            if version == 1 {
                create_configuration_tables(&tx)?;
            }
            create_project_tables(&tx)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        tx.commit()?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        if mode != "wal" {
            return Err(Error::InvalidInput);
        }
        conn.pragma_update(None, "synchronous", "FULL")?;
        Ok(Self {
            connection: conn,
            instance_id,
        })
    }

    /// Consistent snapshot, including committed WAL pages. Never copy the live
    /// SQLite file directly. The destination must be new, even for a retry.
    pub fn backup(&self, destination: &Path) -> Result<(), Error> {
        snapshot(&self.connection, destination)
    }

    /// Restore to a new path only; replacing a live coordinator is not supported.
    /// Preserve instance identity, so this copy must not run beside its source as
    /// an independent coordinator. Catalog files are not part of this backup.
    pub fn restore(source: &Path, destination: &Path) -> Result<(), Error> {
        let conn = connect(source, true)?;
        snapshot(&conn, destination)
    }
}

fn connect(path: &Path, read_only: bool) -> Result<Connection, Error> {
    if path.as_os_str().is_empty() || path == Path::new(":memory:") {
        return Err(Error::InvalidInput);
    }
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let conn = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

fn validate(conn: &Connection) -> Result<Uuid, Error> {
    let app: i32 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if app != APPLICATION_ID {
        return Err(Error::ForeignDatabase);
    }
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Err(Error::UnsupportedSchema);
    }
    for sql in [
        "SELECT id,name,revision FROM global_project LIMIT 0",
        "SELECT id,name,revision FROM rig LIMIT 0",
        "SELECT id,origin_instance_id FROM catalog LIMIT 0",
        "SELECT project_id,catalog_id,source_project_guid FROM project_catalog LIMIT 0",
    ] {
        conn.prepare(sql).map_err(|_| Error::CorruptDatabase)?;
    }
    if version >= 2 {
        for sql in [
            "SELECT id,name,revision FROM site LIMIT 0",
            "SELECT id,site_id,payload FROM site_snapshot LIMIT 0",
            "SELECT id,rig_id,site_snapshot_id,payload FROM rig_setup LIMIT 0",
        ] {
            conn.prepare(sql).map_err(|_| Error::CorruptDatabase)?;
        }
    }
    if version >= 3 {
        for sql in [
            "SELECT id,project_id,payload FROM project_intent LIMIT 0",
            "SELECT intent_id,setup_id FROM project_intent_setup LIMIT 0",
        ] {
            conn.prepare(sql).map_err(|_| Error::CorruptDatabase)?;
        }
    }
    let id: String = conn.query_row(
        "SELECT instance_id FROM meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    parse_id(&id)
}

fn create_configuration_tables(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE site(id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0));
         CREATE TABLE site_snapshot(id TEXT PRIMARY KEY NOT NULL, site_id TEXT NOT NULL REFERENCES site(id), payload TEXT NOT NULL);
         CREATE INDEX site_snapshot_site ON site_snapshot(site_id,id);
         CREATE TABLE rig_setup(id TEXT PRIMARY KEY NOT NULL, rig_id TEXT NOT NULL REFERENCES rig(id), site_snapshot_id TEXT NOT NULL REFERENCES site_snapshot(id), payload TEXT NOT NULL);
         CREATE INDEX rig_setup_rig ON rig_setup(rig_id,id);"
    )?;
    Ok(())
}

fn create_project_tables(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE project_intent(id TEXT PRIMARY KEY NOT NULL, project_id TEXT NOT NULL REFERENCES global_project(id), payload TEXT NOT NULL);
         CREATE INDEX project_intent_project ON project_intent(project_id,id);
         CREATE TABLE project_intent_setup(intent_id TEXT NOT NULL REFERENCES project_intent(id), setup_id TEXT NOT NULL REFERENCES rig_setup(id), PRIMARY KEY(intent_id,setup_id));"
    )?;
    Ok(())
}

fn staging(destination: &Path) -> Result<NamedTempFile, Error> {
    if destination.as_os_str().is_empty()
        || destination == Path::new(":memory:")
        || destination.file_name().is_none()
    {
        return Err(Error::InvalidInput);
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(NamedTempFile::new_in(parent)?)
}
fn publish(file: NamedTempFile, path: &Path) -> Result<(), Error> {
    file.as_file().sync_all()?;
    file.persist_noclobber(path)
        .map_err(|error| Error::Io(error.error))?;
    Ok(())
}
fn snapshot(source: &Connection, destination: &Path) -> Result<(), Error> {
    let file = staging(destination)?;
    let mut target = Connection::open(file.path())?;
    // Pin the source snapshot before copying. Do not retry Busy indefinitely or
    // chase an active writer by restarting the backup on each page batch.
    let tx = source.unchecked_transaction()?;
    validate(&tx)?;
    let backup = Backup::new(&tx, &mut target)?;
    match backup.step(-1)? {
        StepResult::Done => {}
        _ => return Err(Error::Conflict),
    }
    drop(backup);
    tx.commit()?;
    validate(&target)?;
    let check: String = target.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if check != "ok" {
        return Err(Error::CorruptDatabase);
    }
    if target.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(Error::CorruptDatabase);
    }
    target.pragma_update(None, "journal_mode", "DELETE")?;
    target.close().map_err(|(_, error)| Error::Sqlite(error))?;
    publish(file, destination)
}
