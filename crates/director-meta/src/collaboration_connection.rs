//! Non-secret remote identities survive credential loss and disconnection.
use super::*;
use psf_guard_director_interop::astrocollab::Source;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConnectionBinding {
    pub id: Uuid,
    pub rig_id: Uuid,
    pub base_url: String,
    pub name: String,
    pub allow_loopback_http: bool,
    pub agent_id: Option<String>,
    pub state: ConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<psf_guard_director_interop::workflow::Settings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<BackgroundPolicy>,
}

/// Explicit permission to refresh these projects on this rig's catalog.
/// It never enrolls an agent, joins a remote project or starts equipment.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackgroundPolicy {
    pub enabled: bool,
    pub catalog_id: Uuid,
    pub project_ids: Vec<String>,
    pub interval_minutes: u16,
    pub activate: bool,
    #[serde(default, skip_serializing_if = "reports_disabled")]
    pub automatic_reports: bool,
}

fn reports_disabled(value: &bool) -> bool {
    !value
}

impl BackgroundPolicy {
    pub fn validate(&self) -> Result<(), Error> {
        valid_id(self.catalog_id)?;
        if !(5..=1440).contains(&self.interval_minutes)
            || (self.enabled && self.project_ids.is_empty())
            || self.project_ids.len() > 32
            || self
                .project_ids
                .iter()
                .any(|id| !psf_guard_director_interop::workflow::valid_remote_id(id))
            || self
                .project_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.project_ids.len()
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}

pub(crate) fn create_automation_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS collaboration_nightly_run(
        connection_id TEXT PRIMARY KEY NOT NULL REFERENCES collaboration_connection(id),
        night TEXT NOT NULL, completed_at_ms INTEGER NOT NULL);",
    )?;
    validate_automation_table(conn)
}

pub(crate) fn validate_automation_table(conn: &Connection) -> Result<(), Error> {
    conn.prepare(
        "SELECT connection_id,night,completed_at_ms FROM collaboration_nightly_run LIMIT 0",
    )
    .map_err(|_| Error::CorruptDatabase)?;
    Ok(())
}

pub(crate) fn create_pending_night_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS collaboration_nightly_pending(
        connection_id TEXT PRIMARY KEY NOT NULL REFERENCES collaboration_connection(id),
        night TEXT NOT NULL, payload BLOB NOT NULL);",
    )?;
    validate_pending_night_table(conn)
}

pub(crate) fn validate_pending_night_table(conn: &Connection) -> Result<(), Error> {
    conn.prepare("SELECT connection_id,night,payload FROM collaboration_nightly_pending LIMIT 0")
        .map_err(|_| Error::CorruptDatabase)?;
    Ok(())
}

impl MetaStore {
    pub fn pending_collaboration_night(
        &self,
        id: Uuid,
        night: &str,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.connection.query_row(
            "SELECT substr(payload,1,?3) FROM collaboration_nightly_pending WHERE connection_id=?1 AND night=?2",
            params![id.to_string(), night, (psf_guard_director_interop::astrocollab::MAX_BODY_BYTES + 1) as i64],
            |r| r.get::<_, Vec<u8>>(0),
        ).optional()?.map(|bytes| {
            if bytes.len() > psf_guard_director_interop::astrocollab::MAX_BODY_BYTES {
                return Err(Error::CorruptDatabase);
            }
            Ok(bytes)
        }).transpose()
    }

    /// Record the fetched deal and pending local application in one commit.
    pub fn stage_collaboration_night(
        &mut self,
        id: Uuid,
        night: &str,
        now: u64,
        bytes: &[u8],
    ) -> Result<(), Error> {
        let binding = self.collaboration_connection(id)?.ok_or(Error::NotFound)?;
        psf_guard_director_interop::astrocollab::decode_tonight(bytes, &binding.source()?, night)
            .map_err(|_| Error::InvalidInput)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO collaboration_nightly_pending VALUES(?1,?2,?3)
            ON CONFLICT(connection_id) DO UPDATE SET night=excluded.night,payload=excluded.payload",
            params![id.to_string(), night, bytes],
        )?;
        tx.execute("INSERT INTO collaboration_nightly_run VALUES(?1,?2,?3)
            ON CONFLICT(connection_id) DO UPDATE SET night=excluded.night,completed_at_ms=excluded.completed_at_ms",
            params![id.to_string(), night, i64::try_from(now).map_err(|_| Error::InvalidInput)?])?;
        tx.commit()?;
        Ok(())
    }

    pub fn finish_collaboration_night(&mut self, id: Uuid, night: &str) -> Result<(), Error> {
        self.connection.execute(
            "DELETE FROM collaboration_nightly_pending WHERE connection_id=?1 AND night=?2",
            params![id.to_string(), night],
        )?;
        Ok(())
    }

    pub fn collaboration_nightly_run(&self, id: Uuid) -> Result<Option<(String, u64)>, Error> {
        self.connection.query_row("SELECT night,completed_at_ms FROM collaboration_nightly_run WHERE connection_id=?1",
            [id.to_string()], |r| Ok((r.get(0)?, r.get::<_, i64>(1)?))).optional()?
            .map(|(night, at)| Ok((night, u64::try_from(at).map_err(|_| Error::CorruptDatabase)?))).transpose()
    }

    pub fn complete_collaboration_night(
        &mut self,
        id: Uuid,
        night: &str,
        now: u64,
    ) -> Result<(), Error> {
        psf_guard_director_interop::astrocollab::validate_night(night)
            .map_err(|_| Error::InvalidInput)?;
        self.connection.execute("INSERT INTO collaboration_nightly_run VALUES(?1,?2,?3)
            ON CONFLICT(connection_id) DO UPDATE SET night=excluded.night,completed_at_ms=excluded.completed_at_ms",
            params![id.to_string(), night, i64::try_from(now).map_err(|_| Error::InvalidInput)?])?;
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    New,
    OutcomeUnknown,
    Registered,
    Rejected,
    Disabled,
}

pub(crate) fn create_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS collaboration_connection(
        id TEXT PRIMARY KEY NOT NULL, rig_id TEXT NOT NULL REFERENCES rig(id),
        base_url TEXT NOT NULL, agent_id TEXT, payload TEXT NOT NULL,
        UNIQUE(base_url,agent_id));",
    )?;
    validate_table(conn)
}
pub(crate) fn validate_table(conn: &Connection) -> Result<(), Error> {
    conn.prepare(
        "SELECT id,rig_id,base_url,agent_id,payload FROM collaboration_connection LIMIT 0",
    )
    .map_err(|_| Error::CorruptDatabase)?;
    Ok(())
}
impl ConnectionBinding {
    pub fn source(&self) -> Result<Source, Error> {
        Source::new(
            &self.base_url,
            self.agent_id.as_deref().unwrap_or("000000000000"),
            self.allow_loopback_http,
        )
        .map_err(|_| Error::InvalidInput)
    }
    fn validate(&self) -> Result<(), Error> {
        if let Some(background) = &self.background {
            background.validate()?;
        }
        if let Some(settings) = &self.settings {
            settings.validate().map_err(|_| Error::InvalidInput)?;
        }
        valid_id(self.id)?;
        valid_id(self.rig_id)?;
        valid_name(&self.name)?;
        if self.source()?.base_url() != self.base_url {
            return Err(Error::InvalidInput);
        }
        if matches!(
            self.state,
            ConnectionState::Registered | ConnectionState::Rejected
        ) && self.agent_id.is_none()
            || matches!(
                self.state,
                ConnectionState::New | ConnectionState::OutcomeUnknown
            ) && self.agent_id.is_some()
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}
impl MetaStore {
    pub fn collaboration_connection_projects(
        &self,
        id: Uuid,
    ) -> Result<Vec<(String, String, Uuid)>, Error> {
        let binding = self.collaboration_connection(id)?.ok_or(Error::NotFound)?;
        let mut statement = self.connection.prepare("SELECT DISTINCT p.remote_project_id,g.name,g.id FROM collaboration_project p
            JOIN global_project g ON g.id=p.project_id JOIN collaboration_import i ON i.project_id=p.project_id
            WHERE i.base_url=?1 AND i.agent_id=?2 AND i.rig_id=?3 ORDER BY g.name,g.id LIMIT 257")?;
        let rows = statement
            .query_map(
                params![
                    binding.base_url,
                    binding.agent_id,
                    binding.rig_id.to_string()
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if rows.len() > 256 {
            return Err(Error::InvalidInput);
        }
        rows.into_iter()
            .map(|(id, name, project)| Ok((id, name, parse_id(&project)?)))
            .collect()
    }
    pub fn collaboration_connection_ids(&self) -> Result<Vec<Uuid>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM collaboration_connection ORDER BY id LIMIT 257")?;
        let ids = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if ids.len() > 256 {
            return Err(Error::CorruptDatabase);
        }
        ids.iter().map(|id| parse_id(id)).collect()
    }
    pub fn collaboration_imports_for_connection(
        &self,
        id: Uuid,
    ) -> Result<Vec<super::collaboration::Imported>, Error> {
        let binding = self.collaboration_connection(id)?.ok_or(Error::NotFound)?;
        let Some(agent) = binding.agent_id else {
            return Ok(vec![]);
        };
        let mut statement=self.connection.prepare("SELECT id FROM collaboration_import WHERE base_url=?1 AND agent_id=?2 AND rig_id=?3 ORDER BY imported_at_ms DESC LIMIT 257")?;
        let ids = statement
            .query_map(
                params![binding.base_url, agent, binding.rig_id.to_string()],
                |r| r.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if ids.len() > 256 {
            return Err(Error::InvalidInput);
        }
        ids.into_iter()
            .map(|id| {
                self.collaboration_import(parse_id(&id)?)?
                    .ok_or(Error::CorruptDatabase)
            })
            .collect()
    }
    pub fn create_collaboration_connection(
        &mut self,
        binding: &ConnectionBinding,
    ) -> Result<(), Error> {
        binding.validate()?;
        if binding.agent_id.is_some() || binding.state != ConnectionState::New {
            return Err(Error::InvalidInput);
        }
        if self.rig(binding.rig_id)?.is_none() {
            return Err(Error::NotFound);
        }
        if let Some(existing) = self.collaboration_connection(binding.id)? {
            return if existing == *binding {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        let total: i64 = self.connection.query_row(
            "SELECT count(*) FROM collaboration_connection",
            [],
            |r| r.get(0),
        )?;
        if total >= 256 {
            return Err(Error::InvalidInput);
        }
        self.connection.execute(
            "INSERT INTO collaboration_connection VALUES(?1,?2,?3,NULL,?4)",
            params![
                binding.id.to_string(),
                binding.rig_id.to_string(),
                binding.base_url,
                configuration::encode(binding)?
            ],
        )?;
        Ok(())
    }
    pub fn collaboration_connection(&self, id: Uuid) -> Result<Option<ConnectionBinding>, Error> {
        let row: Option<(String, String, Option<String>, String)> = self
            .connection
            .query_row(
                "SELECT rig_id,base_url,agent_id,substr(payload,1,16385) FROM collaboration_connection WHERE id=?1",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        row.map(|(rig, base, agent, bytes)| {
            if bytes.len() > 16384 {
                return Err(Error::CorruptDatabase);
            }
            let binding: ConnectionBinding = configuration::decode(bytes.into_bytes())?;
            binding.validate().map_err(|_| Error::CorruptDatabase)?;
            if binding.id != id
                || binding.rig_id.to_string() != rig
                || binding.base_url != base
                || binding.agent_id != agent
            {
                return Err(Error::CorruptDatabase);
            }
            Ok(binding)
        })
        .transpose()
    }
    pub fn collaboration_connections(&self, rig: Uuid) -> Result<Vec<ConnectionBinding>, Error> {
        let mut stmt = self.connection.prepare(
            "SELECT id FROM collaboration_connection WHERE rig_id=?1 ORDER BY id LIMIT 257",
        )?;
        let ids = stmt
            .query_map([rig.to_string()], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if ids.len() > 256 {
            return Err(Error::CorruptDatabase);
        }
        ids.into_iter()
            .map(|id| {
                self.collaboration_connection(parse_id(&id)?)?
                    .ok_or(Error::CorruptDatabase)
            })
            .collect()
    }
    pub fn update_collaboration_connection(
        &mut self,
        old: &ConnectionBinding,
        new: &ConnectionBinding,
    ) -> Result<(), Error> {
        new.validate()?;
        if old.id != new.id
            || old.rig_id != new.rig_id
            || old.base_url != new.base_url
            || old.name != new.name
            || old.allow_loopback_http != new.allow_loopback_http
            || (old.agent_id.is_some() && old.agent_id != new.agent_id)
        {
            return Err(Error::Conflict);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE collaboration_connection SET agent_id=?2,payload=?3 WHERE id=?1 AND payload=?4",
            params![
                old.id.to_string(),
                new.agent_id,
                configuration::encode(new)?,
                configuration::encode(old)?
            ],
        )?;
        if changed != 1 {
            return Err(Error::Conflict);
        }
        if old.background != new.background {
            tx.execute(
                "DELETE FROM collaboration_nightly_run WHERE connection_id=?1",
                [old.id.to_string()],
            )?;
            tx.execute(
                "DELETE FROM collaboration_nightly_pending WHERE connection_id=?1",
                [old.id.to_string()],
            )?;
        }
        if let Some(agent) = &new.agent_id {
            let foreign: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM collaboration_import WHERE base_url=?1 AND agent_id=?2 AND rig_id!=?3)",
                params![new.base_url, agent, new.rig_id.to_string()], |r| r.get(0))?;
            if foreign {
                return Err(Error::Conflict);
            }
        }
        tx.commit()?;
        Ok(())
    }
}
