//! Durable file delivery, separate from scientific contribution credit.
use super::*;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct Destination {
    pub base_url: String,
    pub project: String,
    pub owner: String,
}

#[derive(Clone, Debug)]
pub struct FileDelivery {
    pub destination: Destination,
    pub name: String,
    pub kind: String,
    pub hash: String,
    pub bytes: u64,
}

pub(crate) fn create_delivery_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS collaboration_file_delivery(base_url TEXT NOT NULL,project TEXT NOT NULL,owner TEXT NOT NULL,kind TEXT NOT NULL,sha256 TEXT NOT NULL,bytes INTEGER NOT NULL CHECK(bytes>=0),PRIMARY KEY(base_url,project,owner,kind,sha256));")?;
    validate_delivery_table(conn)
}

pub(crate) fn validate_delivery_table(conn: &Connection) -> Result<(), Error> {
    conn.prepare(
        "SELECT base_url,project,owner,kind,sha256,bytes FROM collaboration_file_delivery LIMIT 0",
    )
    .map_err(|_| Error::CorruptDatabase)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Preparing,
    Review,
    Queued,
    Uploading,
    Paused,
    Retry,
    NeedsReview,
    Complete,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub connection_id: Uuid,
    pub revision: u64,
    pub state: State,
    pub request: Value,
    pub package: Option<Value>,
    pub completed: Vec<String>,
    pub error: Option<String>,
    pub attempts: u32,
    pub next_attempt_ms: u64,
    pub created_at_ms: u64,
}

pub(crate) fn create_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS collaboration_transfer (
        id TEXT PRIMARY KEY NOT NULL,
        connection_id TEXT NOT NULL REFERENCES collaboration_connection(id),
        revision INTEGER NOT NULL CHECK(revision>0), payload TEXT NOT NULL);",
    )?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS collaboration_transfer_policy(connection_id TEXT PRIMARY KEY NOT NULL REFERENCES collaboration_connection(id),payload TEXT NOT NULL);")?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS collaboration_transfer_definition(id TEXT PRIMARY KEY NOT NULL REFERENCES collaboration_transfer(id),request TEXT NOT NULL,package TEXT);
        CREATE TABLE IF NOT EXISTS collaboration_transfer_artifact(id TEXT NOT NULL REFERENCES collaboration_transfer(id),name TEXT NOT NULL,bytes INTEGER NOT NULL,PRIMARY KEY(id,name));
        CREATE TABLE IF NOT EXISTS collaboration_transfer_receipt(id TEXT NOT NULL REFERENCES collaboration_transfer(id),name TEXT NOT NULL,bytes INTEGER NOT NULL,PRIMARY KEY(id,name));
        CREATE TABLE IF NOT EXISTS collaboration_transfer_summary(id TEXT PRIMARY KEY NOT NULL REFERENCES collaboration_transfer(id),payload TEXT NOT NULL);")?;
    validate_tables(conn)
}

pub(crate) fn validate_tables(conn: &Connection) -> Result<(), Error> {
    for sql in [
        "SELECT id,connection_id,revision,payload FROM collaboration_transfer LIMIT 0",
        "SELECT connection_id,payload FROM collaboration_transfer_policy LIMIT 0",
        "SELECT id,request,package FROM collaboration_transfer_definition LIMIT 0",
        "SELECT id,name,bytes FROM collaboration_transfer_artifact LIMIT 0",
        "SELECT id,name,bytes FROM collaboration_transfer_receipt LIMIT 0",
        "SELECT id,payload FROM collaboration_transfer_summary LIMIT 0",
    ] {
        conn.prepare(sql).map_err(|_| Error::CorruptDatabase)?;
    }
    Ok(())
}

impl MetaStore {
    pub fn transfer_review_required(
        &self,
        connection: Uuid,
        import: Uuid,
        panel: u32,
        images: &[Uuid],
    ) -> Result<bool, Error> {
        let images = serde_json::to_string(images).map_err(|_| Error::InvalidInput)?;
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM collaboration_transfer t JOIN collaboration_transfer_definition d ON d.id=t.id, json_each(d.request,'$.selection.image_guids') old WHERE t.connection_id=?1 AND json_extract(d.request,'$.selection.import_id')=?2 AND json_extract(d.request,'$.selection.panel')=?3 AND json_extract(t.payload,'$.state') IN ('needs_review','review') AND old.value IN (SELECT value FROM json_each(?4)))",params![connection.to_string(),import.to_string(),panel,images],|r|r.get(0))?)
    }

    /// Completed source stamps avoid rehashing old frames. `None` reserves a
    /// frame in unfinished or cancelled work, irrespective of later revisions.
    pub fn raw_transfer_sources(
        &self,
        connection: Uuid,
        import: Uuid,
        panel: u32,
    ) -> Result<BTreeMap<Uuid, BTreeSet<Option<String>>>, Error> {
        let mut stmt = self.connection.prepare("SELECT image.value,CASE WHEN json_extract(t.payload,'$.state')='complete' THEN COALESCE(json_extract(d.request,'$.source_versions.' || image.value),'') ELSE NULL END FROM collaboration_transfer t JOIN collaboration_transfer_definition d ON d.id=t.id, json_each(d.request,'$.selection.image_guids') image WHERE t.connection_id=?1 AND json_extract(d.request,'$.selection.import_id')=?2 AND json_extract(d.request,'$.selection.panel')=?3 AND json_extract(d.request,'$.output') IS NULL")?;
        let images = stmt
            .query_map(
                params![connection.to_string(), import.to_string(), panel],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut sources = BTreeMap::<_, BTreeSet<_>>::new();
        for (id, version) in images {
            sources
                .entry(Uuid::parse_str(&id).map_err(|_| Error::CorruptDatabase)?)
                .or_default()
                .insert(version);
        }
        Ok(sources)
    }

    pub fn delivered_files(
        &self,
        destination: &Destination,
    ) -> Result<BTreeSet<(String, String, u64)>, Error> {
        let mut stmt = self.connection.prepare("SELECT kind,sha256,bytes FROM collaboration_file_delivery WHERE base_url=?1 AND project=?2 AND owner=?3")?;
        let files = stmt
            .query_map(
                params![destination.base_url, destination.project, destination.owner],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        files
            .into_iter()
            .map(|(kind, hash, bytes)| {
                Ok((
                    kind,
                    hash,
                    u64::try_from(bytes).map_err(|_| Error::CorruptDatabase)?,
                ))
            })
            .collect()
    }

    pub fn transfer_activity(&self) -> Result<Vec<Value>, Error> {
        let mut stmt=self.connection.prepare("SELECT s.payload FROM collaboration_transfer_summary s JOIN collaboration_transfer t ON t.id=s.id WHERE json_extract(t.payload,'$.state') NOT IN ('complete','cancelled') OR t.rowid IN (SELECT rowid FROM collaboration_transfer ORDER BY rowid DESC LIMIT 50) ORDER BY t.rowid")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|p| serde_json::from_str(&p).map_err(|_| Error::CorruptDatabase))
            .collect()
    }
    pub fn transfer_revision(&self, id: Uuid) -> Result<Option<u64>, Error> {
        let value = self
            .connection
            .query_row(
                "SELECT revision FROM collaboration_transfer WHERE id=?1",
                [id.to_string()],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        value
            .map(|n| u64::try_from(n).map_err(|_| Error::CorruptDatabase))
            .transpose()
    }
    pub fn next_transfer(&self, now: u64) -> Result<Option<Job>, Error> {
        let id=self.connection.query_row("SELECT id FROM collaboration_transfer WHERE json_extract(payload,'$.state') IN ('preparing','queued') OR (json_extract(payload,'$.state')='retry' AND json_extract(payload,'$.next_attempt_ms')<=?1) ORDER BY rowid LIMIT 1",[i64::try_from(now).map_err(|_|Error::InvalidInput)?],|r|r.get::<_,String>(0)).optional()?;
        id.map(|id| {
            self.transfer(Uuid::parse_str(&id).map_err(|_| Error::CorruptDatabase)?)?
                .ok_or(Error::CorruptDatabase)
        })
        .transpose()
    }
    pub fn transfer_policies(&self) -> Result<Vec<Value>, Error> {
        let mut stmt = self
            .connection
            .prepare("SELECT payload FROM collaboration_transfer_policy ORDER BY connection_id")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|p| serde_json::from_str(&p).map_err(|_| Error::CorruptDatabase))
            .collect()
    }
    pub fn set_transfer_policy(
        &mut self,
        connection: Uuid,
        catalog: Uuid,
        automatic: bool,
        mode: &str,
    ) -> Result<(), Error> {
        if self.collaboration_connection(connection)?.is_none()
            || !matches!(mode, "raw" | "psf_guard")
        {
            return Err(Error::InvalidInput);
        }
        let payload = serde_json::json!({"connection_id":connection,"catalog_id":catalog,"automatic":automatic,"mode":mode});
        self.connection.execute("INSERT INTO collaboration_transfer_policy VALUES(?1,?2) ON CONFLICT(connection_id) DO UPDATE SET payload=excluded.payload",
            params![connection.to_string(),payload.to_string()])?;
        Ok(())
    }
    pub fn create_transfer(
        &mut self,
        connection: Uuid,
        request: Value,
        now: u64,
    ) -> Result<Job, Error> {
        if self.collaboration_connection(connection)?.is_none() || now > i64::MAX as u64 {
            return Err(Error::InvalidInput);
        }
        let encoded = serde_json::to_vec(&request).map_err(|_| Error::InvalidInput)?;
        if encoded.len() > 1_048_576 {
            return Err(Error::InvalidInput);
        }
        let id = Uuid::new_v5(&connection, &encoded);
        if let Some(existing) = self.transfer(id)? {
            return Ok(existing);
        }
        let active:i64=self.connection.query_row("SELECT COUNT(*) FROM collaboration_transfer WHERE json_extract(payload,'$.state') NOT IN ('complete','cancelled')",[],|r|r.get(0))?;
        if active >= 512 {
            return Err(Error::Conflict);
        }
        let job = Job {
            id,
            connection_id: connection,
            revision: 1,
            state: State::Preparing,
            request,
            package: None,
            completed: Vec::new(),
            error: None,
            attempts: 0,
            next_attempt_ms: 0,
            created_at_ms: now,
        };
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO collaboration_transfer VALUES(?1,?2,1,?3)",
            params![id.to_string(), connection.to_string(), checkpoint(&job)?],
        )?;
        write_definition(&tx, &job)?;
        write_summary(&tx, &job)?;
        tx.commit()?;
        Ok(job)
    }

    pub fn transfer(&self, id: Uuid) -> Result<Option<Job>, Error> {
        let row = self
            .connection
            .query_row(
                "SELECT connection_id,revision,payload FROM collaboration_transfer WHERE id=?1",
                [id.to_string()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(connection, revision, payload)| {
            let mut job: Job =
                serde_json::from_str(&payload).map_err(|_| Error::CorruptDatabase)?;
            if job.id != id
                || job.connection_id.to_string() != connection
                || i64::try_from(job.revision).ok() != Some(revision)
            {
                return Err(Error::CorruptDatabase);
            }
            let (request, package) = self.connection.query_row(
                "SELECT request,package FROM collaboration_transfer_definition WHERE id=?1",
                [id.to_string()],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )?;
            job.request = serde_json::from_str(&request).map_err(|_| Error::CorruptDatabase)?;
            job.package = package
                .map(|p| serde_json::from_str(&p).map_err(|_| Error::CorruptDatabase))
                .transpose()?;
            let mut receipts = self.connection.prepare(
                "SELECT name FROM collaboration_transfer_receipt WHERE id=?1 ORDER BY name",
            )?;
            job.completed = receipts
                .query_map([id.to_string()], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(job)
        })
        .transpose()
    }

    pub fn transfers(&self) -> Result<Vec<Job>, Error> {
        let mut stmt = self
            .connection
            .prepare("SELECT id FROM collaboration_transfer WHERE json_extract(payload,'$.state') NOT IN ('complete','cancelled') ORDER BY rowid LIMIT 512")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|id| {
                self.transfer(Uuid::parse_str(&id).map_err(|_| Error::CorruptDatabase)?)?
                    .ok_or(Error::CorruptDatabase)
            })
            .collect()
    }

    /// Compare-and-swap keeps a worker from undoing Pause or Cancel in flight.
    pub fn save_transfer(&mut self, mut job: Job) -> Result<Job, Error> {
        let expected = job.revision;
        job.revision = expected.checked_add(1).ok_or(Error::InvalidInput)?;
        let revision = i64::try_from(job.revision).map_err(|_| Error::InvalidInput)?;
        let expected = i64::try_from(expected).map_err(|_| Error::InvalidInput)?;
        let payload = checkpoint(&job)?;
        if payload.len() > 16 * 1024 * 1024 {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE collaboration_transfer SET revision=?2,payload=?3 WHERE id=?1 AND revision=?4 AND connection_id=?5",
            params![job.id.to_string(),revision,payload,expected,job.connection_id.to_string()])?;
        if changed != 1 {
            return Err(Error::Conflict);
        }
        write_definition(&tx, &job)?;
        let retained = serde_json::to_string(&job.completed).map_err(|_| Error::InvalidInput)?;
        tx.execute("DELETE FROM collaboration_transfer_receipt WHERE id=?1 AND name NOT IN (SELECT value FROM json_each(?2))",params![job.id.to_string(),retained])?;
        // Normal checkpoints never add receipts; this supports importing a
        // prepared checkpoint while acknowledgments use the bounded method.
        for name in &job.completed {
            tx.execute(
                "INSERT OR IGNORE INTO collaboration_transfer_receipt VALUES(?1,?2,0)",
                params![job.id.to_string(), name],
            )?;
        }
        write_summary(&tx, &job)?;
        tx.commit()?;
        Ok(job)
    }

    pub fn acknowledge_transfer_file(
        &mut self,
        id: Uuid,
        revision: u64,
        name: &str,
        bytes: u64,
    ) -> Result<u64, Error> {
        self.acknowledge_transfer_file_inner(id, revision, name, bytes, None)
    }

    pub fn acknowledge_transfer_delivery(
        &mut self,
        id: Uuid,
        revision: u64,
        delivery: &FileDelivery,
    ) -> Result<u64, Error> {
        if delivery.hash.len() != 64
            || !delivery.hash.bytes().all(|b| b.is_ascii_hexdigit())
            || delivery.destination.base_url.is_empty()
            || delivery.destination.project.is_empty()
            || delivery.destination.owner.is_empty()
            || delivery.kind.is_empty()
        {
            return Err(Error::InvalidInput);
        }
        self.acknowledge_transfer_file_inner(
            id,
            revision,
            &delivery.name,
            delivery.bytes,
            Some(delivery),
        )
    }

    fn acknowledge_transfer_file_inner(
        &mut self,
        id: Uuid,
        revision: u64,
        name: &str,
        bytes: u64,
        delivery: Option<&FileDelivery>,
    ) -> Result<u64, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (current, payload): (i64, String) = tx.query_row(
            "SELECT revision,payload FROM collaboration_transfer WHERE id=?1",
            [id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if u64::try_from(current).ok() != Some(revision) {
            return Err(Error::Conflict);
        }
        let mut job: Job = serde_json::from_str(&payload).map_err(|_| Error::CorruptDatabase)?;
        if job.state != State::Uploading {
            return Err(Error::Conflict);
        }
        let belongs:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM collaboration_transfer_artifact WHERE id=?1 AND name=?2 AND bytes=?3)",params![id.to_string(),name,i64::try_from(bytes).map_err(|_|Error::InvalidInput)?],|r|r.get(0))?;
        if !belongs {
            return Err(Error::InvalidInput);
        }
        if let Some(delivery) = delivery {
            tx.execute("INSERT INTO collaboration_file_delivery VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(base_url,project,owner,kind,sha256) DO UPDATE SET bytes=excluded.bytes",
                params![delivery.destination.base_url,delivery.destination.project,delivery.destination.owner,delivery.kind,delivery.hash,bytes as i64])?;
        }
        let added = tx.execute(
            "INSERT OR IGNORE INTO collaboration_transfer_receipt VALUES(?1,?2,?3)",
            params![id.to_string(), name, bytes as i64],
        )?;
        if added == 0 {
            tx.commit()?;
            return Ok(revision);
        }
        job.revision = revision
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or(Error::InvalidInput)?;
        tx.execute(
            "UPDATE collaboration_transfer SET revision=?2,payload=?3 WHERE id=?1",
            params![id.to_string(), job.revision as i64, checkpoint(&job)?],
        )?;
        tx.execute("UPDATE collaboration_transfer_summary SET payload=json_set(payload,'$.revision',?2,'$.completed_files',json_extract(payload,'$.completed_files')+1,'$.completed_bytes',json_extract(payload,'$.completed_bytes')+?3) WHERE id=?1",params![id.to_string(),job.revision as i64,bytes as i64])?;
        tx.commit()?;
        Ok(job.revision)
    }

    pub fn control_transfer(
        &mut self,
        id: Uuid,
        revision: u64,
        action: &str,
    ) -> Result<Job, Error> {
        let mut job = self.transfer(id)?.ok_or(Error::NotFound)?;
        if job.revision != revision {
            return Err(Error::Conflict);
        }
        job.state = match (action, job.state) {
            ("approve", State::Review) => State::Queued,
            ("pause", State::Preparing | State::Queued | State::Uploading | State::Retry) => {
                State::Paused
            }
            ("resume", State::Paused | State::Retry) => {
                if job.package.is_some() {
                    State::Queued
                } else {
                    State::Preparing
                }
            }
            ("retry", State::NeedsReview | State::Cancelled) => {
                job.package = None;
                // Re-preparation can change the remote owner. Rebuild receipts
                // from that destination's listing rather than trusting old ones.
                job.completed.clear();
                if let Some(request) = job.request.as_object_mut() {
                    request.insert("automatic".into(), Value::Bool(false));
                    request.insert("upload_pending".into(), Value::Bool(false));
                    request.remove("source_versions");
                }
                State::Preparing
            }
            ("cancel", state) if state != State::Complete => State::Cancelled,
            _ => return Err(Error::Conflict),
        };
        job.error = None;
        job.next_attempt_ms = 0;
        self.save_transfer(job)
    }

    /// Run once at host startup, never when opening another reader connection.
    pub fn recover_transfers(&mut self) -> Result<(), Error> {
        for mut job in self.transfers()? {
            if job.state == State::Uploading {
                job.state = State::Queued;
                job.error = Some("Interrupted transfer; completed files retained".into());
                self.save_transfer(job)?;
            }
        }
        Ok(())
    }
}

fn checkpoint(job: &Job) -> Result<String, Error> {
    let mut state = job.clone();
    state.request = Value::Null;
    state.package = None;
    state.completed.clear();
    serde_json::to_string(&state).map_err(|_| Error::InvalidInput)
}
fn write_definition(conn: &Connection, job: &Job) -> Result<(), Error> {
    let request = serde_json::to_string(&job.request).map_err(|_| Error::InvalidInput)?;
    let package = job
        .package
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| Error::InvalidInput)?;
    if package.as_ref().is_some_and(|p| p.len() > 16 * 1024 * 1024) {
        return Err(Error::InvalidInput);
    }
    let changed=conn.execute("INSERT INTO collaboration_transfer_definition VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET request=excluded.request,package=excluded.package WHERE request!=excluded.request OR package IS NOT excluded.package",params![job.id.to_string(),request,package])?;
    if changed > 0 {
        conn.execute(
            "DELETE FROM collaboration_transfer_artifact WHERE id=?1",
            [job.id.to_string()],
        )?;
        if let Some(artifacts) = job.package.as_ref().and_then(|p| p["artifacts"].as_array()) {
            let mut insert =
                conn.prepare("INSERT INTO collaboration_transfer_artifact VALUES(?1,?2,?3)")?;
            for artifact in artifacts {
                let name = artifact["name"].as_str().ok_or(Error::InvalidInput)?;
                let bytes = artifact["bytes"]
                    .as_u64()
                    .and_then(|n| i64::try_from(n).ok())
                    .ok_or(Error::InvalidInput)?;
                insert.execute(params![job.id.to_string(), name, bytes])?;
            }
        }
    }
    Ok(())
}
fn write_summary(conn: &Connection, job: &Job) -> Result<(), Error> {
    let artifacts = job.package.as_ref().and_then(|p| p["artifacts"].as_array());
    let complete = job
        .completed
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let total = artifacts.map_or(0, |all| {
        all.iter().filter_map(|a| a["bytes"].as_u64()).sum::<u64>()
    });
    let done = artifacts.map_or(0, |all| {
        all.iter()
            .filter(|a| a["name"].as_str().is_some_and(|n| complete.contains(n)))
            .filter_map(|a| a["bytes"].as_u64())
            .sum::<u64>()
    });
    let summary = serde_json::json!({"id":job.id,"connection_id":job.connection_id,"revision":job.revision,"state":job.state,"catalog":job.request["selection"]["catalog"],"label":job.package.as_ref().and_then(|p|p["label"].as_str()).unwrap_or("Preparing contribution files"),"total_files":artifacts.map_or(0,|a|a.len()),"completed_files":job.completed.len(),"total_bytes":total,"completed_bytes":done,"current_file":"","sent_bytes":0,"error":job.error,"next_attempt_ms":job.next_attempt_ms,"warnings":job.package.as_ref().map(|p|&p["warnings"]),"manifest":null});
    conn.execute("INSERT INTO collaboration_transfer_summary VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![job.id.to_string(),summary.to_string()])?;
    Ok(())
}
