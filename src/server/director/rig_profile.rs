//! Per-rig optics, site, horizon and limits, edited through the bound database
//! and reported by the plugin. A profile is planning input, never authority.

use super::*;
use crate::server::database_context::DatabaseContext;
use psf_guard_director_core::{
    optics::{FieldOfView, Optics, Rotation},
    program::Configuration,
    visibility::{Horizon, Site},
};
use psf_guard_director_meta::profile::{Limits, Reported, RigProfile, SkyQuality, Source};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
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
}

#[derive(Default, Serialize)]
pub(super) struct Defaults {
    pub(super) optics: Option<Reported<Optics>>,
    pub(super) site: Option<Reported<Site>>,
    pub(super) field_of_view: Option<FieldOfView>,
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

fn view(rig: NamedIdentity, profile: RigProfile, defaults: Defaults) -> ProfileView {
    let field_of_view = profile
        .optics
        .as_ref()
        .and_then(|optics| optics.value.field_of_view().ok());
    ProfileView {
        rig,
        profile,
        field_of_view,
        defaults,
    }
}

pub(super) async fn get(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<ApiResponse<ProfileView>>, Error> {
    let service = enabled(&state)?;
    let catalog = state.get_database(&slug).ok_or(Error::Missing)?;
    run_bound(service, catalog, move |store, catalog, connection, rig| {
        let profile = store
            .rig_profile(rig.id)?
            .unwrap_or_else(|| RigProfile::empty(rig.id, now_ms()));
        let defaults = header_defaults(catalog, connection);
        Ok(view(rig, profile, defaults))
    })
    .await
    .map(|value| Json(ApiResponse::success(value)))
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
    if let Some(peer) = &edit.peer_id
        && !crate::server::peers::registered_peers(&state)
            .iter()
            .any(|entry| &entry.id == peer)
    {
        return Err(Error::Invalid);
    }
    for source in [
        edit.optics.as_ref().map(|part| &part.source),
        edit.site.as_ref().map(|part| &part.source),
        edit.horizon.as_ref().map(|part| &part.source),
        edit.sky_quality.as_ref().map(|part| &part.source),
        Some(&edit.limits.source),
    ]
    .into_iter()
    .flatten()
    {
        if matches!(source, Source::Plugin {}) {
            return Err(Error::Invalid);
        }
    }
    run_bound(service, catalog, move |store, _, _, rig| {
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
        next.updated_at_ms = now;
        let saved = store.save_rig_profile(&next, edit.expected_revision)?;
        Ok(view(rig, saved, Defaults::default()))
    })
    .await
    .map(|value| Json(ApiResponse::success(value)))
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
            Ok(view(binding.rig, saved, Defaults::default()))
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

/// Resolve the database's bound rig under both admission permits, then run
/// the operation with the meta store, the catalog and its read-only connection.
async fn run_bound<T: Send + 'static>(
    service: Arc<Service>,
    catalog: Arc<DatabaseContext>,
    operation: impl FnOnce(&mut MetaStore, &DatabaseContext, &Connection, NamedIdentity) -> Result<T, Error>
        + Send
        + 'static,
) -> Result<T, Error> {
    service
        .clone()
        .with_writer(move |store| {
            let mut connection =
                super::super::database_context::open_scheduler_connection_with_flags(
                    FilePath::new(&catalog.database_path),
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .map_err(StoreError::from)
                .map_err(Error::from)?;
            connection
                .busy_timeout(Duration::from_secs(2))
                .map_err(StoreError::from)
                .map_err(Error::from)?;
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(StoreError::from)
                .map_err(Error::from)?;
            let identity = crate::catalog_identity::read(&tx)?.unwrap_or_else(|| {
                super::derived_identity(service.instance_id, &catalog.database_path)
            });
            let binding = store.catalog_rig(identity.id)?.ok_or(Error::Missing)?;
            operation(store, &catalog, &tx, binding.rig)
        })
        .await
}

/// Optics and site from the newest frames whose files can be found. One
/// header read per candidate, first usable file wins.
pub(super) fn header_defaults(catalog: &DatabaseContext, connection: &Connection) -> Defaults {
    let Some(names) = recent_file_names(connection) else {
        return Defaults::default();
    };
    let Ok(tree) = catalog.get_directory_tree() else {
        return Defaults::default();
    };
    let now = now_ms();
    let mut defaults = Defaults::default();
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
        let Ok(headers) = crate::image_io::read_header(path) else {
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
