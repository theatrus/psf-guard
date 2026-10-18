//! Where a remote rig's activation stands at its site, and sending it again
//! once the site answers.
//!
//! A planning push carries every planning row in one rig database, so every
//! push to a peer goes through [`push_catalog`], which records how it went on
//! each activation still waiting for that database and peer. A peer that
//! answers a check, a sync or a push gets what still waits for it from
//! [`peer_reached`], in the background, each rig database once.

use super::activation::{send_planning, Push};
use super::*;
use crate::{db_registry::PeerEntry, server::peers::registered_peers};
use psf_guard_director_meta::activation::{
    Activation, RemotePush, MAX_PEER_NAME_CHARS, MAX_PUSH_ERROR_CHARS,
};
use std::collections::BTreeSet;
use tokio::task::JoinHandle;

/// A remote rig's push an activation is waiting on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Waiting {
    pub(super) project: Uuid,
    pub(super) revision: u64,
    pub(super) rig: Uuid,
}

/// Whether the rig's profile still sends its rows to this peer. A push to a
/// peer the rig no longer uses is moot: its next activation goes where
/// Setup points.
pub(super) fn sends_to(store: &MetaStore, rig: Uuid, peer: &str) -> Result<bool, StoreError> {
    Ok(store
        .rig_profile(rig)?
        .and_then(|profile| profile.peer_id)
        .is_some_and(|id| id == peer))
}

/// The rig database an activation wrote a rig's rows in.
pub(super) fn catalog_of(activation: &Activation, rig: Uuid) -> Option<Uuid> {
    activation
        .rigs
        .iter()
        .find(|r| r.rig_id == rig)
        .map(|r| r.catalog_id)
        .or_else(|| {
            activation
                .inactive_rigs
                .iter()
                .find(|r| r.rig_id == rig)
                .map(|r| r.catalog_id)
        })
}

/// Every push to this peer that has not landed, from the latest record of
/// each plan, on a rig that still sends there, with the rig's database.
fn waiting_for(store: &MetaStore, peer: &str) -> Result<Vec<(Waiting, Uuid)>, StoreError> {
    let mut waiting = Vec::new();
    for activation in store.pending_pushes(peer)? {
        for push in activation
            .pushes
            .iter()
            .filter(|p| p.peer_id == peer && !p.applied)
        {
            let Some(catalog) = catalog_of(&activation, push.rig_id) else {
                continue;
            };
            if sends_to(store, push.rig_id, peer)? {
                waiting.push((
                    Waiting {
                        project: activation.project_id,
                        revision: activation.revision,
                        rig: push.rig_id,
                    },
                    catalog,
                ));
            }
        }
    }
    Ok(waiting)
}

/// Send one rig database's planning rows to a peer and record how it went
/// on every activation waiting for that database and peer, and on `also`,
/// the rigs the caller pushes for.
pub(super) async fn push_catalog(
    state: &AppState,
    service: &Arc<Service>,
    context: &Arc<DatabaseContext>,
    peer: &PeerEntry,
    also: Vec<Waiting>,
) -> Push {
    send_and_record(state, service, context, peer, also, false)
        .await
        .expect("an unconditional push always sends")
        .0
}

/// [`push_catalog`], and with `only_if_waiting` nothing at all unless some
/// activation still waits for this database and peer. Answers the push and
/// the rigs it was recorded on.
async fn send_and_record(
    state: &AppState,
    service: &Arc<Service>,
    context: &Arc<DatabaseContext>,
    peer: &PeerEntry,
    also: Vec<Waiting>,
    only_if_waiting: bool,
) -> Option<(Push, Vec<Waiting>)> {
    // One planning push at a time, and none beside a Sync apply, so a pull
    // cannot rewrite the rows while the bundle is read. The outcome is
    // recorded under the same lock: a failed push recorded after a later
    // one that landed would undo it.
    let _guard = state.sync_apply_lock.lock().await;
    let instance = service.instance_id;
    let path = context.database_path.clone();
    let peer_id = peer.id.clone();
    // Read under the lock: an apply or a click may have sent it since, or a
    // newer activation replaced the record. Each record was written after
    // its rows committed here, so the bundle read below carries them.
    let waiting = service
        .clone()
        .query(move |store| {
            let catalog = identity_of(instance, &path).id;
            Ok(waiting_for(store, &peer_id)?
                .into_iter()
                .filter(|(_, on)| *on == catalog)
                .map(|(waiting, _)| waiting)
                .collect::<Vec<_>>())
        })
        .await;
    let mut waiting = match waiting {
        Ok(waiting) => waiting,
        Err(error) => {
            tracing::warn!(peer = %peer.id, catalog = %context.id, ?error, "Could not read which activations wait for a remote site");
            if only_if_waiting {
                return None;
            }
            vec![]
        }
    };
    if only_if_waiting && waiting.is_empty() {
        return None;
    }
    waiting.extend(also);
    waiting.sort();
    waiting.dedup();
    let push = send_planning(context, peer).await;
    let at = now_ms();
    let mut by_record: BTreeMap<(Uuid, u64), Vec<RemotePush>> = BTreeMap::new();
    for waiting in &waiting {
        by_record
            .entry((waiting.project, waiting.revision))
            .or_default()
            .push(outcome(&push, waiting.rig, at));
    }
    if !by_record.is_empty() {
        let recorded = service
            .clone()
            .with_writer(move |store| {
                for ((project, revision), pushes) in by_record {
                    store
                        .record_pushes(project, revision, &pushes)
                        .map_err(Error::from)?;
                }
                Ok::<_, Error>(())
            })
            .await;
        if let Err(error) = recorded {
            tracing::warn!(peer = %peer.id, catalog = %context.id, ?error, "A remote site's push could not be recorded");
        }
    }
    Some((push, waiting))
}

/// A push as the activation record keeps it.
fn outcome(push: &Push, rig: Uuid, at: u64) -> RemotePush {
    RemotePush {
        rig_id: rig,
        ts_sync: None,
        peer_id: push.peer_id.clone(),
        peer_name: push.peer_name.chars().take(MAX_PEER_NAME_CHARS).collect(),
        applied: push.applied,
        attempted_at_ms: Some(at),
        error: push
            .error
            .as_ref()
            .map(|error| error.chars().take(MAX_PUSH_ERROR_CHARS).collect()),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A peer's place in the running resends, given back when the resend ends.
struct Claim {
    service: Arc<Service>,
    peer: String,
}

impl Claim {
    fn take(service: &Arc<Service>, peer: &str) -> Option<Self> {
        let mut running = service.resending.lock().ok()?;
        running.insert(peer.to_string()).then(|| Self {
            service: service.clone(),
            peer: peer.to_string(),
        })
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Ok(mut running) = self.service.resending.lock() {
            running.remove(&self.peer);
        }
    }
}

/// A peer answered: a check, a sync or a push reached it. Send it, in the
/// background, every activation that still waits for it, one push per rig
/// database. A resend already running for this peer covers this contact
/// too. Only a server that may write rig databases sends anything. The task
/// answers each rig database it pushed, by slug, and whether the peer took it.
pub(crate) fn peer_reached(
    state: &Arc<AppState>,
    peer_id: &str,
) -> Option<JoinHandle<Vec<(String, bool)>>> {
    let service = writable(state).ok()?;
    let claim = Claim::take(&service, peer_id)?;
    let state = state.clone();
    Some(tokio::spawn(async move {
        let sent = resend(state, service, &claim.peer).await;
        drop(claim);
        sent
    }))
}

async fn resend(state: Arc<AppState>, service: Arc<Service>, peer_id: &str) -> Vec<(String, bool)> {
    let Some(peer) = registered_peers(&state)
        .into_iter()
        .find(|peer| peer.id == peer_id)
    else {
        return vec![];
    };
    let catalogs = state.all_databases();
    let instance = service.instance_id;
    let key = peer.id.clone();
    let databases = service
        .clone()
        .query(move |store| {
            let owed: BTreeSet<Uuid> = waiting_for(store, &key)?
                .into_iter()
                .map(|(_, catalog)| catalog)
                .collect();
            if owed.is_empty() {
                return Ok(vec![]);
            }
            let found = identified_catalogs(&catalogs, instance);
            Ok(owed
                .into_iter()
                .filter_map(|catalog| found.get(catalog).cloned())
                .collect::<Vec<_>>())
        })
        .await;
    let databases = match databases {
        Ok(databases) => databases,
        Err(error) => {
            tracing::warn!(peer = %peer.id, ?error, "Could not read which activations wait for a remote site");
            return vec![];
        }
    };
    let mut sent = Vec::new();
    for context in databases {
        let Some((push, waiting)) =
            send_and_record(&state, &service, &context, &peer, vec![], true).await
        else {
            continue;
        };
        let plans: Vec<String> = waiting
            .iter()
            .map(|w| format!("{} r{}", w.project, w.revision))
            .collect();
        if push.applied {
            tracing::info!(peer = %peer.id, catalog = %context.id, ?plans, summary = ?push.summary, "Sent waiting activations to their remote site");
        } else {
            tracing::warn!(peer = %peer.id, catalog = %context.id, ?plans, error = ?push.error, "Waiting activations could not be sent to their remote site");
        }
        sent.push((context.id.clone(), push.applied));
    }
    sent
}
