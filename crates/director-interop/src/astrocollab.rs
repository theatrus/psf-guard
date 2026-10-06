//! AstroCollab 0.2.0-draft.1 / wire protocol 1. No HTTP or equipment access.

use crate::{json, Error};
use chrono::{NaiveDate, NaiveTime};
use psf_guard_director_core::program::MAS_PER_DEGREE;
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;
use url::{Host, Url};

pub const SCHEMA_VERSION: u32 = 1;
pub const WIRE_PROTOCOL: u32 = 1;
pub const MAX_BODY_BYTES: usize = json::MAX_BYTES;
pub const MAX_TASKS: usize = 32;
pub const MAX_PROJECTS: usize = 64;
pub const MAX_CELLS: usize = 256;
pub const MAX_DEMANDS: usize = 256;
const MAX_FILTERS: usize = 32;
const FULL_CIRCLE: u32 = 360 * MAS_PER_DEGREE;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Source {
    base_url: String,
    agent_id: String,
}

impl Source {
    pub fn new(base_url: &str, agent_id: &str, allow_loopback_http: bool) -> Result<Self, Error> {
        let mut url = Url::parse(base_url).map_err(|_| Error::InvalidSource)?;
        let loopback = match url.host() {
            Some(Host::Domain("localhost")) => true,
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if (url.scheme() != "https" && !(url.scheme() == "http" && loopback && allow_loopback_http))
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || base_url.len() > 2048
        {
            return Err(Error::InvalidSource);
        }
        validate_id(agent_id)?;
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        Ok(Self {
            base_url: url.to_string(),
            agent_id: agent_id.into(),
        })
    }
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Health {
    pub protocol: u32,
    pub build: String,
    pub server_time_ms: u64,
    /// None requires /auth discovery; it does not imply pairing or files.
    pub features: Option<BTreeSet<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Rotation {
    Unknown,
    Fixed { position_angle_mas: u32 },
    Adjustable,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Profile {
    pub name: Option<String>,
    pub focal_length_mm: Option<f64>,
    pub pixel_size_um: Option<f64>,
    pub sensor_width: Option<u32>,
    pub sensor_height: Option<u32>,
    pub binning: Option<u32>,
    pub colour: bool,
    pub rotation: Rotation,
    pub filters_nm: BTreeMap<String, Option<f64>>,
    pub exposures_ms: BTreeMap<String, Option<u64>>,
    pub typical_hfr_arcsec: Option<f64>,
    pub typical_guide_rms_arcsec: Option<f64>,
    pub hours_per_night: Option<f64>,
    pub window_from: Option<String>,
    pub window_to: Option<String>,
    pub reported_scale_arcsec_per_pixel: Option<f64>,
    pub reported_field_degrees: Option<[f64; 2]>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Requirements {
    pub min_focal_length_mm: Option<f64>,
    pub max_focal_length_mm: Option<f64>,
    pub min_scale_arcsec: Option<f64>,
    pub max_scale_arcsec: Option<f64>,
    pub accept_colour: bool,
    pub colour_max_moon: Option<f64>,
    pub max_hfr_arcsec: Option<f64>,
    pub max_guide_rms_arcsec: Option<f64>,
    pub min_exposure_ms: Option<u64>,
    pub max_exposure_ms: Option<u64>,
    pub filters_nm: BTreeMap<String, Option<f64>>,
    pub max_moon_illumination: Option<f64>,
    pub min_moon_separation_degrees: Option<f64>,
    pub min_altitude_degrees: Option<f64>,
    pub require_calibrated: bool,
    pub min_frames_per_visit: Option<u32>,
    /// Explicit unreadable limits cannot silently become unrestricted work.
    pub unresolved_fields: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Region {
    pub icrs_ra_mas: u32,
    pub icrs_dec_mas: i32,
    pub width_mas: u32,
    pub height_mas: u32,
    pub position_angle_mas: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Cell {
    pub index: u32,
    pub row: u32,
    pub column: u32,
    pub region: Region,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Single,
    Mosaic,
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Offered,
    Accepted,
    Declined,
    Complete,
    Superseded,
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    NotAccepted,
    UnknownKind,
    WrongNight,
    MissingRequirements,
    UnresolvedRequirements,
    EmptyShare,
    MissingVisit,
    BelowMinimumVisit,
    ExposureOutsideRequirements,
    FilterOutsideRequirements,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PanelDemand {
    pub panel_index: u32,
    pub filter: String,
    pub exposure_ms: u64,
    pub requested_frames: u32,
}

/// A proposed visit, not a Program, Allocation, reservation or dispatch permit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Share {
    pub task_id: String,
    pub project_id: String,
    pub name: Option<String>,
    pub version: u32,
    pub state: State,
    pub kind: Kind,
    pub region: Region,
    pub cells: Vec<Cell>,
    pub geometry_digest: String,
    pub panel_order: Vec<u32>,
    pub assigned_night: Option<String>,
    pub requirements: Option<Requirements>,
    pub review_reasons: Vec<ReviewReason>,
    /// Empty until accepted, current-night work has complete requirements.
    pub demands: Vec<PanelDemand>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Tonight {
    pub schema_version: u32,
    pub source: Source,
    pub night: String,
    pub shares: Vec<Share>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Project {
    pub project_id: String,
    pub name: String,
    pub kind: Kind,
    pub region: Region,
    pub requirements: Requirements,
    pub goals_hours: BTreeMap<String, Option<f64>>,
    pub collected_hours: BTreeMap<String, Option<f64>>,
    pub joined: bool,
    pub compatible: Option<bool>,
    pub compatibility_certain: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Projects {
    pub schema_version: u32,
    pub source: Source,
    pub projects: Vec<Project>,
}

pub fn decode_health(bytes: &[u8]) -> Result<Health, Error> {
    let value = json::decode(bytes)?;
    let map = json::object(&value)?;
    protocol(map)?;
    if !json::boolean(map, "ok", false)? {
        return Err(Error::InvalidValue);
    }
    let features = map
        .get("features")
        .map(|v| {
            json::array(v, 32)?
                .iter()
                .map(|v| json::text(v, 80))
                .collect::<Result<BTreeSet<_>, _>>()
        })
        .transpose()?;
    Ok(Health {
        protocol: WIRE_PROTOCOL,
        build: json::text(required(map, "version")?, 120)?,
        server_time_ms: milliseconds(required(map, "time")?)?.ok_or(Error::InvalidValue)?,
        features,
    })
}

pub fn decode_hello_profile(bytes: &[u8]) -> Result<Profile, Error> {
    let value = json::decode(bytes)?;
    let envelope = json::object(&value)?;
    if envelope.contains_key("protocol") {
        protocol(envelope)?;
    }
    let map = json::object(required(envelope, "profile")?)?;
    let rotation = match map.get("rotation") {
        None => Rotation::Unknown,
        Some(Value::Null) => Rotation::Adjustable,
        Some(v) => match json::number(v) {
            Some(angle) => Rotation::Fixed {
                position_angle_mas: angle_mas(angle)?,
            },
            None => Rotation::Unknown,
        },
    };
    let filters_nm = filter_table(map.get("filters"), |v| json::ranged(v, 0.0, 1e6))?;
    let exposures_ms = filter_table(map.get("exposures"), milliseconds)?;
    let reported_field_degrees = map
        .get("field")
        .filter(|v| !v.is_null())
        .map(|v| {
            let field = json::array(v, 2)?;
            if field.len() != 2 {
                return Err(Error::InvalidValue);
            }
            let width = json::ranged(&field[0], 0.0, 360.0)?;
            let height = json::ranged(&field[1], 0.0, 180.0)?;
            Ok(width.zip(height).map(|(w, h)| [w, h]))
        })
        .transpose()?
        .flatten();
    Ok(Profile {
        name: json::optional_text(map, "name")?,
        focal_length_mm: optional_number(map, "focalLength", 0.0, 1e6)?,
        pixel_size_um: optional_number(map, "pixelSize", 0.0, 1e6)?,
        sensor_width: optional_integer(map, "sensorWidth", 1)?,
        sensor_height: optional_integer(map, "sensorHeight", 1)?,
        binning: if map.contains_key("binning") {
            optional_integer(map, "binning", 1)?
        } else {
            Some(1)
        },
        colour: json::boolean(map, "colour", false)?,
        rotation,
        filters_nm,
        exposures_ms,
        typical_hfr_arcsec: optional_number(map, "typicalHfr", 0.0, 1e6)?,
        typical_guide_rms_arcsec: optional_number(map, "typicalGuideRms", 0.0, 1e6)?,
        hours_per_night: optional_number(map, "hoursPerNight", 0.0, 24.0)?,
        window_from: clock_window(map, "windowFrom")?,
        window_to: clock_window(map, "windowTo")?,
        reported_scale_arcsec_per_pixel: optional_number(map, "scale", 0.0, 1e6)?,
        reported_field_degrees,
    })
}

pub fn decode_projects(bytes: &[u8], source: &Source) -> Result<Projects, Error> {
    let value = json::decode(bytes)?;
    let map = json::object(&value)?;
    protocol(map)?;
    let mut seen = BTreeSet::new();
    let mut projects = Vec::new();
    for raw in json::array(required(map, "projects")?, MAX_PROJECTS)? {
        let p = json::object(raw)?;
        let project_id = id(required(p, "id")?)?;
        if !seen.insert(project_id.clone()) {
            return Err(Error::InvalidIdentity);
        }
        let compatibility = p.get("compatibility").map(json::object).transpose()?;
        projects.push(Project {
            project_id,
            name: json::text(required(p, "name")?, 120)?,
            kind: kind(p)?,
            region: region(required(p, "region")?)?,
            requirements: requirements(required(p, "requirements")?)?,
            goals_hours: filter_table(Some(required(p, "goals")?), |v| json::ranged(v, 0.0, 1e6))?,
            collected_hours: filter_table(p.get("collected"), |v| json::ranged(v, 0.0, 1e6))?,
            joined: json::boolean(p, "joined", false)?,
            compatible: compatibility
                .map(|c| json::boolean(c, "ok", false))
                .transpose()?,
            compatibility_certain: compatibility
                .map(|c| json::boolean(c, "certain", false))
                .transpose()?,
        });
    }
    Ok(Projects {
        schema_version: SCHEMA_VERSION,
        source: source.clone(),
        projects,
    })
}

pub fn decode_tonight(bytes: &[u8], source: &Source, night: &str) -> Result<Tonight, Error> {
    validate_night(night)?;
    let value = json::decode(bytes)?;
    let map = json::object(&value)?;
    protocol(map)?;
    let legacy = map.get("task").filter(|v| !v.is_null());
    let raw_tasks: Vec<&Value> = match map.get("tasks") {
        Some(v) => json::array(v, MAX_TASKS)?.iter().collect(),
        None => legacy.into_iter().collect(),
    };
    let first_project = raw_tasks
        .first()
        .map(|v| id(required(json::object(v)?, "project")?))
        .transpose()?;
    let by_project = map
        .get("requirementsByProject")
        .map(json::object)
        .transpose()?;
    let read = |raw: &Value| -> Result<Share, Error> {
        let project = id(required(json::object(raw)?, "project")?)?;
        let wants = by_project.and_then(|p| p.get(&project)).or_else(|| {
            (first_project.as_ref() == Some(&project))
                .then(|| map.get("requirements"))
                .flatten()
        });
        share(raw, source, night, wants)
    };
    let mut shares = Vec::new();
    let mut seen = BTreeSet::new();
    let mut demands = 0;
    for raw in raw_tasks {
        let parsed = read(raw)?;
        if !seen.insert(parsed.task_id.clone()) {
            return Err(Error::AmbiguousTask);
        }
        demands += parsed.demands.len();
        if demands > MAX_DEMANDS {
            return Err(Error::LimitExceeded);
        }
        shares.push(parsed);
    }
    // A supplied compatibility alias must describe the same first share, not
    // hide different geometry or replace an explicitly empty tasks list.
    if let Some(raw) = legacy
        && shares.first() != Some(&read(raw)?)
    {
        return Err(Error::AmbiguousTask);
    }
    if map.get("task").is_some_and(Value::is_null) && !shares.is_empty() {
        return Err(Error::AmbiguousTask);
    }
    Ok(Tonight {
        schema_version: SCHEMA_VERSION,
        source: source.clone(),
        night: night.into(),
        shares,
    })
}

fn share(raw: &Value, source: &Source, night: &str, wants: Option<&Value>) -> Result<Share, Error> {
    let map = json::object(raw)?;
    let task_id = id(required(map, "id")?)?;
    let project_id = id(required(map, "project")?)?;
    if let Some(agent) = map.get("agent")
        && id(agent)? != source.agent_id
    {
        return Err(Error::InvalidIdentity);
    }
    let version = match map.get("version") {
        None => 1,
        Some(v) => json::integer(v, 1)?.ok_or(Error::InvalidValue)?,
    };
    let state = match map
        .get("state")
        .map(|v| json::text(v, 80))
        .transpose()?
        .as_deref()
        .unwrap_or("offered")
    {
        "offered" => State::Offered,
        "accepted" => State::Accepted,
        "declined" => State::Declined,
        "complete" => State::Complete,
        "superseded" => State::Superseded,
        v => State::Unknown(v.into()),
    };
    let kind = kind(map)?;
    let region = region(required(map, "region")?)?;
    let mut cells = Vec::new();
    let mut positions = BTreeSet::new();
    if let Some(raw) = map.get("cells") {
        for (index, cell) in json::array(raw, MAX_CELLS)?.iter().enumerate() {
            let c = json::object(cell)?;
            let row = json::integer(required(c, "row")?, 0)?.ok_or(Error::InvalidValue)?;
            let column = json::integer(required(c, "column")?, 0)?.ok_or(Error::InvalidValue)?;
            if !positions.insert((row, column)) {
                return Err(Error::InvalidRegion);
            }
            cells.push(Cell {
                index: index as u32,
                row,
                column,
                region: self::region(cell)?,
            });
        }
    }
    if kind == Kind::Single && cells.len() > 1 {
        return Err(Error::InvalidRegion);
    }
    let mut panel_order = Vec::new();
    let mut panels = BTreeSet::new();
    if let Some(raw) = map.get("share") {
        for v in json::array(raw, MAX_CELLS)? {
            let index = json::integer(v, 0)?.ok_or(Error::InvalidValue)?;
            if index as usize >= cells.len() || !panels.insert(index) {
                return Err(Error::InvalidRegion);
            }
            panel_order.push(index);
        }
    }
    let assigned_night = json::optional_text(map, "assignedNight")?;
    if let Some(n) = &assigned_night
        && !n.is_empty()
    {
        validate_night(n)?;
    }
    let requirements = wants.map(requirements).transpose()?;
    let mut review_reasons = Vec::new();
    if state != State::Accepted {
        review_reasons.push(ReviewReason::NotAccepted);
    }
    if matches!(kind, Kind::Unknown(_)) {
        review_reasons.push(ReviewReason::UnknownKind);
    }
    if assigned_night.as_deref() != Some(night) {
        review_reasons.push(ReviewReason::WrongNight);
    }
    match &requirements {
        None => review_reasons.push(ReviewReason::MissingRequirements),
        Some(r) if !r.unresolved_fields.is_empty() => {
            review_reasons.push(ReviewReason::UnresolvedRequirements)
        }
        _ => {}
    }
    if panel_order.is_empty() {
        review_reasons.push(ReviewReason::EmptyShare);
    }
    let exposures = task_exposures(map)?;
    let mut visits = BTreeMap::new();
    if let Some(raw) = map.get("visit").filter(|v| !v.is_null()) {
        let visit = json::object(raw)?;
        if let Some(raw) = visit.get("frames") {
            visits = filter_table(Some(raw), |v| json::integer(v, 1))?;
            if let Some(chosen) = json::optional_text(visit, "filter")? {
                let chosen = fold_filter(&chosen)?;
                if visits.keys().any(|f| f != &chosen) {
                    return Err(Error::AmbiguousFilter);
                }
            }
        }
    }
    if visits.is_empty() {
        review_reasons.push(ReviewReason::MissingVisit);
    }
    if kind == Kind::Mosaic && visits.len() > 1 {
        return Err(Error::AmbiguousFilter);
    }
    let mut candidate = Vec::new();
    for (filter, frames) in visits {
        let Some(frames) = frames else {
            review_reasons.push(ReviewReason::MissingVisit);
            continue;
        };
        let exposure = exposures
            .get(&filter)
            .copied()
            .flatten()
            .ok_or(Error::InvalidValue)?;
        if exposure == 0 {
            return Err(Error::InvalidValue);
        }
        u64::from(frames)
            .checked_mul(exposure)
            .ok_or(Error::InvalidValue)?;
        if let Some(r) = &requirements {
            if r.min_frames_per_visit.is_some_and(|min| frames < min) {
                review_reasons.push(ReviewReason::BelowMinimumVisit);
            }
            if r.min_exposure_ms.is_some_and(|min| exposure < min)
                || r.max_exposure_ms.is_some_and(|max| exposure > max)
            {
                review_reasons.push(ReviewReason::ExposureOutsideRequirements);
            }
            if !r.filters_nm.is_empty() && !r.filters_nm.contains_key(&filter) {
                review_reasons.push(ReviewReason::FilterOutsideRequirements);
            }
        }
        for &panel_index in &panel_order {
            if candidate.len() == MAX_DEMANDS {
                return Err(Error::LimitExceeded);
            }
            candidate.push(PanelDemand {
                panel_index,
                filter: filter.clone(),
                exposure_ms: exposure,
                requested_frames: frames,
            });
        }
    }
    // Digest geometry only. State/version/visit/unknown fields do not rewrite
    // the sky represented by old captures; same-task retile is detectable.
    let geometry =
        serde_json::to_vec(&(&kind, &region, &cells)).map_err(|_| Error::InvalidValue)?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let geometry_digest = Sha256::digest(geometry)
        .iter()
        .flat_map(|b| {
            [
                HEX[(b >> 4) as usize] as char,
                HEX[(b & 15) as usize] as char,
            ]
        })
        .collect();
    let demands = if review_reasons.is_empty() {
        candidate
    } else {
        Vec::new()
    };
    Ok(Share {
        task_id,
        project_id,
        name: json::optional_text(map, "projectName")?,
        version,
        state,
        kind,
        region,
        cells,
        geometry_digest,
        panel_order,
        assigned_night,
        requirements,
        review_reasons,
        demands,
    })
}

fn requirements(raw: &Value) -> Result<Requirements, Error> {
    let map = json::object(raw)?;
    let mut unresolved_fields = Vec::new();
    let mut limit = |key: &str, minimum: f64, maximum: f64| -> Result<Option<f64>, Error> {
        let number = optional_number(map, key, minimum, maximum)?;
        if map.get(key).is_some_and(|v| !v.is_null()) && number.is_none() {
            unresolved_fields.push(key.into());
        }
        Ok(number)
    };
    let min_focal_length_mm = limit("minFocalLength", 0.0, 1e6)?;
    let max_focal_length_mm = limit("maxFocalLength", 0.0, 1e6)?;
    let min_scale_arcsec = limit("minScale", 0.0, 1e6)?;
    let max_scale_arcsec = limit("maxScale", 0.0, 1e6)?;
    let colour_max_moon = limit("colourMaxMoon", 0.0, 1.0)?;
    let max_hfr_arcsec = limit("maxHfr", 0.0, 1e6)?;
    let max_guide_rms_arcsec = limit("maxGuideRms", 0.0, 1e6)?;
    let min_exposure = limit("minExposure", 0.0, 1e9)?;
    let max_exposure = limit("maxExposure", 0.0, 1e9)?;
    let max_moon_illumination = limit("maxMoonIllumination", 0.0, 1.0)?;
    let min_moon_separation_degrees = limit("minMoonSeparation", 0.0, 180.0)?;
    let min_altitude_degrees = limit("minAltitude", -90.0, 90.0)?;
    let min_exposure_ms = min_exposure.map(seconds_ms).transpose()?;
    let max_exposure_ms = max_exposure.map(seconds_ms).transpose()?;
    for (min, max) in [
        (min_focal_length_mm, max_focal_length_mm),
        (min_scale_arcsec, max_scale_arcsec),
        (min_exposure, max_exposure),
    ] {
        if min.zip(max).is_some_and(|(min, max)| min > max) {
            return Err(Error::InvalidValue);
        }
    }
    let filters_nm = filter_table(map.get("filters"), |v| json::ranged(v, 0.0, 1e6))?;
    if let Some(raw) = map.get("filters") {
        for (key, value) in json::object(raw)? {
            if !value.is_null() && json::number(value).is_none() {
                unresolved_fields.push(format!("filters.{}", fold_filter(key)?));
            }
        }
    }
    let min_frames_per_visit = match map.get("minFramesPerVisit") {
        None => Some(10),
        Some(v) => json::integer(v, 1)?,
    };
    if min_frames_per_visit.is_none() {
        unresolved_fields.push("minFramesPerVisit".into());
    }
    Ok(Requirements {
        min_focal_length_mm,
        max_focal_length_mm,
        min_scale_arcsec,
        max_scale_arcsec,
        accept_colour: json::boolean(map, "acceptColour", true)?,
        colour_max_moon,
        max_hfr_arcsec,
        max_guide_rms_arcsec,
        min_exposure_ms,
        max_exposure_ms,
        filters_nm,
        max_moon_illumination,
        min_moon_separation_degrees,
        min_altitude_degrees,
        require_calibrated: json::boolean(map, "requireCalibrated", false)?,
        min_frames_per_visit,
        unresolved_fields,
    })
}

fn task_exposures(map: &Map<String, Value>) -> Result<BTreeMap<String, Option<u64>>, Error> {
    let mut out = BTreeMap::new();
    if let Some(raw) = map.get("filters") {
        for entry in json::array(raw, MAX_FILTERS)? {
            let e = json::object(entry)?;
            let key = fold_filter(&json::text(required(e, "filter")?, 80)?)?;
            if out
                .insert(key, milliseconds(required(e, "exposure")?)?)
                .is_some()
            {
                return Err(Error::AmbiguousFilter);
            }
        }
    }
    Ok(out)
}

fn filter_table<T>(
    raw: Option<&Value>,
    read: impl Fn(&Value) -> Result<Option<T>, Error>,
) -> Result<BTreeMap<String, Option<T>>, Error> {
    let mut out = BTreeMap::new();
    if let Some(raw) = raw {
        let map = json::object(raw)?;
        if map.len() > MAX_FILTERS {
            return Err(Error::LimitExceeded);
        }
        for (key, value) in map {
            if out.insert(fold_filter(key)?, read(value)?).is_some() {
                return Err(Error::AmbiguousFilter);
            }
        }
    }
    Ok(out)
}

/// Wire identity deliberately does not use the core's broader OSC/L aliases.
pub fn fold_filter(name: &str) -> Result<String, Error> {
    static BANDPASS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)[\s(\[-]*\d+(?:\.\d+)?\s*nm[)\]]*\s*$").expect("constant regex")
    });
    let name = json::text(&Value::String(name.into()), 80)?;
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(Error::InvalidValue);
    }
    let bare = BANDPASS.replace(&name, "");
    let key: String = bare
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '_' | '-' | '/'))
        .flat_map(char::to_lowercase)
        .collect();
    let letter = match key.as_str() {
        "l" | "lum" | "luminance" | "clear" | "uvircut" | "uvir" | "ircut" | "none" => "L",
        "r" | "red" => "R",
        "g" | "green" => "G",
        "b" | "blue" => "B",
        "h" | "ha" | "halpha" | "hα" | "hydrogenalpha" => "H",
        "o" | "oiii" | "o3" | "oxygen" | "oxygeniii" => "O",
        "s" | "sii" | "s2" | "sulphur" | "sulfur" | "sulphurii" | "sulfurii" => "S",
        _ => return Ok(name),
    };
    Ok(letter.into())
}

fn required<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a Value, Error> {
    map.get(key).ok_or(Error::InvalidValue)
}
pub(crate) fn validate_id(value: &str) -> Result<(), Error> {
    if value.len() != 12
        || !value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        Err(Error::InvalidIdentity)
    } else {
        Ok(())
    }
}
fn id(raw: &Value) -> Result<String, Error> {
    let id = json::text(raw, 12)?;
    validate_id(&id)?;
    Ok(id)
}
fn protocol(map: &Map<String, Value>) -> Result<(), Error> {
    if json::integer(required(map, "protocol")?, 1)? != Some(WIRE_PROTOCOL) {
        Err(Error::UnsupportedProtocol)
    } else {
        Ok(())
    }
}
fn validate_night(value: &str) -> Result<(), Error> {
    if value.len() != 10
        || !value.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
        || NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err()
    {
        Err(Error::InvalidNight)
    } else {
        Ok(())
    }
}
fn clock_window(map: &Map<String, Value>, key: &str) -> Result<Option<String>, Error> {
    let Some(value) = json::optional_text(map, key)? else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() != 5
        || !value.bytes().enumerate().all(|(i, b)| {
            if i == 2 {
                b == b':'
            } else {
                b.is_ascii_digit()
            }
        })
        || NaiveTime::parse_from_str(&value, "%H:%M").is_err()
    {
        return Err(Error::InvalidValue);
    }
    Ok(Some(value))
}
fn optional_number(
    map: &Map<String, Value>,
    key: &str,
    min: f64,
    max: f64,
) -> Result<Option<f64>, Error> {
    map.get(key).map_or(Ok(None), |v| json::ranged(v, min, max))
}
fn optional_integer(map: &Map<String, Value>, key: &str, min: u32) -> Result<Option<u32>, Error> {
    map.get(key).map_or(Ok(None), |v| json::integer(v, min))
}
fn seconds_ms(value: f64) -> Result<u64, Error> {
    let ms = value * 1000.0;
    if !ms.is_finite()
        || !(0.0..=9_007_199_254_740_991.0).contains(&ms)
        || (ms - ms.round()).abs() > 0.000001
    {
        Err(Error::InvalidValue)
    } else {
        Ok(ms.round() as u64)
    }
}
fn milliseconds(raw: &Value) -> Result<Option<u64>, Error> {
    json::number(raw).map(seconds_ms).transpose()
}
fn kind(map: &Map<String, Value>) -> Result<Kind, Error> {
    Ok(
        match map
            .get("kind")
            .map(|v| json::text(v, 80))
            .transpose()?
            .as_deref()
            .unwrap_or("mosaic")
        {
            "single" => Kind::Single,
            "mosaic" => Kind::Mosaic,
            other => Kind::Unknown(other.into()),
        },
    )
}
fn angle_mas(degrees: f64) -> Result<u32, Error> {
    if !degrees.is_finite() || degrees.abs() > 3600.0 {
        return Err(Error::InvalidRegion);
    }
    Ok(((degrees.rem_euclid(360.0) * f64::from(MAS_PER_DEGREE)).round() as u32) % FULL_CIRCLE)
}
fn region(raw: &Value) -> Result<Region, Error> {
    let map = json::object(raw)?;
    let get =
        |key, min, max| json::ranged(required(map, key)?, min, max)?.ok_or(Error::InvalidRegion);
    let ra = get("ra", 0.0, 360.0)?;
    if ra == 360.0 {
        return Err(Error::InvalidRegion);
    }
    let dec = get("dec", -90.0, 90.0)?;
    let width_mas = (get("width", 0.0, 360.0)? * f64::from(MAS_PER_DEGREE)).round() as u32;
    let height_mas = (get("height", 0.0, 180.0)? * f64::from(MAS_PER_DEGREE)).round() as u32;
    if width_mas == 0 || height_mas == 0 {
        return Err(Error::InvalidRegion);
    }
    let rotation = map.get("rotation").map_or(Ok(0), |v| {
        angle_mas(json::number(v).ok_or(Error::InvalidRegion)?)
    })?;
    Ok(Region {
        icrs_ra_mas: angle_mas(ra)?,
        icrs_dec_mas: (dec * f64::from(MAS_PER_DEGREE)).round() as i32,
        width_mas,
        height_mas,
        position_angle_mas: rotation,
    })
}
