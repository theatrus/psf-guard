//! The rigs Live lists, kept in memory.
//!
//! The header's Rigs tab, the Sky and Live poll every 15 seconds from every
//! open page. Reading the rig list from the store, and each rig database's
//! catalog identity from its file (often on a network share), on every poll
//! cost far more than the answer.
//!
//! This server is the only writer of the store and of the identities in the
//! rig files, so memory stays current without rereading on a timer:
//! - Each rig's status and contacts come from the writer's memory
//!   ([`psf_guard_director_meta::liveness`]), current with every write.
//! - Any other store write moves [`Service::planning_rows`], and the next
//!   poll reads the rig list again from the store, opening no rig file.
//! - Identities are read again only when the registered databases change or
//!   an identity was adopted ([`crate::catalog_identity::adoptions`]). They
//!   are read under the store's writer, which every adoption that can change
//!   an identity holds until its file is committed.

use super::*;
use psf_guard_director_meta::inbox::{Contact, RigStatus};
use std::sync::atomic::Ordering;

/// One rig with its newest status and contacts.
#[derive(Clone)]
pub(super) struct LiveRig {
    pub(super) rig: NamedIdentity,
    pub(super) catalog_slug: Option<String>,
    pub(super) catalog_name: Option<String>,
    pub(super) status: Option<RigStatus>,
    pub(super) contacts: Vec<Contact>,
}

/// The registered databases: slug, name and path, sorted.
type CatalogsKey = Vec<(String, String, String)>;

/// Each registered database's identity, with its slug and name.
#[derive(Clone)]
struct Identities {
    catalogs: CatalogsKey,
    adoptions: u64,
    list: Arc<Vec<(Uuid, String, String)>>,
}

/// A listed rig and the database it is bound to, if any.
#[derive(Clone)]
struct Listed {
    rig: NamedIdentity,
    catalog_slug: Option<String>,
    catalog_name: Option<String>,
}

struct Snapshot {
    catalogs: CatalogsKey,
    planning_rows: u64,
    rigs: Arc<Vec<Listed>>,
}

#[derive(Default)]
struct Kept {
    identities: Option<Identities>,
    snapshot: Option<Snapshot>,
}

#[derive(Default)]
pub(super) struct LiveRigs {
    kept: Mutex<Kept>,
}

fn catalogs_key(catalogs: &[Arc<DatabaseContext>]) -> CatalogsKey {
    let mut key: Vec<_> = catalogs
        .iter()
        .map(|catalog| {
            (
                catalog.id.clone(),
                catalog.name.clone(),
                catalog.database_path.clone(),
            )
        })
        .collect();
    key.sort();
    key
}

impl LiveRigs {
    fn kept(&self) -> std::sync::MutexGuard<'_, Kept> {
        self.kept
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Service {
    /// Every bound rig, and every rig that has reported though its database
    /// binding is gone, with its newest status and contacts, sorted by name.
    pub(super) async fn live_rigs(
        self: Arc<Self>,
        catalogs: Vec<Arc<DatabaseContext>>,
    ) -> Result<Vec<LiveRig>, Error> {
        let listed = self.clone().listed_rigs(catalogs).await?;
        Ok(listed
            .iter()
            .map(|entry| LiveRig {
                rig: entry.rig.clone(),
                catalog_slug: entry.catalog_slug.clone(),
                catalog_name: entry.catalog_name.clone(),
                status: self.liveness.status(entry.rig.id),
                contacts: self.liveness.contacts(entry.rig.id),
            })
            .collect())
    }

    async fn listed_rigs(
        self: Arc<Self>,
        catalogs: Vec<Arc<DatabaseContext>>,
    ) -> Result<Arc<Vec<Listed>>, Error> {
        let key = catalogs_key(&catalogs);
        // Read before the store, so a write that lands while it is read
        // leaves the count behind and the next poll reads again.
        let planning_rows = self.planning_rows.load(Ordering::Acquire);
        let adoptions = crate::catalog_identity::adoptions();
        let identities = {
            let kept = self.live_rigs.kept();
            let identities = kept.identities.clone().filter(|identities| {
                identities.catalogs == key && identities.adoptions == adoptions
            });
            if let Some(snapshot) = kept.snapshot.as_ref().filter(|snapshot| {
                snapshot.catalogs == key && snapshot.planning_rows == planning_rows
            }) && identities.is_some()
            {
                return Ok(snapshot.rigs.clone());
            }
            identities
        };
        let identities = match identities {
            Some(identities) => identities,
            None => {
                let instance = self.instance_id;
                let catalogs_key = key.clone();
                let identities = self
                    .clone()
                    .with_writer(move |_store| {
                        let adoptions = crate::catalog_identity::adoptions();
                        let list = identified_catalogs(&catalogs, instance)
                            .iter()
                            .map(|(identity, catalog)| {
                                (identity.id, catalog.id.clone(), catalog.name.clone())
                            })
                            .collect();
                        Ok::<_, Error>(Identities {
                            catalogs: catalogs_key,
                            adoptions,
                            list: Arc::new(list),
                        })
                    })
                    .await?;
                self.live_rigs.kept().identities = Some(identities.clone());
                identities
            }
        };
        let list = identities.list.clone();
        let rigs = self
            .clone()
            .query(move |store| {
                let mut rigs: BTreeMap<Uuid, (Option<String>, Option<String>)> = BTreeMap::new();
                for (identity, slug, name) in list.iter() {
                    if let Some(binding) = store.catalog_rig(*identity)? {
                        rigs.insert(binding.rig.id, (Some(slug.clone()), Some(name.clone())));
                    }
                }
                // A rig that reported keeps its row after its binding goes.
                for status in store.rig_statuses()? {
                    rigs.entry(status.rig_id).or_insert((None, None));
                }
                let mut list = Vec::new();
                for (rig_id, (catalog_slug, catalog_name)) in rigs {
                    if let Some(rig) = store.rig(rig_id)? {
                        list.push(Listed {
                            rig,
                            catalog_slug,
                            catalog_name,
                        });
                    }
                }
                list.sort_by(|a, b| a.rig.name.cmp(&b.rig.name).then(a.rig.id.cmp(&b.rig.id)));
                Ok(list)
            })
            .await?;
        let rigs = Arc::new(rigs);
        self.live_rigs.kept().snapshot = Some(Snapshot {
            catalogs: key,
            planning_rows,
            rigs: rigs.clone(),
        });
        Ok(rigs)
    }
}
