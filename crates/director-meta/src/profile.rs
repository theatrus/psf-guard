//! The mutable working description of a rig: optics, site, horizon, limits
//! and the equipment the plugin last reported. Activation later freezes a
//! setup revision from it; the profile itself is never an authorization.

use super::*;
use psf_guard_director_core::{
    optics::Optics,
    program::{validate_configuration, Configuration},
    visibility::{AltitudeLimits, Horizon, Site},
    windows::MeridianExclusion,
};

/// Who picks a rig's work: Target Scheduler from the rig's database, or
/// Director through its N.I.N.A. plugin. Night plans schedule the rig with
/// it, and it decides whether the rig takes night overrides or Target
/// Scheduler settings. It grants the plugin nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RigMode {
    TargetScheduler,
    Director,
}

impl RigMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::TargetScheduler => "target_scheduler",
            Self::Director => "director",
        }
    }
}

impl fmt::Display for RigMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for RigMode {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "target_scheduler" => Ok(Self::TargetScheduler),
            "director" => Ok(Self::Director),
            _ => Err(Error::InvalidInput),
        }
    }
}

/// Where a value came from, so the UI can say "from FITS headers" or
/// "reported by N.I.N.A." instead of presenting a guess as a fact.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    Manual {},
    FrameHeaders { file_name: String },
    Plugin {},
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Reported<T> {
    pub value: T,
    pub source: Source,
    pub reported_at_ms: u64,
}

/// Sky darkness at the rig, used only to pick default exposure lengths.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SkyQuality {
    pub bortle_class: u8,
    #[serde(deserialize_with = "Option::deserialize")]
    pub sqm_mag_per_arcsec2: Option<f64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub minimum_altitude_degrees: f64,
    pub maximum_altitude_degrees: f64,
    pub meridian_exclusion: MeridianExclusion,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            minimum_altitude_degrees: 20.0,
            maximum_altitude_degrees: 90.0,
            meridian_exclusion: MeridianExclusion {
                before_ms: 0,
                after_ms: 0,
            },
        }
    }
}

/// The gain and offset N.I.N.A. uses for a frame whose template sets neither.
/// A recipe must carry explicit values, so these fill in the template's -1.
/// `None` where the camera has no such control.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CameraDefaults {
    pub gain: Option<i32>,
    pub offset: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RigProfile {
    pub rig_id: Uuid,
    /// Compare-and-set revision; every accepted change increments it.
    pub revision: u64,
    pub optics: Option<Reported<Optics>>,
    pub site: Option<Reported<Site>>,
    /// `None` means the flat minimum altitude from `limits`.
    pub horizon: Option<Reported<Horizon>>,
    pub sky_quality: Option<Reported<SkyQuality>>,
    pub limits: Reported<Limits>,
    /// Only the plugin reports this; operators cannot type a camera's modes.
    pub configuration: Option<Reported<Configuration>>,
    /// Native filter labels keyed by the exact configuration filter IDs.
    /// Labels help compile templates; they never replace dispatch identities.
    #[serde(default)]
    pub filter_names: std::collections::BTreeMap<String, String>,
    /// What the camera uses for a template that leaves gain or offset to it.
    /// `None` until a plugin that reports them is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_defaults: Option<CameraDefaults>,
    /// The registered Sync peer that holds this rig's database when the rig
    /// runs on another PSF Guard. Activation pushes the plan there. `None`
    /// means the database on this server is the one the rig executes from.
    #[serde(default)]
    pub peer_id: Option<String>,
    /// Who picks the rig's work, as set in Rig setup or by pairing a
    /// Director plugin. `None` until set: PSF Guard then guesses from what
    /// the rig has reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<RigMode>,
    pub updated_at_ms: u64,
}

impl RigProfile {
    pub fn empty(rig_id: Uuid, now_ms: u64) -> Self {
        Self {
            rig_id,
            revision: 0,
            optics: None,
            site: None,
            horizon: None,
            sky_quality: None,
            limits: Reported {
                value: Limits::default(),
                source: Source::Manual {},
                reported_at_ms: now_ms,
            },
            configuration: None,
            filter_names: Default::default(),
            camera_defaults: None,
            peer_id: None,
            mode: None,
            updated_at_ms: now_ms,
        }
    }
}

const MAX_PEER_ID_LEN: usize = 128;

const MAX_TIME_MS: u64 = 4_102_444_800_000; // 2100-01-01

fn valid_source(source: &Source) -> Result<(), Error> {
    match source {
        Source::FrameHeaders { file_name }
            if file_name.is_empty()
                || file_name.len() > 512
                || file_name.chars().any(char::is_control) =>
        {
            Err(Error::InvalidInput)
        }
        _ => Ok(()),
    }
}

fn valid_reported<T>(reported: &Reported<T>) -> Result<(), Error> {
    valid_source(&reported.source)?;
    if reported.reported_at_ms > MAX_TIME_MS {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

pub(crate) fn validate_profile(profile: &RigProfile) -> Result<(), Error> {
    valid_id(profile.rig_id)?;
    if profile.updated_at_ms > MAX_TIME_MS {
        return Err(Error::InvalidInput);
    }
    if let Some(peer) = &profile.peer_id
        && (peer.is_empty()
            || peer.len() > MAX_PEER_ID_LEN
            || peer.trim() != peer
            || peer.chars().any(char::is_control))
    {
        return Err(Error::InvalidInput);
    }
    if let Some(optics) = &profile.optics {
        valid_reported(optics)?;
        optics.value.validate().map_err(|_| Error::InvalidInput)?;
    }
    if let Some(site) = &profile.site {
        valid_reported(site)?;
        site.value.validate().map_err(|_| Error::InvalidInput)?;
    }
    if let Some(horizon) = &profile.horizon {
        valid_reported(horizon)?;
        horizon.value.validate().map_err(|_| Error::InvalidInput)?;
    }
    if let Some(sky) = &profile.sky_quality {
        valid_reported(sky)?;
        if !(1..=9).contains(&sky.value.bortle_class)
            || sky
                .value
                .sqm_mag_per_arcsec2
                .is_some_and(|sqm| !sqm.is_finite() || !(10.0..=23.0).contains(&sqm))
        {
            return Err(Error::InvalidInput);
        }
    }
    valid_reported(&profile.limits)?;
    let limits = &profile.limits.value;
    AltitudeLimits {
        rig_minimum_degrees: limits.minimum_altitude_degrees,
        rig_maximum_degrees: limits.maximum_altitude_degrees,
        project_minimum_degrees: -90.0,
        project_maximum_degrees: 90.0,
        horizon_offset_degrees: 0.0,
    }
    .validate()
    .map_err(|_| Error::InvalidInput)?;
    if limits.meridian_exclusion.before_ms > 86_400_000
        || limits.meridian_exclusion.after_ms > 86_400_000
    {
        return Err(Error::InvalidInput);
    }
    if let Some(configuration) = &profile.configuration {
        valid_reported(configuration)?;
        if configuration.source != (Source::Plugin {}) {
            return Err(Error::InvalidInput);
        }
        validate_configuration(&configuration.value).map_err(|_| Error::InvalidInput)?;
        if parse_id(&configuration.value.rig_id).map_err(|_| Error::InvalidInput)? != profile.rig_id
        {
            return Err(Error::InvalidInput);
        }
    }
    if !profile.filter_names.is_empty() {
        let configuration = profile.configuration.as_ref().ok_or(Error::InvalidInput)?;
        if profile.filter_names.len() != configuration.value.filters.len()
            || configuration
                .value
                .filters
                .iter()
                .any(|f| !profile.filter_names.contains_key(&f.id))
            || profile.filter_names.values().any(|name| {
                name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control)
            })
        {
            return Err(Error::InvalidInput);
        }
    }
    if let Some(defaults) = &profile.camera_defaults {
        let configuration = &profile
            .configuration
            .as_ref()
            .ok_or(Error::InvalidInput)?
            .value;
        if defaults
            .gain
            .is_some_and(|v| !configuration.gain.accepts(Some(v)))
            || defaults
                .offset
                .is_some_and(|v| !configuration.offset.accepts(Some(v)))
        {
            return Err(Error::InvalidInput);
        }
    }
    Ok(())
}

impl MetaStore {
    pub fn rig_profile(&self, rig: Uuid) -> Result<Option<RigProfile>, Error> {
        read_profile(&self.connection, rig)
    }

    /// Save with compare-and-set. `expected_revision` is the revision the
    /// caller read (0 when none existed); the stored revision becomes one
    /// more. An unchanged body under the right revision is a no-op that
    /// returns the stored record, so a retried request does not bump it.
    pub fn save_rig_profile(
        &mut self,
        profile: &RigProfile,
        expected_revision: u64,
    ) -> Result<RigProfile, Error> {
        validate_profile(profile)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next = save_profile(&tx, profile, expected_revision)?;
        tx.commit()?;
        Ok(next)
    }

    /// Set who picks the rig's work when nobody has chosen, as pairing a
    /// Director plugin does. A chosen mode stays: a rig can pair the plugin
    /// to try it while Target Scheduler keeps running the rig. A rig without
    /// a profile gets one.
    pub fn default_rig_mode(
        &mut self,
        rig: Uuid,
        mode: RigMode,
        now_ms: u64,
    ) -> Result<RigProfile, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = read_profile(&tx, rig)?;
        let revision = stored.as_ref().map_or(0, |profile| profile.revision);
        let mut next = stored.unwrap_or_else(|| RigProfile::empty(rig, now_ms));
        if next.mode.is_some() {
            tx.commit()?;
            return Ok(next);
        }
        next.mode = Some(mode);
        next.updated_at_ms = now_ms;
        let saved = save_profile(&tx, &next, revision)?;
        tx.commit()?;
        Ok(saved)
    }
}

pub(crate) fn save_profile(
    conn: &Connection,
    profile: &RigProfile,
    expected_revision: u64,
) -> Result<RigProfile, Error> {
    validate_profile(profile)?;
    if read_named(conn, Kind::Rig, profile.rig_id)?.is_none() {
        return Err(Error::NotFound);
    }
    let stored = read_profile(conn, profile.rig_id)?;
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
    conn.execute(
            "INSERT INTO rig_profile(rig_id,revision,payload) VALUES(?1,?2,?3)
             ON CONFLICT(rig_id) DO UPDATE SET revision=excluded.revision, payload=excluded.payload",
            params![
                next.rig_id.to_string(),
                i64::try_from(next.revision).map_err(|_| Error::Conflict)?,
                payload
            ],
        )?;
    crate::workload::follow_profile_change(
        conn,
        next.rig_id,
        next.revision,
        next.configuration
            .as_ref()
            .map(|reported| reported.value.id.as_str()),
    )?;
    Ok(next)
}

pub(crate) fn read_profile(conn: &Connection, rig: Uuid) -> Result<Option<RigProfile>, Error> {
    valid_id(rig)?;
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT revision,substr(CAST(payload AS BLOB),1,?2) FROM rig_profile WHERE rig_id=?1",
            params![
                rig.to_string(),
                (psf_guard_director_core::MAX_REQUEST_BYTES + 1) as i64
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(revision, payload)| {
        let value: RigProfile = super::configuration::decode(payload)?;
        validate_profile(&value).map_err(|_| Error::CorruptDatabase)?;
        if value.rig_id != rig || revision <= 0 || value.revision != revision as u64 {
            return Err(Error::CorruptDatabase);
        }
        Ok(value)
    })
    .transpose()
}
