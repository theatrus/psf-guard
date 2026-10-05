//! The working description of a site: where it is and what blocks its sky.
//! Rigs that name the site as their planning site inherit both; a rig's own
//! location or horizon, typed or reported by the plugin, still wins.

use super::*;
use crate::profile::{Reported, RigProfile};
use psf_guard_director_core::priority::Scope;
use psf_guard_director_core::visibility::{Horizon, Site};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SiteProfile {
    pub site_id: Uuid,
    /// Compare-and-set revision; every accepted change increments it.
    pub revision: u64,
    pub location: Option<Reported<Site>>,
    /// `None` means each rig's flat minimum altitude.
    pub horizon: Option<Reported<Horizon>>,
    pub updated_at_ms: u64,
}

impl SiteProfile {
    pub fn empty(site_id: Uuid, now_ms: u64) -> Self {
        Self {
            site_id,
            revision: 0,
            location: None,
            horizon: None,
            updated_at_ms: now_ms,
        }
    }
}

/// Which record a rig's location or horizon came from.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Rig,
    Site,
    /// Nothing set: no location, or the flat minimum altitude.
    None,
}

/// The location and horizon planning uses for one rig.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RigSite {
    /// The planning site the rig names, if any.
    pub site: Option<NamedIdentity>,
    pub location: Option<Site>,
    pub location_from: Origin,
    pub horizon: Horizon,
    pub horizon_from: Origin,
}

const MAX_TIME_MS: u64 = 4_102_444_800_000; // 2100-01-01

fn validate(profile: &SiteProfile) -> Result<(), Error> {
    valid_id(profile.site_id)?;
    if profile.updated_at_ms > MAX_TIME_MS {
        return Err(Error::InvalidInput);
    }
    if let Some(location) = &profile.location {
        if location.reported_at_ms > MAX_TIME_MS {
            return Err(Error::InvalidInput);
        }
        location.value.validate().map_err(|_| Error::InvalidInput)?;
    }
    if let Some(horizon) = &profile.horizon {
        if horizon.reported_at_ms > MAX_TIME_MS {
            return Err(Error::InvalidInput);
        }
        horizon.value.validate().map_err(|_| Error::InvalidInput)?;
    }
    Ok(())
}

pub(crate) fn create_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS site_profile(
            site_id TEXT PRIMARY KEY NOT NULL REFERENCES site(id),
            revision INTEGER NOT NULL CHECK(revision>0),
            payload TEXT NOT NULL);",
    )?;
    Ok(())
}

impl MetaStore {
    pub fn site_profile(&self, site: Uuid) -> Result<Option<SiteProfile>, Error> {
        read(&self.connection, site)
    }

    /// Save with compare-and-set, as for rig profiles: `expected_revision` is
    /// the revision the caller read (0 when none existed), and an unchanged
    /// body returns the stored record without bumping it.
    pub fn save_site_profile(
        &mut self,
        profile: &SiteProfile,
        expected_revision: u64,
    ) -> Result<SiteProfile, Error> {
        validate(profile)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Site, profile.site_id)?.is_none() {
            return Err(Error::NotFound);
        }
        let stored = read(&tx, profile.site_id)?;
        let current = stored.as_ref().map_or(0, |value| value.revision);
        if current != expected_revision {
            return Err(Error::Conflict);
        }
        let mut next = profile.clone();
        next.revision = current;
        if let Some(stored) = stored {
            let mut same = stored.clone();
            same.updated_at_ms = next.updated_at_ms;
            if same == next {
                return Ok(stored);
            }
        }
        next.revision = current.checked_add(1).ok_or(Error::Conflict)?;
        let payload = super::configuration::encode(&next)?;
        tx.execute(
            "INSERT INTO site_profile(site_id,revision,payload) VALUES(?1,?2,?3)
             ON CONFLICT(site_id) DO UPDATE SET revision=excluded.revision, payload=excluded.payload",
            params![
                next.site_id.to_string(),
                i64::try_from(next.revision).map_err(|_| Error::Conflict)?,
                payload
            ],
        )?;
        tx.commit()?;
        Ok(next)
    }

    /// Location and horizon for a rig: its own profile first, then the
    /// planning site it names, then nothing. `profile` is the rig's stored
    /// profile, passed in because every caller has already read it.
    pub fn rig_site(&self, rig: Uuid, profile: Option<&RigProfile>) -> Result<RigSite, Error> {
        let site_id = self.observing_settings(Scope::Rig, rig)?.site_id;
        let site = site_id
            .map(|id| read_named(&self.connection, Kind::Site, id))
            .transpose()?
            .flatten();
        let site_profile = site
            .as_ref()
            .map(|site| read(&self.connection, site.id))
            .transpose()?
            .flatten();
        let own_location = profile.and_then(|p| p.site.as_ref()).map(|s| s.value);
        let site_location = site_profile
            .as_ref()
            .and_then(|p| p.location.as_ref())
            .map(|s| s.value);
        let (location, location_from) = match (own_location, site_location) {
            (Some(location), _) => (Some(location), Origin::Rig),
            (None, Some(location)) => (Some(location), Origin::Site),
            (None, None) => (None, Origin::None),
        };
        let own_horizon = profile.and_then(|p| p.horizon.as_ref()).map(|h| &h.value);
        let site_horizon = site_profile
            .as_ref()
            .and_then(|p| p.horizon.as_ref())
            .map(|h| &h.value);
        let (horizon, horizon_from) = match (own_horizon, site_horizon) {
            (Some(horizon), _) => (horizon.clone(), Origin::Rig),
            (None, Some(horizon)) => (horizon.clone(), Origin::Site),
            (None, None) => (Horizon::FixedMinimum {}, Origin::None),
        };
        Ok(RigSite {
            site,
            location,
            location_from,
            horizon,
            horizon_from,
        })
    }
}

fn read(conn: &Connection, site: Uuid) -> Result<Option<SiteProfile>, Error> {
    valid_id(site)?;
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT revision,substr(CAST(payload AS BLOB),1,?2) FROM site_profile WHERE site_id=?1",
            params![
                site.to_string(),
                (psf_guard_director_core::MAX_REQUEST_BYTES + 1) as i64
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(revision, payload)| {
        let value: SiteProfile = super::configuration::decode(payload)?;
        validate(&value).map_err(|_| Error::CorruptDatabase)?;
        if value.site_id != site || revision <= 0 || value.revision != revision as u64 {
            return Err(Error::CorruptDatabase);
        }
        Ok(value)
    })
    .transpose()
}
