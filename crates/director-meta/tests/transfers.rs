use psf_guard_director_meta::{
    collaboration_connection::{ConnectionBinding, ConnectionState},
    transfer::State,
    Error, MetaStore, Uuid,
};
use serde_json::json;

fn setup() -> (tempfile::TempDir, MetaStore, Uuid) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = Uuid::new_v4();
    let id = Uuid::new_v4();
    store.create_rig(rig, "Rig").unwrap();
    store
        .create_collaboration_connection(&ConnectionBinding {
            id,
            rig_id: rig,
            base_url: "https://example.com/".into(),
            name: "Rig".into(),
            allow_loopback_http: false,
            agent_id: None,
            state: ConnectionState::New,
            settings: None,
            background: None,
        })
        .unwrap();
    (dir, store, id)
}

#[test]
fn duplicate_requests_and_checkpoints_survive_reopen() {
    let (dir, mut store, id) = setup();
    let mut job = store
        .create_transfer(id, json!({"selection":{"images":["one"]}}), 10)
        .unwrap();
    assert_eq!(
        store
            .create_transfer(id, job.request.clone(), 20)
            .unwrap()
            .id,
        job.id
    );
    job.state = State::Uploading;
    job.package = Some(json!({"files":["one","two"]}));
    job.completed.push("one".into());
    let job = store.save_transfer(job).unwrap();
    drop(store);
    let mut store = MetaStore::open(&dir.path().join("meta.sqlite")).unwrap();
    store.recover_transfers().unwrap();
    let recovered = store.transfer(job.id).unwrap().unwrap();
    assert_eq!(recovered.state, State::Queued);
    assert_eq!(recovered.completed, vec!["one"]);
    assert_eq!(recovered.package, job.package);
    assert!(recovered.revision > job.revision);
}

#[test]
fn stale_worker_cannot_undo_pause_or_cancel() {
    let (_dir, mut store, id) = setup();
    let mut stale = store.create_transfer(id, json!({}), 10).unwrap();
    let paused = store
        .control_transfer(stale.id, stale.revision, "pause")
        .unwrap();
    stale.state = State::Complete;
    assert!(matches!(store.save_transfer(stale), Err(Error::Conflict)));
    let cancelled = store
        .control_transfer(paused.id, paused.revision, "cancel")
        .unwrap();
    store.recover_transfers().unwrap();
    assert_eq!(
        store.transfer(cancelled.id).unwrap().unwrap().state,
        State::Cancelled
    );
}

#[test]
fn review_is_required_before_manual_upload_and_invalid_controls_fail() {
    let (_dir, mut store, id) = setup();
    let job = store.create_transfer(id, json!({}), 10).unwrap();
    assert!(store
        .control_transfer(job.id, job.revision, "approve")
        .is_err());
    let mut ready = job;
    ready.state = State::Review;
    ready.package = Some(json!({}));
    let ready = store.save_transfer(ready).unwrap();
    let queued = store
        .control_transfer(ready.id, ready.revision, "approve")
        .unwrap();
    assert_eq!(queued.state, State::Queued);
    assert!(store
        .control_transfer(queued.id, ready.revision, "cancel")
        .is_err());
    assert!(store
        .control_transfer(queued.id, queued.revision, "invent")
        .is_err());
}

#[test]
fn schema_30_upgrade_adds_transfers_without_replacing_identity() {
    let (dir, store, _) = setup();
    let instance = store.instance_id();
    drop(store);
    let conn = rusqlite::Connection::open(dir.path().join("meta.sqlite")).unwrap();
    conn.execute_batch("DROP TABLE collaboration_transfer_summary;DROP TABLE collaboration_transfer_receipt;DROP TABLE collaboration_transfer_artifact;DROP TABLE collaboration_transfer_definition;DROP TABLE collaboration_transfer;DROP TABLE collaboration_transfer_policy;PRAGMA user_version=30").unwrap();
    drop(conn);
    let store = MetaStore::open(&dir.path().join("meta.sqlite")).unwrap();
    assert_eq!(store.instance_id(), instance);
    assert!(store.transfers().unwrap().is_empty());
    assert!(store.transfer_policies().unwrap().is_empty());
}

#[test]
fn failed_transfer_migration_does_not_advance_the_schema_or_replace_data() {
    let (dir, store, _) = setup();
    let instance = store.instance_id();
    drop(store);
    let conn = rusqlite::Connection::open(dir.path().join("meta.sqlite")).unwrap();
    conn.execute_batch("DROP TABLE collaboration_transfer_artifact; CREATE TABLE collaboration_transfer_artifact(unrelated INTEGER); INSERT INTO collaboration_transfer_artifact VALUES(42); PRAGMA user_version=30").unwrap();
    assert!(matches!(
        MetaStore::open(&dir.path().join("meta.sqlite")),
        Err(Error::CorruptDatabase)
    ));
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        30
    );
    assert_eq!(
        conn.query_row(
            "SELECT unrelated FROM collaboration_transfer_artifact",
            [],
            |r| r.get::<_, i32>(0)
        )
        .unwrap(),
        42
    );
    assert_eq!(
        conn.query_row("SELECT instance_id FROM meta", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        instance.to_string()
    );
}

#[test]
fn automatic_policy_is_explicit_and_database_bound() {
    let (dir, mut store, id) = setup();
    let catalog = Uuid::new_v4();
    assert!(store.transfer_policies().unwrap().is_empty());
    store.set_transfer_policy(id, catalog, true, "raw").unwrap();
    assert!(store
        .set_transfer_policy(id, catalog, true, "wbpp")
        .is_err());
    drop(store);
    let store = MetaStore::open(&dir.path().join("meta.sqlite")).unwrap();
    let policy = store.transfer_policies().unwrap().pop().unwrap();
    assert_eq!(policy["catalog_id"], catalog.to_string());
    assert_eq!(policy["automatic"], true);
}

#[test]
fn manual_retry_of_an_automatic_package_requires_review() {
    let (_dir, mut store, id) = setup();
    let mut job = store
        .create_transfer(id, json!({"automatic":true}), 10)
        .unwrap();
    job.state = State::NeedsReview;
    job.package = Some(json!({"artifacts":[]}));
    let job = store.save_transfer(job).unwrap();
    let retried = store
        .control_transfer(job.id, job.revision, "retry")
        .unwrap();
    assert_eq!(retried.state, State::Preparing);
    assert_eq!(retried.request["automatic"], false);
    assert!(retried.package.is_none());
}

#[test]
fn receipts_are_small_idempotent_writes_and_do_not_rewrite_package_data() {
    let (dir, mut store, id) = setup();
    let mut job = store
        .create_transfer(id, json!({"selection":{"catalog":"rig"}}), 10)
        .unwrap();
    job.package = Some(
        json!({"label":"M31","artifacts":[{"name":"one.fits","bytes":100}],"manifest":{"large":"x".repeat(100000)}}),
    );
    job.state = State::Uploading;
    let job = store.save_transfer(job).unwrap();
    let revision = store
        .acknowledge_transfer_file(job.id, job.revision, "one.fits", 100)
        .unwrap();
    assert_eq!(
        store
            .acknowledge_transfer_file(job.id, revision, "one.fits", 100)
            .unwrap(),
        revision
    );
    assert!(store
        .acknowledge_transfer_file(job.id, revision, "other.fits", 100)
        .is_err());
    let restored = store.transfer(job.id).unwrap().unwrap();
    assert_eq!(restored.package, job.package);
    assert_eq!(restored.completed, vec!["one.fits"]);
    let summary = store.transfer_activity().unwrap().pop().unwrap();
    assert_eq!(summary["completed_bytes"], 100);
    assert_eq!(summary["completed_files"], 1);
    let conn = rusqlite::Connection::open(dir.path().join("meta.sqlite")).unwrap();
    let checkpoint: String = conn
        .query_row("SELECT payload FROM collaboration_transfer", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(checkpoint.len() < 2000);
    assert!(!checkpoint.contains("large"));
}

#[test]
fn completed_history_does_not_hide_new_queued_work() {
    let (_dir, mut store, id) = setup();
    for n in 0..60 {
        let mut job = store.create_transfer(id, json!({"n":n}), 10).unwrap();
        job.state = State::Complete;
        store.save_transfer(job).unwrap();
    }
    let next = store.create_transfer(id, json!({"n":"next"}), 10).unwrap();
    assert_eq!(store.next_transfer(20).unwrap().unwrap().id, next.id);
    assert_eq!(store.transfer_activity().unwrap().len(), 50);
}
