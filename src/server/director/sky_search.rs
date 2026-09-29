//! Name search for the framing view: the local Seiza object catalog first
//! (prefix matches over designations, common names and aliases), then CDS
//! Sesame for what the catalog does not know. Sesame's answers, hits and
//! misses alike, are kept on disk so a name goes online once.

use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Query, State},
    Json,
};
use seiza::objects::{ObjectCatalog, ObjectNameMatch};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    enabled,
    sky_image::{self, Resolved},
    Error,
};
use crate::server::{api::ApiResponse, state::AppState};

const MAX_QUERY_CHARS: usize = 64;
const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 25;
/// A hit is a catalog position and does not go stale. A miss may: the
/// catalogs grow and typos get fixed, so a miss is asked again after a day.
const HIT_MEMORY: Duration = Duration::from_secs(90 * 24 * 3600);
const MISS_MEMORY: Duration = Duration::from_secs(24 * 3600);
pub(super) const LOCAL_SOURCE: &str = "Seiza object catalog";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchQuery {
    q: String,
    #[serde(default = "default_limit")]
    limit: usize,
    /// Also ask Sesame (through the cache). Off while a name is being
    /// typed; on when the user asks for the name as typed.
    #[serde(default)]
    online: bool,
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

/// One name the local catalog knows, with the designation or alias that
/// matched what was typed.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub(super) struct SearchHit {
    pub name: String,
    pub common_name: String,
    pub kind: &'static str,
    pub ra_degrees: f64,
    pub dec_degrees: f64,
    pub matched: String,
    pub source: &'static str,
}

#[derive(Serialize)]
pub(super) struct LocalHits {
    /// Whether the object catalog is on this server.
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub items: Vec<SearchHit>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum OnlineState {
    /// Not asked: the request did not say `online`.
    Skipped,
    Hit,
    Miss,
    Failed,
}

#[derive(Serialize)]
pub(super) struct SearchAnswer {
    pub query: String,
    pub local: LocalHits,
    pub online: Option<Resolved>,
    pub online_state: OnlineState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online_note: Option<String>,
    /// Whether the online answer came from the cache rather than Sesame.
    pub online_cached: bool,
}

/// Letters and digits only, upper-cased, so "ngc7000", "NGC 7000" and
/// "N.G.C. 7000" are one name.
fn compact(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

fn hit_from(matched: ObjectNameMatch) -> SearchHit {
    let object = matched.object;
    SearchHit {
        name: object.name,
        common_name: object.common_name,
        kind: object.kind.as_str(),
        ra_degrees: object.ra,
        dec_degrees: object.dec,
        matched: matched.matched_name,
        source: LOCAL_SOURCE,
    }
}

/// The catalog's names starting with `query`: one row per object, an exact
/// match first, then the catalog's alphabetical order.
pub(super) fn local_hits(
    catalog: Result<Arc<ObjectCatalog>, String>,
    query: &str,
    limit: usize,
) -> LocalHits {
    let catalog = match catalog {
        Ok(catalog) => catalog,
        Err(note) => {
            return LocalHits {
                available: false,
                note: Some(note),
                items: Vec::new(),
            };
        }
    };
    // Aliases make several rows per object; ask for more than shown.
    let matches = match catalog.search_names(query, limit.saturating_mul(4).max(limit)) {
        Ok(matches) => matches,
        Err(error) => {
            return LocalHits {
                available: true,
                note: Some(format!("Object catalog search failed: {error}")),
                items: Vec::new(),
            };
        }
    };
    let wanted = compact(query);
    let mut items: Vec<SearchHit> = Vec::new();
    for matched in matches {
        if items.iter().any(|item| item.name == matched.object.name) {
            continue;
        }
        items.push(hit_from(matched));
    }
    items.sort_by_key(|item| compact(&item.matched) != wanted);
    items.truncate(limit);
    LocalHits {
        available: true,
        note: None,
        items,
    }
}

/// The catalog's own answer for a name typed in full, when it has one.
pub(super) fn local_exact(
    catalog: Result<Arc<ObjectCatalog>, String>,
    query: &str,
) -> Option<Resolved> {
    let catalog = catalog.ok()?;
    let matched = catalog.lookup_name(query).ok()?.into_iter().next()?;
    Some(Resolved {
        query: query.to_owned(),
        name: matched.object.name,
        ra_degrees: matched.object.ra,
        dec_degrees: matched.object.dec,
        source: LOCAL_SOURCE.to_owned(),
    })
}

/// What Sesame said about a name, kept in memory and on disk under the
/// cache root, one small JSON file per name.
pub struct ResolverCache {
    dir: PathBuf,
    memory: Mutex<HashMap<String, Remembered>>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Remembered {
    query: String,
    at_unix: u64,
    hit: Option<Resolved>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl ResolverCache {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            memory: Mutex::new(HashMap::new()),
        }
    }

    /// Lower-cased with whitespace collapsed: the key one name shares
    /// however it is spaced or cased.
    fn key(query: &str) -> String {
        query
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }

    fn path(&self, key: &str) -> PathBuf {
        let digest = Sha256::digest(key.as_bytes());
        let mut name = String::with_capacity(40);
        for byte in &digest[..16] {
            name.push_str(&format!("{byte:02x}"));
        }
        name.push_str(".json");
        self.dir.join(name)
    }

    fn fresh(remembered: &Remembered, now: u64) -> bool {
        let memory = if remembered.hit.is_some() {
            HIT_MEMORY
        } else {
            MISS_MEMORY
        };
        now.saturating_sub(remembered.at_unix) <= memory.as_secs()
    }

    /// `Some(answer)` when the name was asked recently enough; the answer
    /// itself may be a miss.
    pub(super) fn lookup(&self, query: &str) -> Option<Option<Resolved>> {
        self.lookup_at(query, now_unix())
    }

    fn lookup_at(&self, query: &str, now: u64) -> Option<Option<Resolved>> {
        let key = Self::key(query);
        if let Some(remembered) = self.memory.lock().expect("resolver cache").get(&key)
            && Self::fresh(remembered, now)
        {
            return Some(remembered.hit.clone());
        }
        let text = fs::read_to_string(self.path(&key)).ok()?;
        let remembered: Remembered = serde_json::from_str(&text).ok()?;
        if remembered.query != key || !Self::fresh(&remembered, now) {
            return None;
        }
        let hit = remembered.hit.clone();
        self.memory
            .lock()
            .expect("resolver cache")
            .insert(key, remembered);
        Some(hit)
    }

    pub(super) fn remember(&self, query: &str, hit: Option<Resolved>) {
        self.remember_at(query, hit, now_unix());
    }

    fn remember_at(&self, query: &str, hit: Option<Resolved>, now: u64) {
        let key = Self::key(query);
        let remembered = Remembered {
            query: key.clone(),
            at_unix: now,
            hit,
        };
        if let Ok(text) = serde_json::to_vec(&remembered)
            && fs::create_dir_all(&self.dir).is_ok()
            && let Ok(mut file) = tempfile::NamedTempFile::new_in(&self.dir)
            && file.write_all(&text).is_ok()
        {
            // A failed rename leaves the last answer in place; the memory
            // copy still serves this process.
            let _ = file.persist(self.path(&key));
        }
        self.memory
            .lock()
            .expect("resolver cache")
            .insert(key, remembered);
    }
}

pub(super) async fn search(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<ApiResponse<SearchAnswer>>, Error> {
    enabled(&state)?;
    let text = query.q.trim().to_owned();
    if text.is_empty()
        || text.chars().count() > MAX_QUERY_CHARS
        || text.chars().any(char::is_control)
        || query.limit == 0
        || query.limit > MAX_LIMIT
    {
        return Err(Error::Invalid);
    }
    let astrometry = state.astrometry.clone();
    let typed = text.clone();
    let local = tokio::task::spawn_blocking(move || {
        local_hits(astrometry.object_catalog(), &typed, query.limit)
    })
    .await
    .map_err(|_| Error::Internal)?;
    let (online, online_state, online_note, online_cached) = if query.online {
        match sky_image::service(&state).resolve_remembered(&text).await {
            Ok((Some(hit), cached)) => (Some(hit), OnlineState::Hit, None, cached),
            Ok((None, cached)) => (None, OnlineState::Miss, None, cached),
            Err(note) => (None, OnlineState::Failed, Some(note), false),
        }
    } else {
        (None, OnlineState::Skipped, None, false)
    };
    Ok(Json(ApiResponse::success(SearchAnswer {
        query: text,
        local,
        online,
        online_state,
        online_note,
        online_cached,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza::objects::{ObjectKind, ObjectMetadata, SkyObject};

    fn object(kind: ObjectKind, name: &str, common: &str, aliases: &[&str], ra: f64) -> SkyObject {
        SkyObject {
            kind,
            ra,
            dec: 40.0,
            mag: None,
            major_arcmin: Some(10.0),
            minor_arcmin: None,
            position_angle_deg: None,
            name: name.to_owned(),
            common_name: common.to_owned(),
            metadata: ObjectMetadata {
                id: format!("test:{name}"),
                source: "test".to_owned(),
                aliases: aliases.iter().map(|a| (*a).to_owned()).collect(),
                parent_ids: Vec::new(),
                alternate_ids: Vec::new(),
                alternate_sources: Vec::new(),
            },
        }
    }

    fn catalog() -> Arc<ObjectCatalog> {
        Arc::new(ObjectCatalog::new(vec![
            object(
                ObjectKind::Nebula,
                "NGC 7000",
                "North America Nebula",
                &["Sh2-117", "C 20"],
                314.75,
            ),
            object(
                ObjectKind::Nebula,
                "NGC 7023",
                "Iris Nebula",
                &["C 4"],
                315.4,
            ),
            object(ObjectKind::Galaxy, "NGC 7", "", &[], 2.0),
            object(ObjectKind::Nebula, "IC 5070", "Pelican Nebula", &[], 312.75),
        ]))
    }

    #[test]
    fn local_search_lists_one_row_per_object_with_the_exact_name_first() {
        let hits = local_hits(Ok(catalog()), "ngc 7", 8);
        assert!(hits.available);
        let names: Vec<_> = hits.items.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names[0], "NGC 7", "the name typed in full comes first");
        assert!(names.contains(&"NGC 7000") && names.contains(&"NGC 7023"));
        assert_eq!(names.len(), 3, "aliases do not add rows: {names:?}");
        assert_eq!(hits.items[0].source, LOCAL_SOURCE);
        assert_eq!(hits.items[0].kind, "galaxy");

        // A common name and an alias find the object too, and say what matched.
        let by_common = local_hits(Ok(catalog()), "pelican", 8);
        assert_eq!(by_common.items[0].name, "IC 5070");
        assert_eq!(by_common.items[0].matched, "Pelican Nebula");
        let by_alias = local_hits(Ok(catalog()), "sh2-117", 8);
        assert_eq!(by_alias.items[0].name, "NGC 7000");
        assert_eq!(local_hits(Ok(catalog()), "ngc 7", 2).items.len(), 2);

        let missing = local_hits(Err("object catalog is not configured".to_owned()), "m", 8);
        assert!(!missing.available);
        assert!(missing.items.is_empty());
    }

    #[test]
    fn a_full_name_resolves_locally_before_anyone_goes_online() {
        let hit = local_exact(Ok(catalog()), "north america nebula").unwrap();
        assert_eq!(hit.name, "NGC 7000");
        assert_eq!(hit.source, LOCAL_SOURCE);
        assert!((hit.ra_degrees - 314.75).abs() < 1e-9);
        assert!(
            local_exact(Ok(catalog()), "ngc 70").is_none(),
            "a prefix is not a name"
        );
        assert!(local_exact(Err("none".to_owned()), "NGC 7000").is_none());
    }

    #[test]
    fn sesame_answers_are_remembered_on_disk_hits_for_long_and_misses_for_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResolverCache::new(&dir.path().join("sesame"));
        let hit = Resolved {
            query: "Heart Nebula".to_owned(),
            name: "IC 1805".to_owned(),
            ra_degrees: 38.2,
            dec_degrees: 61.45,
            source: "CDS Sesame".to_owned(),
        };
        let day = 24 * 3600;
        assert!(cache.lookup_at("Heart Nebula", 1_000).is_none());
        cache.remember_at("Heart  Nebula", Some(hit.clone()), 1_000);
        cache.remember_at("Nowhere", None, 1_000);
        // Spacing and case do not make a new name.
        assert_eq!(
            cache
                .lookup_at("heart nebula", 2_000)
                .unwrap()
                .unwrap()
                .name,
            "IC 1805"
        );
        assert_eq!(cache.lookup_at("nowhere", 2_000), Some(None));
        // A fresh process reads the files back.
        let again = ResolverCache::new(&dir.path().join("sesame"));
        assert_eq!(
            again
                .lookup_at("Heart Nebula", 1_000 + 30 * day)
                .unwrap()
                .unwrap()
                .ra_degrees,
            38.2
        );
        assert_eq!(again.lookup_at("Nowhere", 1_000 + 3600), Some(None));
        // A miss is asked again after a day; a hit lasts ninety.
        assert!(again.lookup_at("Nowhere", 1_000 + day + 1).is_none());
        assert!(again.lookup_at("Heart Nebula", 1_000 + 89 * day).is_some());
        assert!(again.lookup_at("Heart Nebula", 1_000 + 91 * day).is_none());
        assert_eq!(fs::read_dir(dir.path().join("sesame")).unwrap().count(), 2);
    }
}
