//! Immutable configuration revisions. IDs select exact snapshots, not "latest".
//! Dynamic EOP, conditions and permission to acquire are deliberately absent.

use super::*;
use psf_guard_director_core::{
    program::{validate_configuration, Configuration},
    visibility::{AltitudeLimits, Horizon, Site},
    windows::MeridianExclusion,
    MAX_REQUEST_BYTES,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SiteSnapshot {
    pub id: Uuid,
    pub site_id: Uuid,
    pub location: Site,
    pub horizon: Horizon,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RigSetup {
    /// Coordinator revision identity, distinct from the native equipment ID.
    pub id: Uuid,
    pub configuration: Configuration,
    pub site_snapshot_id: Uuid,
    pub minimum_altitude_degrees: f64,
    pub maximum_altitude_degrees: f64,
    pub meridian_exclusion: MeridianExclusion,
}

/// A bounded ID inventory. Fetch the immutable record with its snapshot ID.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotIds {
    pub ids: Vec<Uuid>,
    pub next_after: Option<Uuid>,
}

impl MetaStore {
    pub fn create_site(&mut self, id: Uuid, name: &str) -> Result<NamedIdentity, Error> {
        self.create_named(Kind::Site, id, name)
    }
    pub fn site(&self, id: Uuid) -> Result<Option<NamedIdentity>, Error> {
        read_named(&self.connection, Kind::Site, id)
    }
    pub fn sites(&self, after: Option<Uuid>, limit: usize) -> Result<IdentityPage, Error> {
        self.list_named(Kind::Site, after, limit)
    }
    pub fn rename_site(
        &mut self,
        id: Uuid,
        revision: u64,
        name: &str,
    ) -> Result<NamedIdentity, Error> {
        self.rename(Kind::Site, id, revision, name)
    }

    pub fn register_site_snapshot(&mut self, snapshot: &SiteSnapshot) -> Result<(), Error> {
        validate_site(snapshot)?;
        let payload = encode(snapshot)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Site, snapshot.site_id)?.is_none() {
            return Err(Error::NotFound);
        }
        if let Some(existing) = read_site(&tx, snapshot.id)? {
            if existing != *snapshot {
                return Err(Error::Conflict);
            }
        } else {
            tx.execute(
                "INSERT INTO site_snapshot(id,site_id,payload) VALUES(?1,?2,?3)",
                params![
                    snapshot.id.to_string(),
                    snapshot.site_id.to_string(),
                    payload
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn site_snapshot(&self, id: Uuid) -> Result<Option<SiteSnapshot>, Error> {
        read_site(&self.connection, id)
    }

    pub fn register_rig_setup(&mut self, setup: &RigSetup) -> Result<(), Error> {
        let (id, rig_id) = validate_setup(setup)?;
        let payload = encode(setup)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Rig, rig_id)?.is_none()
            || read_site(&tx, setup.site_snapshot_id)?.is_none()
        {
            return Err(Error::NotFound);
        }
        if let Some(existing) = read_setup(&tx, id)? {
            if existing != *setup {
                return Err(Error::Conflict);
            }
        } else {
            tx.execute(
                "INSERT INTO rig_setup(id,rig_id,site_snapshot_id,payload) VALUES(?1,?2,?3,?4)",
                params![
                    id.to_string(),
                    rig_id.to_string(),
                    setup.site_snapshot_id.to_string(),
                    payload
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn rig_setup(&self, id: Uuid) -> Result<Option<RigSetup>, Error> {
        read_setup(&self.connection, id)
    }

    pub fn site_snapshot_ids(
        &self,
        site: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<SnapshotIds, Error> {
        snapshot_ids(
            &self.connection,
            "SELECT id FROM site_snapshot WHERE site_id=?1 AND id>?2 ORDER BY id LIMIT ?3",
            site,
            after,
            limit,
        )
    }
    pub fn rig_setup_ids(
        &self,
        rig: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<SnapshotIds, Error> {
        snapshot_ids(
            &self.connection,
            "SELECT id FROM rig_setup WHERE rig_id=?1 AND id>?2 ORDER BY id LIMIT ?3",
            rig,
            after,
            limit,
        )
    }
}

fn validate_site(snapshot: &SiteSnapshot) -> Result<(), Error> {
    valid_id(snapshot.id)?;
    valid_id(snapshot.site_id)?;
    snapshot
        .location
        .validate()
        .map_err(|_| Error::InvalidInput)?;
    snapshot
        .horizon
        .validate()
        .map_err(|_| Error::InvalidInput)?;
    Ok(())
}
fn validate_setup(setup: &RigSetup) -> Result<(Uuid, Uuid), Error> {
    validate_configuration(&setup.configuration).map_err(|_| Error::InvalidInput)?;
    valid_id(setup.id)?;
    let rig = parse_id(&setup.configuration.rig_id).map_err(|_| Error::InvalidInput)?;
    valid_id(setup.site_snapshot_id)?;
    AltitudeLimits {
        rig_minimum_degrees: setup.minimum_altitude_degrees,
        rig_maximum_degrees: setup.maximum_altitude_degrees,
        project_minimum_degrees: -90.0,
        project_maximum_degrees: 90.0,
        horizon_offset_degrees: 0.0,
    }
    .validate()
    .map_err(|_| Error::InvalidInput)?;
    Ok((setup.id, rig))
}
fn encode(value: &impl Serialize) -> Result<String, Error> {
    let json = serde_json::to_string(value).map_err(|_| Error::InvalidInput)?;
    if json.len() > MAX_REQUEST_BYTES {
        return Err(Error::InvalidInput);
    }
    Ok(json)
}
fn decode<T: serde::de::DeserializeOwned>(bytes: Vec<u8>) -> Result<T, Error> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(Error::CorruptDatabase);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::CorruptDatabase)
}
fn read_site(conn: &Connection, id: Uuid) -> Result<Option<SiteSnapshot>, Error> {
    valid_id(id)?;
    let row: Option<(String, Vec<u8>)> = conn
        .query_row(
            "SELECT site_id,substr(CAST(payload AS BLOB),1,?2) FROM site_snapshot WHERE id=?1",
            params![id.to_string(), (MAX_REQUEST_BYTES + 1) as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(site, payload)| {
        let value: SiteSnapshot = decode(payload)?;
        validate_site(&value).map_err(|_| Error::CorruptDatabase)?;
        if value.id != id || value.site_id != parse_id(&site)? {
            return Err(Error::CorruptDatabase);
        }
        Ok(value)
    })
    .transpose()
}
fn read_setup(conn: &Connection, id: Uuid) -> Result<Option<RigSetup>, Error> {
    valid_id(id)?;
    let row: Option<(String, String, Vec<u8>)> = conn.query_row(
        "SELECT rig_id,site_snapshot_id,substr(CAST(payload AS BLOB),1,?2) FROM rig_setup WHERE id=?1",
        params![id.to_string(), (MAX_REQUEST_BYTES + 1) as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
    row.map(|(rig, site, payload)| {
        let value: RigSetup = decode(payload)?;
        let (stored_id, stored_rig) = validate_setup(&value).map_err(|_| Error::CorruptDatabase)?;
        if stored_id != id
            || stored_rig != parse_id(&rig)?
            || value.site_snapshot_id != parse_id(&site)?
        {
            return Err(Error::CorruptDatabase);
        }
        Ok(value)
    })
    .transpose()
}
fn snapshot_ids(
    conn: &Connection,
    sql: &str,
    owner: Uuid,
    after: Option<Uuid>,
    limit: usize,
) -> Result<SnapshotIds, Error> {
    valid_id(owner)?;
    if !(1..=256).contains(&limit) {
        return Err(Error::InvalidInput);
    }
    if let Some(id) = after {
        valid_id(id)?;
    }
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(
        params![
            owner.to_string(),
            after.map(|id| id.to_string()).unwrap_or_default(),
            (limit + 1) as i64
        ],
        |r| r.get::<_, String>(0),
    )?;
    let mut ids = Vec::with_capacity(limit + 1);
    for id in rows {
        ids.push(parse_id(&id?)?);
    }
    let next_after = if ids.len() > limit {
        ids.pop();
        ids.last().copied()
    } else {
        None
    };
    Ok(SnapshotIds { ids, next_after })
}
