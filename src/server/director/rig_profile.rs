//! Per-rig optics, site, horizon and limits, edited through the bound database
//! and reported by the plugin. A profile is planning input, never authority.

use super::*;
use crate::server::database_context::DatabaseContext;
use psf_guard_director_core::{
    optics::{FieldOfView, Optics, Rotation},
    program::Configuration,
    visibility::{Horizon, Site},
};
use psf_guard_director_meta::profile::{Limits, Reported, RigMode, RigProfile, SkyQuality, Source};
use rusqlite::{Connection, OpenFlags};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const HEADER_CANDIDATES: usize = 12;

#[derive(Serialize)]
pub(super) struct ProfileView {
    rig: NamedIdentity,
    profile: RigProfile,
    /// Computed by the shared core from the saved optics.
    field_of_view: Option<FieldOfView>,
    /// Header-derived suggestions for the form. Shown, never applied silently.
    defaults: Defaults,
    /// Who picks the rig's work, and what Director needs to run it.
    mode: ModeView,
    /// The rig's filters: its Director plugin's wheel, else the list typed
    /// in Rig setup. `None` when neither is known.
    filter_list: Option<psf_guard_director_meta::profile::RigFilters>,
}

#[derive(Default, Serialize)]
pub(super) struct Defaults {
    pub(super) optics: Option<Reported<Optics>>,
    pub(super) site: Option<Reported<Site>>,
    pub(super) field_of_view: Option<FieldOfView>,
    /// Filter names the database uses, in its exposure templates and frames,
    /// most used first.
    pub(super) filters: Vec<String>,
}

/// An operator's edit. Timestamps are stamped by the server.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Edit {
    expected_revision: u64,
    optics: Option<Edited<Optics>>,
    site: Option<Edited<Site>>,
    horizon: Option<Edited<Horizon>>,
    sky_quality: Option<Edited<SkyQuality>>,
    limits: Edited<Limits>,
    /// A registered Sync peer that holds this rig's database, or `null` when
    /// the rig executes from the database on this server.
    #[serde(default)]
    peer_id: Option<String>,
    /// Who picks the rig's work. Left out keeps the stored mode, `null`
    /// returns the rig to PSF Guard's guess, and a mode sets it.
    #[serde(default, deserialize_with = "present")]
    mode: Option<Option<RigMode>>,
    /// The rig's filter names, typed for a rig whose filters no Director
    /// plugin reports. Left out keeps the stored list, and `null` clears it.
    #[serde(default, deserialize_with = "present")]
    filters: Option<Option<Edited<Vec<String>>>>,
}

/// A field that is present, `null` included, as opposed to left out.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edited<T> {
    value: T,
    source: Source,
}

/// What the plugin knows about its profile. The tuple is checked before any
/// field is read; a mismatched rig cannot overwrite another rig's profile.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EquipmentReport {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    configuration: Configuration,
    #[serde(default)]
    filter_names: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    camera_defaults: Option<psf_guard_director_meta::profile::CameraDefaults>,
    optics: Optics,
    site: Option<Site>,
    horizon: Option<Horizon>,
    limits: Option<Limits>,
    reported_at_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Who picks the rig's work, and what Director needs to run it.
#[derive(Serialize)]
pub(super) struct ModeView {
    /// The mode set in Rig setup or by pairing; `None` while PSF Guard
    /// guesses.
    set: Option<RigMode>,
    /// The mode night plans use: the set one, or the guess.
    runtime: night_plans::Runtime,
    /// A Director plugin has paired with the rig.
    paired: bool,
    /// The plugin has reported the rig's equipment.
    equipment: bool,
}

fn view(
    store: &MetaStore,
    rig: NamedIdentity,
    profile: RigProfile,
    defaults: Defaults,
) -> Result<ProfileView, StoreError> {
    let field_of_view = profile
        .optics
        .as_ref()
        .and_then(|optics| optics.value.field_of_view().ok());
    let mode = ModeView {
        set: profile.mode,
        runtime: night_plans::runtime_of(store, rig.id, Some(&profile))?,
        paired: !store.clients(rig.id)?.is_empty(),
        equipment: profile.configuration.is_some(),
    };
    Ok(ProfileView {
        rig,
        filter_list: profile.known_filters(),
        profile,
        field_of_view,
        defaults,
        mode,
    })
}

pub(super) async fn get(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<ApiResponse<ProfileView>>, Error> {
    let service = enabled(&state)?;
    let catalog = state.get_database(&slug).ok_or(Error::Missing)?;
    let (identity, connection) = file_identity(&service, catalog.clone()).await?;
    let mut shown = service
        .clone()
        .with_reader(move |store| {
            let binding = store.catalog_rig(identity.id)?.ok_or(Error::Missing)?;
            let profile = store
                .rig_profile(binding.rig.id)?
                .unwrap_or_else(|| RigProfile::empty(binding.rig.id, now_ms()));
            Ok::<_, Error>(view(store, binding.rig, profile, Defaults::default())?)
        })
        .await?;
    // The first header search of a database scans its image folders; no
    // store gate is held while it runs.
    shown.defaults = blocking(move || {
        let mut defaults = header_defaults(&catalog, &connection);
        defaults.filters = database_filters(&connection);
        defaults
    })
    .await?;
    Ok(Json(ApiResponse::success(shown)))
}

pub(super) async fn put(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(edit): Json<Edit>,
) -> Result<Json<ApiResponse<ProfileView>>, Error> {
    let service = enabled(&state)?;
    let catalog = state.get_database(&slug).ok_or(Error::Missing)?;
    // Only a peer this server knows can be named; the select in the browser
    // offers exactly those, so anything else is a stale or forged request.
    // The peers live in the registry file, read off the async workers.
    if let Some(peer) = edit.peer_id.clone() {
        let state = state.clone();
        let known = blocking(move || {
            crate::server::peers::registered_peers(&state)
                .iter()
                .any(|entry| entry.id == peer)
        })
        .await?;
        if !known {
            return Err(Error::Invalid);
        }
    }
    for source in [
        edit.optics.as_ref().map(|part| &part.source),
        edit.site.as_ref().map(|part| &part.source),
        edit.horizon.as_ref().map(|part| &part.source),
        edit.sky_quality.as_ref().map(|part| &part.source),
        Some(&edit.limits.source),
        edit.filters
            .as_ref()
            .and_then(Option::as_ref)
            .map(|part| &part.source),
    ]
    .into_iter()
    .flatten()
    {
        if matches!(source, Source::Plugin {}) {
            return Err(Error::Invalid);
        }
    }
    let (identity, _) = file_identity(&service, catalog).await?;
    service
        .run(move |store| {
            let rig = store
                .catalog_rig(identity.id)?
                .ok_or(StoreError::NotFound)?
                .rig;
            let now = now_ms();
            let stored = store.rig_profile(rig.id)?;
            let mut next = stored
                .clone()
                .unwrap_or_else(|| RigProfile::empty(rig.id, now));
            let previous = stored.as_ref();
            next.optics = stamp(edit.optics, previous.and_then(|p| p.optics.as_ref()), now);
            next.site = stamp(edit.site, previous.and_then(|p| p.site.as_ref()), now);
            next.horizon = stamp(edit.horizon, previous.and_then(|p| p.horizon.as_ref()), now);
            next.sky_quality = stamp(
                edit.sky_quality,
                previous.and_then(|p| p.sky_quality.as_ref()),
                now,
            );
            next.limits = stamp(Some(edit.limits), previous.map(|p| &p.limits), now)
                .expect("limits are always present");
            next.peer_id = edit.peer_id;
            if let Some(mode) = edit.mode {
                next.mode = mode;
            }
            if let Some(filters) = edit.filters {
                let filters = filters.map(|mut part| {
                    part.value = typed_filters(part.value);
                    part
                });
                next.filters = stamp(
                    filters.filter(|part| !part.value.is_empty()),
                    previous.and_then(|p| p.filters.as_ref()),
                    now,
                );
            }
            next.updated_at_ms = now;
            let saved = store.save_rig_profile(&next, edit.expected_revision)?;
            view(store, rig, saved, Defaults::default())
        })
        .await
        .map(|value| Json(ApiResponse::success(value)))
}

/// Whether a typed list can store this filter name.
fn typed_filter_fits(name: &str) -> bool {
    name.len() <= psf_guard_director_meta::profile::MAX_FILTER_NAME_LEN
        && !name.chars().any(char::is_control)
}

/// Typed filter names as stored: trimmed, blanks dropped, and each name once
/// regardless of case, in the order typed.
fn typed_filters(names: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    names
        .into_iter()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty() && seen.insert(name.to_lowercase()))
        .collect()
}

/// Keep the old timestamp when nothing about the part changed, so a resave of
/// the form does not pretend a value was re-measured.
fn stamp<T: PartialEq>(
    edited: Option<Edited<T>>,
    previous: Option<&Reported<T>>,
    now: u64,
) -> Option<Reported<T>> {
    let edited = edited?;
    let reported_at_ms = match previous {
        Some(previous) if previous.value == edited.value && previous.source == edited.source => {
            previous.reported_at_ms
        }
        _ => now,
    };
    Some(Reported {
        value: edited.value,
        source: edited.source,
        reported_at_ms,
    })
}

pub(super) async fn report_equipment(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Json(report): Json<EquipmentReport>,
) -> Result<Json<ApiResponse<ProfileView>>, Error> {
    let service = enabled(&state)?;
    if report.coordinator_instance_id != service.instance_id
        || report.configuration.rig_id != rig.to_string()
    {
        return Err(Error::WrongRig);
    }
    service
        .run(move |store| {
            let binding = store.catalog_rig(report.catalog_id)?;
            let Some(binding) = binding.filter(|binding| binding.rig.id == rig) else {
                return Err(StoreError::NotFound);
            };
            let stored = store.rig_profile(rig)?;
            let revision = stored.as_ref().map_or(0, |p| p.revision);
            let mut next = stored.unwrap_or_else(|| RigProfile::empty(rig, report.reported_at_ms));
            let at = report.reported_at_ms;
            next.configuration = Some(from_plugin(report.configuration, at));
            next.filter_names = report.filter_names;
            next.camera_defaults = report.camera_defaults;
            next.optics = Some(from_plugin(report.optics, at));
            if let Some(site) = report.site {
                next.site = Some(from_plugin(site, at));
            }
            if let Some(horizon) = report.horizon {
                next.horizon = Some(from_plugin(horizon, at));
            }
            if let Some(limits) = report.limits {
                next.limits = from_plugin(limits, at);
            }
            next.updated_at_ms = now_ms();
            let saved = store.save_rig_profile(&next, revision)?;
            view(store, binding.rig, saved, Defaults::default())
        })
        .await
        .map_err(|error| match error {
            // The catalog exists but is bound to another rig, or to none.
            Error::Missing => Error::WrongRig,
            other => other,
        })
        .map(|value| Json(ApiResponse::success(value)))
}

fn from_plugin<T>(value: T, reported_at_ms: u64) -> Reported<T> {
    Reported {
        value,
        source: Source::Plugin {},
        reported_at_ms,
    }
}

/// The identity the database file carries, or the one it would be given,
/// read with no store gate held, and the read-only connection it was read on.
pub(super) async fn file_identity(
    service: &Service,
    catalog: Arc<DatabaseContext>,
) -> Result<(CatalogIdentity, Connection), Error> {
    let instance = service.instance_id;
    blocking(move || {
        let connection = super::super::database_context::open_scheduler_connection_with_flags(
            FilePath::new(&catalog.database_path),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(StoreError::from)?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(StoreError::from)?;
        let identity = crate::catalog_identity::read(&connection)?
            .unwrap_or_else(|| super::derived_identity(instance, &catalog.database_path));
        Ok((identity, connection))
    })
    .await?
}

/// The filter names a Target Scheduler database uses, in its exposure
/// templates and frames, most used first; each name once regardless of case.
/// Names a typed list could not store are left out.
fn database_filters(connection: &Connection) -> Vec<String> {
    // Name as first seen, and uses, by lower-case name.
    let mut counted: std::collections::HashMap<String, (String, i64)> = Default::default();
    // Either table may lack the column in an older or partial database.
    for table in ["exposuretemplate", "acquiredimage"] {
        let Ok(mut statement) = connection.prepare(&format!(
            "SELECT TRIM(filtername), COUNT(*) FROM {table}
             WHERE filtername IS NOT NULL AND TRIM(filtername) != ''
             GROUP BY TRIM(filtername)"
        )) else {
            continue;
        };
        let Ok(rows) = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        }) else {
            continue;
        };
        for (name, uses) in rows
            .filter_map(Result::ok)
            .filter(|(name, _)| typed_filter_fits(name))
        {
            counted
                .entry(name.to_lowercase())
                .or_insert_with(|| (name, 0))
                .1 += uses;
        }
    }
    let mut filters: Vec<(String, i64)> = counted.into_values().collect();
    filters.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    filters
        .into_iter()
        .take(psf_guard_director_meta::profile::MAX_TYPED_FILTERS)
        .map(|(name, _)| name)
        .collect()
}

/// Optics and site from the newest frames whose files can be found. One
/// header read per candidate, first usable file wins.
pub(super) fn header_defaults(catalog: &DatabaseContext, connection: &Connection) -> Defaults {
    let mut defaults = Defaults::default();
    // No frames, no reason to scan the image folders.
    let Some(names) = recent_file_names(connection).filter(|names| !names.is_empty()) else {
        return defaults;
    };
    let Ok(tree) = catalog.get_directory_tree() else {
        return defaults;
    };
    let now = now_ms();
    for name in names {
        // Catalogs written on Windows keep backslash paths; the tree is keyed
        // by basename either way.
        let base = name.rsplit(['/', '\\']).next().unwrap_or(name.as_str());
        if base.is_empty() {
            continue;
        }
        let Some(path) = tree.find_file_first(base) else {
            continue;
        };
        let Ok(headers) = seiza_image_io::read_header(path).map(|header| header.cards) else {
            continue;
        };
        let parsed = crate::astrometry_headers::FitsAstrometryHeaders::from_headers(&headers);
        let source = Source::FrameHeaders {
            file_name: base.to_owned(),
        };
        if defaults.optics.is_none()
            && let Some(optics) = optics_from(&parsed, &headers)
        {
            defaults.field_of_view = optics.field_of_view().ok();
            defaults.optics = Some(Reported {
                value: optics,
                source: source.clone(),
                reported_at_ms: now,
            });
        }
        if defaults.site.is_none()
            && let Some(observer) = &parsed.observer
        {
            let site = Site {
                latitude_degrees: observer.value.latitude_deg,
                longitude_degrees: observer.value.longitude_deg,
                elevation_meters: observer.value.altitude_m,
            };
            if site.validate().is_ok() {
                defaults.site = Some(Reported {
                    value: site,
                    source,
                    reported_at_ms: now,
                });
            }
        }
        if defaults.optics.is_some() && defaults.site.is_some() {
            break;
        }
    }
    defaults
}

/// Target Scheduler keeps the file name inside the metadata JSON, not in a
/// column, so read the newest rows and pull it out the way the sky view does.
fn recent_file_names(connection: &Connection) -> Option<Vec<String>> {
    let mut columns = connection
        .prepare("PRAGMA table_info(acquiredimage)")
        .ok()?;
    let columns: Vec<String> = columns
        .query_map([], |row| row.get::<_, String>(1))
        .ok()?
        .filter_map(Result::ok)
        .collect();
    let has = |name: &str| columns.iter().any(|c| c.eq_ignore_ascii_case(name));
    if !has("metadata") {
        return None;
    }
    let order = if has("acquireddate") {
        "acquireddate DESC, Id DESC"
    } else {
        "Id DESC"
    };
    let mut statement = connection
        .prepare(&format!(
            "SELECT metadata FROM acquiredimage WHERE metadata IS NOT NULL ORDER BY {order} LIMIT ?1"
        ))
        .ok()?;
    let names = statement
        .query_map([HEADER_CANDIDATES as i64], |row| row.get::<_, String>(0))
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|metadata| super::super::handlers::filename_from_metadata(&metadata))
        .collect();
    Some(names)
}

fn header_f64(headers: &[(String, seiza_fits::HeaderValue)], name: &str) -> Option<f64> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.as_f64())
        .filter(|value| value.is_finite() && *value > 0.0)
}

/// N.I.N.A. writes NAXIS after binning and XPIXSZ as the binned pitch, so
/// both fold back to the unbinned sensor here. Aperture comes from APTDIA or
/// FOCRATIO when either is present.
fn optics_from(
    parsed: &crate::astrometry_headers::FitsAstrometryHeaders,
    headers: &[(String, seiza_fits::HeaderValue)],
) -> Option<Optics> {
    let width = parsed.width.as_ref()?.value;
    let height = parsed.height.as_ref()?.value;
    let focal = parsed.focal_length_mm.as_ref()?;
    let pixel = parsed.pixel_size_um.as_ref()?;
    let binning = parsed
        .binning_x
        .as_ref()
        .map_or(1.0, |value| value.value)
        .max(1.0);
    let pixel_is_binned = pixel
        .sources
        .first()
        .is_some_and(|source| source.eq_ignore_ascii_case("XPIXSZ"));
    let pixel_size_um = if pixel_is_binned {
        pixel.value / binning
    } else {
        pixel.value
    };
    let scale = |px: u32| (f64::from(px) * binning).round() as u32;
    let aperture_mm = header_f64(headers, "APTDIA")
        .or_else(|| header_f64(headers, "FOCRATIO").map(|ratio| focal.value / ratio));
    let optics = Optics {
        sensor_width_px: scale(width),
        sensor_height_px: scale(height),
        pixel_size_um,
        focal_length_mm: focal.value,
        aperture_mm,
        rotation: Rotation::Manual { angle_degrees: 0.0 },
    };
    optics.validate().ok().map(|()| optics)
}
