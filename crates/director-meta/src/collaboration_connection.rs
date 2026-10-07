//! Non-secret remote identities survive credential loss and disconnection.
use super::*;
use psf_guard_director_interop::astrocollab::Source;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConnectionBinding {
    pub id: Uuid,
    pub rig_id: Uuid,
    pub base_url: String,
    pub name: String,
    pub allow_loopback_http: bool,
    pub agent_id: Option<String>,
    pub state: ConnectionState,
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
