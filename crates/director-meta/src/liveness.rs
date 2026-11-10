//! Each rig's newest status and plugin contacts, kept in memory by the
//! store's writer as it commits them.
//!
//! Live and the header poll this every few seconds from every open page.
//! The writer is the only connection that writes, and it updates this
//! memory after each status or contact it commits, so no write can leave
//! it behind and a poll reads no database.

use crate::inbox::{Contact, ContactKind, RigStatus};
use crate::{parse_id, Error, MetaStore};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use uuid::Uuid;

#[derive(Default)]
struct Rigs {
    statuses: HashMap<Uuid, RigStatus>,
    contacts: HashMap<Uuid, Vec<Contact>>,
}

#[derive(Default)]
pub struct Liveness {
    rigs: Mutex<Rigs>,
    weather_reports: AtomicU64,
}

impl Liveness {
    fn rigs(&self) -> MutexGuard<'_, Rigs> {
        self.rigs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The rig's newest status, as [`MetaStore::rig_status`] would read it.
    pub fn status(&self, rig: Uuid) -> Option<RigStatus> {
        self.rigs().statuses.get(&rig).cloned()
    }

    /// The rig's contacts, as [`MetaStore::contacts_for_rig`] would read them.
    pub fn contacts(&self, rig: Uuid) -> Vec<Contact> {
        self.rigs().contacts.get(&rig).cloned().unwrap_or_default()
    }

    /// How many weather reports the writer has been given since the store
    /// opened, written or not. Rig weather judged at an older count may be
    /// out of date.
    pub fn weather_reports(&self) -> u64 {
        self.weather_reports.load(Ordering::Acquire)
    }

    pub(crate) fn status_written(&self, status: &RigStatus) {
        self.rigs().statuses.insert(status.rig_id, status.clone());
    }

    /// As in the store, an earlier call never replaces a later one.
    pub(crate) fn contact_written(
        &self,
        rig: Uuid,
        kind: ContactKind,
        at_ms: u64,
        detail: Option<&str>,
    ) {
        let mut rigs = self.rigs();
        let contacts = rigs.contacts.entry(rig).or_default();
        match contacts.iter_mut().find(|contact| contact.kind == kind) {
            Some(contact) if contact.at_ms > at_ms => {}
            Some(contact) => {
                contact.at_ms = at_ms;
                contact.detail = detail.map(str::to_owned);
            }
            None => contacts.push(Contact {
                rig_id: rig,
                kind,
                at_ms,
                detail: detail.map(str::to_owned),
            }),
        }
    }

    /// The rig's status and contacts were deleted with it.
    pub(crate) fn rig_removed(&self, rig: Uuid) {
        let mut rigs = self.rigs();
        rigs.statuses.remove(&rig);
        rigs.contacts.remove(&rig);
    }

    pub(crate) fn weather_reported(&self) {
        self.weather_reports.fetch_add(1, Ordering::AcqRel);
    }
}

impl MetaStore {
    /// Keep every rig's status and contacts in memory from now on, loading
    /// what the store holds. Call it on the writer, before any write.
    pub fn keep_liveness(&mut self) -> Result<Arc<Liveness>, Error> {
        let liveness = Liveness::default();
        {
            let mut rigs = liveness.rigs();
            for status in self.rig_statuses()? {
                rigs.statuses.insert(status.rig_id, status);
            }
            let mut statement = self.connection.prepare(
                "SELECT rig_id, kind, at_ms, detail FROM rig_contact ORDER BY rig_id, kind",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? as u64,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (rig, kind, at_ms, detail) in rows {
                let (Ok(rig_id), Some(kind)) = (parse_id(&rig), ContactKind::parse(&kind)) else {
                    continue;
                };
                rigs.contacts.entry(rig_id).or_default().push(Contact {
                    rig_id,
                    kind,
                    at_ms,
                    detail,
                });
            }
        }
        let liveness = Arc::new(liveness);
        self.liveness = Some(liveness.clone());
        Ok(liveness)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::ContactKind;

    #[test]
    fn keeps_what_the_writer_commits_and_reloads_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.sqlite");
        let mut store = MetaStore::create(&path).unwrap();
        let rig = Uuid::new_v4();
        store.create_rig(rig, "Askar").unwrap();
        let liveness = store.keep_liveness().unwrap();
        assert_eq!(liveness.status(rig), None);
        let status = RigStatus {
            rig_id: rig,
            session_id: "s".into(),
            reported_at_ms: 10,
            payload: serde_json::json!({ "phase": "exposing" }),
            received_at_ms: 11,
        };
        assert!(store.record_status(&status).unwrap());
        // A report the store refuses leaves memory alone.
        let older = RigStatus {
            reported_at_ms: 5,
            payload: serde_json::json!({ "phase": "old" }),
            ..status.clone()
        };
        assert!(!store.record_status(&older).unwrap());
        assert_eq!(liveness.status(rig), Some(status.clone()));
        store
            .record_contact(rig, ContactKind::Status, 11, Some("s"))
            .unwrap();
        store
            .record_contact(rig, ContactKind::Status, 5, Some("earlier"))
            .unwrap();
        assert_eq!(liveness.contacts(rig), store.contacts_for_rig(rig).unwrap());
        assert_eq!(liveness.contacts(rig)[0].at_ms, 11);
        // Reopened, the store's memory starts from what it holds.
        drop(store);
        let mut store = MetaStore::open(&path).unwrap();
        let reloaded = store.keep_liveness().unwrap();
        assert_eq!(reloaded.status(rig), Some(status));
        assert_eq!(reloaded.contacts(rig), store.contacts_for_rig(rig).unwrap());
        // Removing a rig deletes its status and contacts; memory follows.
        reloaded.rig_removed(rig);
        assert_eq!(reloaded.status(rig), None);
        assert!(reloaded.contacts(rig).is_empty());
    }

    #[test]
    fn removing_a_rig_forgets_its_status_and_contacts() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
        let liveness = store.keep_liveness().unwrap();
        let catalog = crate::CatalogIdentity {
            id: Uuid::new_v4(),
            origin_instance_id: store.instance_id(),
        };
        let rig = store
            .bind_catalog_rig_after(catalog, "Askar", true, || Ok(()))
            .unwrap()
            .rig
            .id;
        let status = RigStatus {
            rig_id: rig,
            session_id: "s".into(),
            reported_at_ms: 10,
            payload: serde_json::json!({ "phase": "exposing" }),
            received_at_ms: 11,
        };
        store.record_status(&status).unwrap();
        store
            .record_contact(rig, ContactKind::Status, 11, Some("s"))
            .unwrap();
        assert!(liveness.status(rig).is_some());
        assert!(store.remove_rig(catalog.id).unwrap().is_some());
        assert_eq!(liveness.status(rig), None);
        assert!(liveness.contacts(rig).is_empty());
    }
}
