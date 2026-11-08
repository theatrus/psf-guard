//! The N.I.N.A. plugins that serve a database, as far as the server knows
//! them: whether a Sync plugin is paired, which N.I.N.A. profile it last
//! reported, what it supports, and when each paired client last called.
//!
//! The Sync and Director plugins keep separate protocols and credentials.
//! This module keeps the rules that decide how PSF Guard treats a rig's Sync
//! plugin in one place: the export path, activation and Settings all read
//! them from here.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::server::{
    database_context::DatabaseContext,
    remote_sync::{seal, BundleTable, CatalogBundle, SyncOperation, WireValue, WireValueKind},
};

/// The Sync plugin's release version, sent with every request. The server
/// logs it; behavior follows [`PLUGIN_CAPABILITIES_HEADER`], not the
/// version, since a local build reports `0.1.0.0`.
pub const PLUGIN_VERSION_HEADER: &str = "x-psf-guard-plugin-version";
/// What the Sync plugin supports, comma-separated, sent with every request.
/// A request without it comes from a plugin older than the header.
pub const PLUGIN_CAPABILITIES_HEADER: &str = "x-psf-guard-plugin-capabilities";
/// A plugin capability: the plugin writes rows on PSF Guard's waiting
/// profile ([`psf_guard_director_meta::SYNC_PLACED_PROFILE`]) into the
/// N.I.N.A. profile it runs in. An older plugin copies the ID as sent, and
/// N.I.N.A. then hides the rows, so the server gives them a real profile
/// first (see [`place_waiting_rows`]).
pub const PLUGIN_WAITING_PROFILE_CAPABILITY: &str = "waiting_profile_v1";

/// Whether the requesting plugin lists `capability` in
/// [`PLUGIN_CAPABILITIES_HEADER`].
pub(crate) fn plugin_supports(headers: &HeaderMap, capability: &str) -> bool {
    headers
        .get_all(PLUGIN_CAPABILITIES_HEADER)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|listed| listed.trim().eq_ignore_ascii_case(capability))
}

/// Whether a Sync plugin can reach this database, and so write its plans into
/// the N.I.N.A. profile it runs in.
pub(crate) fn sync_plugin_paired(context: &DatabaseContext) -> bool {
    context
        .remote_image_upload
        .as_ref()
        .is_some_and(|config| config.sync_enabled && config.token_is_configured())
}

/// The N.I.N.A. profile a plugin paired with `catalog_id` last reported,
/// from its star-detection settings.
pub(crate) fn reported_profile(catalog_id: &str) -> Option<String> {
    crate::star_detection::current()
        .plugin_profile(catalog_id)
        .map(str::to_owned)
}

/// How an export treats rows on PSF Guard's waiting profile for the plugin
/// asking: `None` when they go out as they are, else the profile to give
/// them. A plugin that cannot place waiting rows gets them under a real
/// profile: the one it reported, else the catalog's only one.
pub(crate) struct WaitingRows {
    reported: Option<String>,
    plugin_version: String,
}

impl WaitingRows {
    pub(crate) fn for_export(
        headers: &HeaderMap,
        operation: SyncOperation,
        catalog_id: &str,
    ) -> Option<Self> {
        (operation != SyncOperation::PushGrades
            && !plugin_supports(headers, PLUGIN_WAITING_PROFILE_CAPABILITY))
        .then(|| Self {
            reported: reported_profile(catalog_id),
            plugin_version: plugin_version(headers).unwrap_or("not sent").to_owned(),
        })
    }

    /// Rewrite the bundle's waiting rows, and log what that did.
    pub(crate) fn place(&self, bundle: &mut CatalogBundle, catalog_id: &str) -> anyhow::Result<()> {
        let plugin_version = &self.plugin_version;
        match place_waiting_rows(bundle, self.reported.as_deref())? {
            Placement::Placed(profile) => tracing::info!(
                catalog = %catalog_id, %profile, %plugin_version,
                "Gave waiting rows a N.I.N.A. profile for a Sync plugin that cannot place them"
            ),
            Placement::Unknown => tracing::warn!(
                catalog = %catalog_id, %plugin_version,
                "Rows wait for a N.I.N.A. profile, and this Sync plugin cannot place them; \
                 N.I.N.A. hides them until the plugin is updated"
            ),
            Placement::NothingWaiting => {}
        }
        Ok(())
    }
}

/// The version a request's [`PLUGIN_VERSION_HEADER`] reports.
fn plugin_version(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(PLUGIN_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|version| !version.is_empty())
}

/// Every capability a request lists in [`PLUGIN_CAPABILITIES_HEADER`],
/// lower case, sorted, once each.
fn plugin_capabilities(headers: &HeaderMap) -> Vec<String> {
    let mut listed: Vec<String> = headers
        .get_all(PLUGIN_CAPABILITIES_HEADER)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|listed| listed.trim().to_ascii_lowercase())
        .filter(|listed| !listed.is_empty())
        .collect();
    listed.sort();
    listed.dedup();
    listed
}

/// A client's last authenticated Sync request.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SyncContact {
    /// Unix seconds.
    pub at: i64,
    /// What [`PLUGIN_VERSION_HEADER`] reported; `None` when it was not sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// What [`PLUGIN_CAPABILITIES_HEADER`] listed.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl SyncContact {
    /// The client writes rows on the waiting profile into its own N.I.N.A.
    /// profile, so new projects need no placing on the server.
    pub fn places_new_projects(&self) -> bool {
        self.capabilities
            .iter()
            .any(|listed| listed == PLUGIN_WAITING_PROFILE_CAPABILITY)
    }
}

/// Catalog ID, then credential holder (a client ID or
/// [`crate::db_registry::LEGACY_KEY_HOLDER`]), to its last contact.
type Contacts = BTreeMap<String, BTreeMap<String, SyncContact>>;

/// Rewrite the file at most this often when only the time changed.
const CONTACT_WRITE_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// When each paired client last called, with the version and capabilities
/// it reported. Kept in memory and in a small file below the cache folder,
/// so Settings can show it after a restart. Losing the file loses only this
/// history.
pub struct SyncContacts {
    /// `None` keeps the contacts in memory only.
    path: Option<PathBuf>,
    state: Mutex<ContactState>,
    /// Serializes file writes, and holds the generation last written so an
    /// older snapshot never replaces a newer one.
    written: Arc<Mutex<u64>>,
}

#[derive(Default)]
struct ContactState {
    contacts: Contacts,
    /// Bumped for each snapshot sent to the file.
    generation: u64,
    last_write: Option<Instant>,
}

impl SyncContacts {
    pub fn path_in(cache_root: impl AsRef<Path>) -> PathBuf {
        cache_root.as_ref().join("sync-plugin-contacts.json")
    }

    pub fn new(cache_root: impl AsRef<Path>) -> Self {
        let path = Self::path_in(cache_root);
        let contacts = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            state: Mutex::new(ContactState {
                contacts,
                ..ContactState::default()
            }),
            written: Arc::default(),
        }
    }

    /// Contacts that are never written to disk.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            state: Mutex::default(),
            written: Arc::default(),
        }
    }

    /// Record an authenticated request from `holder` to `catalog_id`.
    /// `current(catalog, holder)` says whether a recorded credential still
    /// exists; the others are dropped when the file is next written. The
    /// file is written off the request path.
    pub(crate) fn note(
        &self,
        catalog_id: &str,
        holder: &str,
        headers: &HeaderMap,
        current: impl Fn(&str, &str) -> bool,
    ) {
        let contact = SyncContact {
            at: chrono::Utc::now().timestamp(),
            version: plugin_version(headers).map(str::to_owned),
            capabilities: plugin_capabilities(headers),
        };
        let snapshot = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            let previous = state
                .contacts
                .entry(catalog_id.to_owned())
                .or_default()
                .insert(holder.to_owned(), contact.clone());
            let changed = previous.is_none_or(|previous| {
                previous.version != contact.version || previous.capabilities != contact.capabilities
            });
            if !changed
                && state
                    .last_write
                    .is_some_and(|at| at.elapsed() < CONTACT_WRITE_INTERVAL)
            {
                return;
            }
            state.last_write = Some(Instant::now());
            state.contacts.retain(|catalog, holders| {
                holders.retain(|holder, _| current(catalog, holder));
                !holders.is_empty()
            });
            state.generation += 1;
            (state.generation, state.contacts.clone())
        };
        let Some(path) = self.path.clone() else {
            return;
        };
        let written = self.written.clone();
        let write = move || {
            let (generation, contacts) = snapshot;
            let Ok(mut last) = written.lock() else {
                return;
            };
            if *last >= generation {
                return;
            }
            match write_atomically(&path, &contacts) {
                Ok(()) => *last = generation,
                Err(error) => tracing::debug!(%error, "Sync plugin contacts were not saved"),
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(write);
            }
            Err(_) => write(),
        }
    }

    /// The last contact of each credential holder for `catalog_id`.
    pub fn for_catalog(&self, catalog_id: &str) -> BTreeMap<String, SyncContact> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.contacts.get(catalog_id).cloned())
            .unwrap_or_default()
    }
}

fn write_atomically(path: &Path, contacts: &Contacts) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(contacts)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// What [`place_waiting_rows`] did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    NothingWaiting,
    /// The waiting rows now carry this profile.
    Placed(String),
    /// No profile is known, so the rows stay on the waiting profile.
    Unknown,
}

/// Give rows on PSF Guard's waiting profile a real N.I.N.A. profile, for a
/// plugin that copies `profileId` as sent: `reported`, the profile the
/// plugin last reported, else the bundle's only other profile. Keeps the
/// bundle's spelling of that profile, and seals the bundle again.
pub(crate) fn place_waiting_rows(
    bundle: &mut CatalogBundle,
    reported: Option<&str>,
) -> anyhow::Result<Placement> {
    use psf_guard_director_meta::SYNC_PLACED_PROFILE;
    let profile_column = |table: &BundleTable| {
        table
            .columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case("profileId"))
    };
    let waiting = |value: &WireValue| {
        value
            .value
            .as_deref()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case(SYNC_PLACED_PROFILE))
    };
    let mut any_waiting = false;
    let mut others: BTreeMap<String, String> = BTreeMap::new();
    for table in bundle.tables.values() {
        let Some(column) = profile_column(table) else {
            continue;
        };
        for row in &table.rows {
            let value = &row.values[column];
            if waiting(value) {
                any_waiting = true;
            } else if let Some(profile) = value
                .value
                .as_deref()
                .map(str::trim)
                .filter(|profile| !profile.is_empty())
            {
                others
                    .entry(profile.to_lowercase())
                    .or_insert_with(|| profile.to_owned());
            }
        }
    }
    if !any_waiting {
        return Ok(Placement::NothingWaiting);
    }
    let profile = match reported {
        Some(reported) => Some(
            others
                .get(&reported.to_lowercase())
                .cloned()
                .unwrap_or_else(|| reported.to_owned()),
        ),
        None if others.len() == 1 => others.into_values().next(),
        None => None,
    };
    let Some(profile) = profile else {
        return Ok(Placement::Unknown);
    };
    for table in bundle.tables.values_mut() {
        let Some(column) = profile_column(table) else {
            continue;
        };
        for row in &mut table.rows {
            if waiting(&row.values[column]) {
                row.values[column] = WireValue {
                    kind: WireValueKind::Text,
                    value: Some(profile.clone()),
                };
            }
        }
    }
    seal(bundle)?;
    Ok(Placement::Placed(profile))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::remote_sync::{
        digest_hex, BundleColumn, BundleRow, CatalogIdentity, PROTOCOL_VERSION,
    };

    fn waiting_bundle(profiles: &[&str]) -> CatalogBundle {
        let column = |name: &str| BundleColumn {
            name: name.into(),
            declared_type: "TEXT".into(),
            not_null: false,
            primary_key: false,
        };
        let text = |value: &str| WireValue {
            kind: WireValueKind::Text,
            value: Some(value.into()),
        };
        let table = BundleTable {
            columns: vec![column("Id"), column("profileId")],
            rows: profiles
                .iter()
                .enumerate()
                .map(|(id, profile)| BundleRow {
                    values: vec![text(&id.to_string()), text(profile)],
                })
                .collect(),
        };
        CatalogBundle {
            protocol_version: PROTOCOL_VERSION,
            bundle_id: "bundle".into(),
            created_at_utc: "2026-11-07T00:00:00Z".into(),
            operation: SyncOperation::PushPlanning,
            source: CatalogIdentity {
                id: "rig".into(),
                product: "PSF Guard".into(),
                product_version: "test".into(),
                schema_version: 23,
            },
            tables: [
                ("project".to_string(), table.clone()),
                ("exposuretemplate".to_string(), table),
            ]
            .into(),
            payload_sha256: Some("stale".into()),
        }
    }

    fn profiles_in(bundle: &CatalogBundle) -> Vec<String> {
        bundle.tables["project"]
            .rows
            .iter()
            .map(|row| row.values[1].value.clone().unwrap())
            .collect()
    }

    #[test]
    fn waiting_rows_get_the_reported_profile_in_the_bundles_spelling() {
        const WAITING: &str = psf_guard_director_meta::SYNC_PLACED_PROFILE;
        let reducer = "9F1C0D2E-0000-4000-8000-00000000000A";
        let mut bundle = waiting_bundle(&[reducer, "profile-b", WAITING]);

        let placed = place_waiting_rows(&mut bundle, Some(&reducer.to_lowercase())).unwrap();

        assert_eq!(placed, Placement::Placed(reducer.into()));
        assert_eq!(profiles_in(&bundle), [reducer, "profile-b", reducer]);
        assert!(bundle.tables["exposuretemplate"]
            .rows
            .iter()
            .all(|row| row.values[1].value.as_deref() != Some(WAITING)));
        // Sealed again over the changed rows.
        let mut unsealed = bundle.clone();
        unsealed.payload_sha256 = None;
        assert_eq!(
            bundle.payload_sha256,
            Some(digest_hex(&serde_json::to_vec(&unsealed).unwrap()))
        );
    }

    #[test]
    fn waiting_rows_fall_back_to_the_only_profile_and_stay_when_none_is_known() {
        const WAITING: &str = psf_guard_director_meta::SYNC_PLACED_PROFILE;
        // A reported profile the bundle does not use is still the plugin's.
        let mut blank = waiting_bundle(&[WAITING]);
        assert_eq!(
            place_waiting_rows(&mut blank, Some("profile-new")).unwrap(),
            Placement::Placed("profile-new".into())
        );

        let mut single = waiting_bundle(&["profile-a", "PROFILE-A", WAITING]);
        assert_eq!(
            place_waiting_rows(&mut single, None).unwrap(),
            Placement::Placed("profile-a".into())
        );

        let mut two = waiting_bundle(&["profile-a", "profile-b", WAITING]);
        assert_eq!(
            place_waiting_rows(&mut two, None).unwrap(),
            Placement::Unknown
        );
        assert_eq!(profiles_in(&two)[2], WAITING);
        assert_eq!(two.payload_sha256.as_deref(), Some("stale"));

        let mut none = waiting_bundle(&["profile-a"]);
        assert_eq!(
            place_waiting_rows(&mut none, Some("profile-b")).unwrap(),
            Placement::NothingWaiting
        );
        assert_eq!(profiles_in(&none), ["profile-a"]);
    }

    #[test]
    fn plugin_capabilities_are_read_from_a_comma_separated_header() {
        let mut headers = HeaderMap::new();
        assert!(!plugin_supports(
            &headers,
            PLUGIN_WAITING_PROFILE_CAPABILITY
        ));
        headers.insert(
            PLUGIN_CAPABILITIES_HEADER,
            "something_v2, Waiting_Profile_V1".parse().unwrap(),
        );
        assert!(plugin_supports(&headers, PLUGIN_WAITING_PROFILE_CAPABILITY));
        assert!(!plugin_supports(&headers, "waiting_profile_v2"));
    }

    #[test]
    fn contacts_keep_each_clients_version_and_capabilities_across_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let contacts = SyncContacts::new(directory.path());
        let mut headers = HeaderMap::new();
        headers.insert(PLUGIN_VERSION_HEADER, "1.4.0.0".parse().unwrap());
        headers.insert(
            PLUGIN_CAPABILITIES_HEADER,
            "Waiting_Profile_V1, other_v2, waiting_profile_v1"
                .parse()
                .unwrap(),
        );
        let all = |_: &str, _: &str| true;
        contacts.note("rig", "client-a", &headers, all);
        contacts.note("rig", "remote-api-key", &HeaderMap::new(), all);
        contacts.note("gone", "client-b", &headers, all);
        // A write drops credentials and catalogs that no longer exist.
        contacts.note("rig", "client-c", &headers, |catalog, holder| {
            catalog == "rig" && holder != "client-c-revoked"
        });

        let reopened = SyncContacts::new(directory.path());
        let seen = reopened.for_catalog("rig");
        assert_eq!(seen["client-a"].version.as_deref(), Some("1.4.0.0"));
        assert_eq!(
            seen["client-a"].capabilities,
            ["other_v2", "waiting_profile_v1"]
        );
        assert!(seen["client-a"].places_new_projects());
        assert_eq!(seen["remote-api-key"].version, None);
        assert!(!seen["remote-api-key"].places_new_projects());
        assert!(reopened.for_catalog("other").is_empty());
        assert!(reopened.for_catalog("gone").is_empty());
        assert!(seen.contains_key("client-c"));
    }
}
