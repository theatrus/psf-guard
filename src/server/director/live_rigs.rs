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
//! - Identities are read again only when the registered databases change,
//!   an identity was adopted ([`crate::catalog_identity::adoptions`]), or a
//!   file could not be read last time. They are read holding the discovery
//!   permit, which every adoption holds until its file is committed, so a
//!   read never sees an adoption half done; writes to the store do not wait.

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
    /// The adoption count of the identities the list was read with.
    adoptions: u64,
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
        let current = |kept: &Kept| {
            let adoptions = crate::catalog_identity::adoptions();
            kept.identities.clone().filter(|identities| {
                identities.catalogs == key && identities.adoptions == adoptions
            })
        };
        let identities = {
            let kept = self.live_rigs.kept();
            let identities = current(&kept);
            if let Some(identities) = &identities
                && let Some(snapshot) = kept.snapshot.as_ref().filter(|snapshot| {
                    snapshot.catalogs == key
                        && snapshot.planning_rows == planning_rows
                        && snapshot.adoptions == identities.adoptions
                })
            {
                return Ok(snapshot.rigs.clone());
            }
            identities
        };
        let (identities, keep) = match identities {
            Some(identities) => (identities, true),
            None => {
                // Every adoption holds this permit until its file is
                // committed. Without it (busy), read anyway and keep nothing.
                let permit = admit(&self.discovery_admission).await.ok();
                let waited = permit
                    .as_ref()
                    .and_then(|_| current(&self.live_rigs.kept()));
                match waited {
                    // Another poll read them while this one waited.
                    Some(identities) => (identities, true),
                    None => {
                        let instance = self.instance_id;
                        let catalogs_key = key.clone();
                        let (identities, all_read) = blocking(move || {
                            let adoptions = crate::catalog_identity::adoptions();
                            let (found, all_read) = identified_catalogs_read(&catalogs, instance);
                            let list = found
                                .iter()
                                .map(|(identity, catalog)| {
                                    (identity.id, catalog.id.clone(), catalog.name.clone())
                                })
                                .collect();
                            (
                                Identities {
                                    catalogs: catalogs_key,
                                    adoptions,
                                    list: Arc::new(list),
                                },
                                all_read,
                            )
                        })
                        .await?;
                        // A file that could not be read stands under its
                        // derived identity; read it again next time.
                        let keep = all_read && permit.is_some();
                        if keep {
                            self.live_rigs.kept().identities = Some(identities.clone());
                        }
                        (identities, keep)
                    }
                }
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
        if keep {
            self.live_rigs.kept().snapshot = Some(Snapshot {
                catalogs: key,
                planning_rows,
                adoptions: identities.adoptions,
                rigs: rigs.clone(),
            });
        }
        Ok(rigs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_cannot_be_read_is_not_taken_as_read() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.sqlite");
        let mut catalog =
            DatabaseContext::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        catalog.database_path = missing.to_string_lossy().into_owned();
        let catalog = Arc::new(catalog);
        let (found, all_read) = identified_catalogs_read(&[catalog], Uuid::new_v4());
        assert!(!all_read);
        // It still counts, under its derived identity, for this poll.
        assert_eq!(found.iter().count(), 1);
    }
}
