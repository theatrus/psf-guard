#[path = "../../director-interop/tests/support/mod.rs"]
mod support;
use psf_guard_director_interop::{astrocollab::Source, collaboration::*};
use psf_guard_director_meta::{collaboration::ImportAction, plan::Goal, Error, MetaStore, Uuid};
use rusqlite::Connection;
use serde_json::json;
use support::*;
use tempfile::TempDir;

fn store() -> (TempDir, MetaStore, Uuid) {
    let dir = TempDir::new().unwrap();
    let mut s = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = s.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    (dir, s, rig)
}
fn apply(s: &mut MetaStore, p: &PreparedImport, rig: Uuid) {
    let preview = s.preview_collaboration_import(p, rig).unwrap();
    s.apply_collaboration_import(p, rig, &preview.review_digest, 1000)
        .unwrap();
}
fn report(
    s: &mut MetaStore,
    p: &PreparedImport,
    frames: &[u128],
) -> psf_guard_director_meta::collaboration::QueuedReport {
    let frames: Vec<_> = frames.iter().map(|n| frame(p, *n)).collect();
    s.queue_collaboration_report(&finalize_contribution(p, &frames).unwrap(), 2000)
        .unwrap()
}
fn changed() -> PreparedImport {
    let mut v = wire();
    v.as_object_mut().unwrap().remove("task");
    v["tasks"][0]["version"] = json!(3);
    v["tasks"][0]["visit"]["frames"]["O"] = json!(12);
    prepared(&v)
}

#[test]
fn measured_shutter_jitter_survives_queue_reopen_and_idempotent_replay() {
    let (dir, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let mut a = frame(&p, 1);
    let mut b = frame(&p, 2);
    a.exposure_ms = 300_007;
    b.exposure_ms = 299_995;
    let finalized = finalize_contribution(&p, &[a, b]).unwrap();
    let queued = s.queue_collaboration_report(&finalized, 2000).unwrap();
    assert_eq!(queued.integration_ms, 600_002);
    assert_eq!(queued.payload["exposure"], 300.001);
    drop(s);
    let mut s = MetaStore::open(&dir.path().join("meta.sqlite")).unwrap();
    assert_eq!(
        s.pending_collaboration_reports(p.source(), 1).unwrap()[0].id,
        queued.id
    );
    assert_eq!(
        s.queue_collaboration_report(&finalized, 3000).unwrap().id,
        queued.id
    );
}

#[test]
fn connection_and_import_cannot_bind_one_agent_to_different_rigs() {
    use psf_guard_director_meta::collaboration_connection::{ConnectionBinding, ConnectionState};
    let (_, mut s, rig) = store();
    let other = s.create_rig(Uuid::new_v4(), "Other rig").unwrap().id;
    let p = import();
    let binding = ConnectionBinding {
        id: Uuid::new_v4(),
        rig_id: rig,
        base_url: p.source().base_url().into(),
        name: "Collaboration".into(),
        allow_loopback_http: false,
        agent_id: None,
        state: ConnectionState::New,
        settings: None,
        background: None,
    };
    s.create_collaboration_connection(&binding).unwrap();
    let mut registered = binding.clone();
    registered.agent_id = Some(p.source().agent_id().into());
    registered.state = ConnectionState::Registered;
    s.update_collaboration_connection(&binding, &registered)
        .unwrap();
    assert!(matches!(
        s.preview_collaboration_import(&p, other),
        Err(Error::Conflict)
    ));
    apply(&mut s, &p, rig);
    let second = ConnectionBinding {
        id: Uuid::new_v4(),
        rig_id: other,
        ..binding
    };
    s.create_collaboration_connection(&second).unwrap();
    let wrong = ConnectionBinding {
        id: second.id,
        rig_id: other,
        ..registered
    };
    assert!(s.update_collaboration_connection(&second, &wrong).is_err());
    assert_eq!(s.collaboration_connection(second.id).unwrap(), Some(second));
}

#[test]
fn broad_parent_regions_do_not_prevent_importing_supported_camera_cells() {
    let (_, mut s, rig) = store();
    let mut v = wire();
    v.as_object_mut().unwrap().remove("task");
    v["tasks"][0]["region"]["width"] = json!(40.0);
    let p = prepared(&v);
    apply(&mut s, &p, rig);
    let framing = s.framing_draft(p.project_id()).unwrap().unwrap();
    assert_eq!(
        framing.panel.unwrap().width_degrees,
        f64::from(p.share().cells[0].region.width_mas) / 3_600_000.0
    );
    assert_eq!(
        s.collaboration_import(p.import_id())
            .unwrap()
            .unwrap()
            .plan
            .share()
            .region
            .width_mas,
        p.share().region.width_mas
    );
}

#[test]
fn preview_is_read_only_and_apply_keeps_exact_panels_in_an_inactive_project_draft() {
    let (dir, mut s, rig) = store();
    let p = import();
    let preview = s.preview_collaboration_import(&p, rig).unwrap();
    assert_eq!(preview.action, ImportAction::Create);
    assert_eq!(preview.expected_revision, 0);
    assert!(!preview.acquisition_enabled);
    assert!(s.project(p.project_id()).unwrap().is_none());
    assert!(s.collaboration_import(p.import_id()).unwrap().is_none());
    apply(&mut s, &p, rig);
    let draft = s.plan_draft(p.project_id()).unwrap().unwrap();
    assert_eq!(draft.objectives.len(), 1);
    assert_eq!(draft.objectives[0].bandpass_id, "oiii");
    assert_eq!(draft.objectives[0].goal, Goal::Frames { value: 11 });
    assert!(draft.contributions.is_empty());
    let framing = s.framing_draft(p.project_id()).unwrap().unwrap();
    assert!((framing.center.ra_degrees - 10.6847).abs() < 1e-6);
    assert_eq!(
        framing.mosaic,
        psf_guard_director_core::framing::Mosaic::SINGLE
    );
    assert!(s.collaboration_requires_admission(p.project_id()).unwrap());
    assert!(s.activation(p.project_id()).unwrap().is_none());
    drop(s);
    let s = MetaStore::open(&dir.path().join("meta.sqlite")).unwrap();
    let read = s.collaboration_import(p.import_id()).unwrap().unwrap();
    assert_eq!(read.plan.share().cells, p.share().cells);
    assert_eq!(read.plan.share().panel_order, p.share().panel_order);
    assert_eq!(read.plan.digest(), p.digest());
    assert_eq!(read.rig_id, rig);
    assert_eq!(
        s.collaboration_import_ids(p.project_id(), None, 256)
            .unwrap()
            .ids,
        vec![p.import_id()]
    );
}

#[test]
fn reimport_is_idempotent_but_stale_previews_cannot_overwrite_operator_edits() {
    let (_, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let same = s.preview_collaboration_import(&p, rig).unwrap();
    assert_eq!(same.action, ImportAction::Unchanged);
    s.apply_collaboration_import(&p, rig, &same.review_digest, 1500)
        .unwrap();
    assert_eq!(
        s.collaboration_import(p.import_id())
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    let update = changed();
    let preview = s.preview_collaboration_import(&update, rig).unwrap();
    assert_eq!(preview.action, ImportAction::Update);
    let mut draft = s.plan_draft(p.project_id()).unwrap().unwrap();
    draft.objectives[0].goal = Goal::Hours { value: 9.0 };
    draft.objectives[0].priority = 3;
    let saved = s.save_plan_draft(&draft, draft.revision).unwrap();
    assert!(matches!(
        s.apply_collaboration_import(&update, rig, &preview.review_digest, 2000),
        Err(Error::Conflict)
    ));
    apply(&mut s, &update, rig);
    assert_eq!(s.plan_draft(p.project_id()).unwrap().unwrap(), saved);
    assert_eq!(
        s.collaboration_import(p.import_id())
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        s.collaboration_import_revision(p.import_id(), p.digest())
            .unwrap()
            .unwrap()
            .plan
            .digest(),
        p.digest()
    );
}

#[test]
fn project_rollup_uses_origin_identity_not_names_and_an_agent_stays_bound_to_one_rig() {
    let (_, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let rig2 = s.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    assert!(matches!(
        s.preview_collaboration_import(&p, rig2),
        Err(Error::Conflict)
    ));
    let source2 = Source::new(source().base_url(), "000000000009", false).unwrap();
    let mut v = wire();
    v.as_object_mut().unwrap().remove("task");
    v["tasks"][0]["agent"] = json!(source2.agent_id());
    let p2 = prepare_import(&serde_json::to_vec(&v).unwrap(), &source2, NIGHT, TASK).unwrap();
    apply(&mut s, &p2, rig2);
    assert_eq!(p2.project_id(), p.project_id());
    let page = s.collaboration_import_ids(p.project_id(), None, 1).unwrap();
    assert_eq!(page.ids.len(), 1);
    assert!(page.next_after.is_some());
    let end = s
        .collaboration_import_ids(p.project_id(), page.next_after, 1)
        .unwrap();
    assert_eq!(end.ids.len(), 1);
    assert!(end.next_after.is_none());
    assert_ne!(end.ids, page.ids);
    assert!(s.collaboration_import_ids(p.project_id(), None, 0).is_err());
    v["tasks"][0]["project"] = json!("000000000008");
    let requirements = v["requirementsByProject"]["000000000002"].clone();
    v["requirementsByProject"]["000000000008"] = requirements;
    v["tasks"][0]["id"] = json!("000000000007");
    let other = prepare_import(
        &serde_json::to_vec(&v).unwrap(),
        &source2,
        NIGHT,
        "000000000007",
    )
    .unwrap();
    apply(&mut s, &other, rig2);
    assert_ne!(other.project_id(), p.project_id());
    assert_eq!(
        s.project(other.project_id()).unwrap().unwrap().name,
        s.project(p.project_id()).unwrap().unwrap().name
    );
}

#[test]
fn version_rollback_or_silent_mutation_is_refused_and_retiling_after_credit_is_held() {
    let (_, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let mut v = wire();
    v.as_object_mut().unwrap().remove("task");
    v["tasks"][0]["version"] = json!(1);
    assert!(matches!(
        s.preview_collaboration_import(&prepared(&v), rig),
        Err(Error::Conflict)
    ));
    v["tasks"][0]["version"] = json!(2);
    v["tasks"][0]["visit"]["frames"]["O"] = json!(12);
    assert!(matches!(
        s.preview_collaboration_import(&prepared(&v), rig),
        Err(Error::Conflict)
    ));
    report(&mut s, &p, &[1]);
    v["tasks"][0]["version"] = json!(3);
    v["tasks"][0]["cells"][0]["ra"] = json!(7.7);
    assert!(matches!(
        s.preview_collaboration_import(&prepared(&v), rig),
        Err(Error::Conflict)
    ));
}

#[test]
fn original_assignment_survives_updates_and_report_replay_survives_backup_and_restore() {
    let (dir, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    apply(&mut s, &changed(), rig);
    let queued = report(&mut s, &p, &[2, 1]);
    assert_eq!(queued.payload["night"], NIGHT);
    assert_eq!(
        queued.captures,
        vec![frame(&p, 1).capture_id, frame(&p, 2).capture_id]
    );
    assert_eq!(
        queued.images,
        vec![frame(&p, 1).image_guid, frame(&p, 2).image_guid]
    );
    let backup = dir.path().join("backup.sqlite");
    s.backup(&backup).unwrap();
    let restored = dir.path().join("restored.sqlite");
    MetaStore::restore(&backup, &restored).unwrap();
    let mut copy = MetaStore::open(&restored).unwrap();
    let replay = report(&mut copy, &p, &[1, 2]);
    assert_eq!(queued.id, replay.id);
    assert_eq!(queued.payload, replay.payload);
    assert_eq!(
        copy.pending_collaboration_reports(&source(), 200)
            .unwrap()
            .len(),
        1
    );
    assert!(copy.pending_collaboration_reports(&source(), 201).is_err());
}

#[test]
fn later_nights_have_independent_report_keys_and_cannot_recredit_the_same_image() {
    let (dir, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let original = report(&mut s, &p, &[1]);
    let later = finalize_contribution_for_night(&p, &[frame(&p, 2)], "2026-10-12").unwrap();
    let queued = s.queue_collaboration_report(&later, 3000).unwrap();
    assert_eq!(queued.payload["night"], "2026-10-12");
    assert_eq!(queued.payload["task"], p.share().task_id);
    let duplicate = finalize_contribution_for_night(&p, &[frame(&p, 1)], "2026-10-12").unwrap();
    assert!(matches!(
        s.queue_collaboration_report(&duplicate, 3000),
        Err(Error::Conflict)
    ));
    assert_eq!(
        s.pending_collaboration_reports(&source(), 200)
            .unwrap()
            .len(),
        2
    );
    let extended =
        finalize_contribution_for_night(&p, &[frame(&p, 2), frame(&p, 3)], "2026-10-12").unwrap();
    let extension = s.queue_collaboration_report(&extended, 4000).unwrap();
    assert_eq!(
        s.pending_collaboration_reports(&source(), 200)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![original.id, queued.id]
    );
    let path = dir.path().join("meta.sqlite");
    drop(s);
    let mut reopened = MetaStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .queue_collaboration_report(&later, 5000)
            .unwrap()
            .id,
        queued.id
    );
    let pending = reopened
        .pending_collaboration_reports(&source(), 200)
        .unwrap();
    let response = serde_json::to_vec(&json!({"recorded":[reply(1,true),reply(2,true)]})).unwrap();
    reopened
        .acknowledge_collaboration_reports(
            &source(),
            &pending.iter().map(|r| r.id).collect::<Vec<_>>(),
            &response,
            6000,
        )
        .unwrap();
    assert_eq!(
        reopened
            .pending_collaboration_reports(&source(), 200)
            .unwrap()[0]
            .id,
        extension.id
    );
}

#[test]
fn delayed_credit_survives_a_retile_but_different_snapshots_cannot_replace_one_wire_key() {
    let (_, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let mut v = wire();
    v.as_object_mut().unwrap().remove("task");
    v["tasks"][0]["version"] = json!(3);
    v["tasks"][0]["cells"][0]["ra"] = json!(7.7);
    let retiled = prepared(&v);
    apply(&mut s, &retiled, rig);
    let late = finalize_contribution_for_night(&p, &[frame(&p, 1)], "2026-10-12").unwrap();
    let old = s.queue_collaboration_report(&late, 3000).unwrap();
    assert_eq!(late.source_digest(), p.digest());
    assert_eq!(
        s.collaboration_report(old.id).unwrap().unwrap().payload["night"],
        "2026-10-12"
    );

    v["tasks"][0]["assignedNight"] = json!("2026-10-08");
    let next = prepare_import(
        &serde_json::to_vec(&v).unwrap(),
        &source(),
        "2026-10-08",
        TASK,
    )
    .unwrap();
    apply(&mut s, &next, rig);
    let same_key =
        finalize_contribution_for_night(&next, &[frame(&next, 2)], "2026-10-12").unwrap();
    assert!(matches!(
        s.queue_collaboration_report(&same_key, 4000),
        Err(Error::Conflict)
    ));
    let next_night =
        finalize_contribution_for_night(&next, &[frame(&next, 3)], "2026-10-13").unwrap();
    s.queue_collaboration_report(&next_night, 4000).unwrap();
    assert_eq!(
        s.pending_collaboration_reports(&source(), 200)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn acknowledgement_clears_only_the_sent_snapshot_and_preserves_the_first_remote_verdict() {
    let (_, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let a = report(&mut s, &p, &[1]);
    let b = report(&mut s, &p, &[1, 2]);
    let pending = s.pending_collaboration_reports(&source(), 200).unwrap();
    assert_eq!(pending.iter().map(|r| r.id).collect::<Vec<_>>(), vec![a.id]);
    let response = serde_json::to_vec(&json!({"recorded":[reply(1,false)]})).unwrap();
    assert!(matches!(
        s.acknowledge_collaboration_reports(&source(), &[a.id], &response, 1999),
        Err(Error::InvalidInput)
    ));
    assert!(s
        .collaboration_report(a.id)
        .unwrap()
        .unwrap()
        .recorded
        .is_none());
    let ack = s
        .acknowledge_collaboration_reports(&source(), &[a.id], &response, 3000)
        .unwrap();
    assert!(!ack[0].recorded.as_ref().unwrap().accepted);
    assert_eq!(
        s.pending_collaboration_reports(&source(), 200).unwrap()[0].id,
        b.id
    );
    let mut retry = reply(1, true);
    retry["duplicate"] = json!(true);
    let ack = s
        .acknowledge_collaboration_reports(
            &source(),
            &[a.id],
            &serde_json::to_vec(&json!({"recorded":[retry]})).unwrap(),
            4000,
        )
        .unwrap();
    assert_eq!(ack[0].acknowledged_at_ms, Some(3000));
    assert!(!ack[0].recorded.as_ref().unwrap().accepted);
    assert!(s
        .acknowledge_collaboration_reports(
            &source(),
            &[a.id],
            &serde_json::to_vec(&json!({"recorded":[reply(2,true)]})).unwrap(),
            4000
        )
        .is_err());
    assert_eq!(
        s.plan_draft(p.project_id())
            .unwrap()
            .unwrap()
            .contributions
            .len(),
        0
    );
}

#[test]
fn short_malformed_foreign_or_unknown_acknowledgements_are_atomic() {
    let (_, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let a = report(&mut s, &p, &[1]);
    let mut f = frame(&p, 2);
    f.panel_index = 1;
    let b = s
        .queue_collaboration_report(&finalize_contribution(&p, &[f]).unwrap(), 2000)
        .unwrap();
    for response in [
        json!({"recorded":[reply(1,true)]}),
        json!({"recorded":[reply(1,true),{"id":"bad"}]}),
        json!({"recorded":[reply(1,true),reply(1,true)]}),
    ] {
        assert!(s
            .acknowledge_collaboration_reports(
                &source(),
                &[a.id, b.id],
                &serde_json::to_vec(&response).unwrap(),
                3000
            )
            .is_err());
        assert!(s
            .collaboration_report(a.id)
            .unwrap()
            .unwrap()
            .recorded
            .is_none());
        assert!(s
            .collaboration_report(b.id)
            .unwrap()
            .unwrap()
            .recorded
            .is_none());
    }
    let reply = serde_json::to_vec(&json!({"recorded":[reply(1,true),reply(2,true)]})).unwrap();
    assert!(s
        .acknowledge_collaboration_reports(&source(), &[a.id, Uuid::new_v4()], &reply, 3000)
        .is_err());
    assert!(s
        .collaboration_report(a.id)
        .unwrap()
        .unwrap()
        .recorded
        .is_none());
    let foreign = Source::new("https://other.example", source().agent_id(), false).unwrap();
    assert!(s
        .acknowledge_collaboration_reports(&foreign, &[a.id, b.id], &reply, 3000)
        .is_err());
    assert!(s
        .pending_collaboration_reports(&foreign, 200)
        .unwrap()
        .is_empty());
    assert!(s
        .collaboration_report(a.id)
        .unwrap()
        .unwrap()
        .recorded
        .is_none());
}

#[test]
fn corrections_and_capture_reassignment_cannot_rewrite_scientific_credit() {
    let (_, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    report(&mut s, &p, &[1]);
    let mut changed = frame(&p, 1);
    changed.hfr_arcsec = Some(3.0);
    assert!(matches!(
        s.queue_collaboration_report(&finalize_contribution(&p, &[changed]).unwrap(), 2000),
        Err(Error::Conflict)
    ));
    let mut moved = frame(&p, 1);
    moved.panel_index = 1;
    assert!(matches!(
        s.queue_collaboration_report(&finalize_contribution(&p, &[moved]).unwrap(), 2000),
        Err(Error::Conflict)
    ));
    let mut twin = frame(&p, 2);
    twin.image_guid = frame(&p, 1).image_guid;
    assert!(matches!(
        s.queue_collaboration_report(&finalize_contribution(&p, &[twin]).unwrap(), 2000),
        Err(Error::Conflict)
    ));
    let a = report(&mut s, &p, &[1, 2]);
    assert_eq!(a.integration_ms, 600_000);
    assert!(matches!(
        s.queue_collaboration_report(&finalize_contribution(&p, &[frame(&p, 2)]).unwrap(), 2000),
        Err(Error::Conflict)
    ));
}

#[test]
fn imported_projects_cannot_lose_provenance_through_attach_or_detach() {
    let (_, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let local = s.create_project(Uuid::new_v4(), "Local").unwrap().id;
    assert!(matches!(
        s.attach_project(local, p.project_id()),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        s.attach_project(p.project_id(), local),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        s.detach_project(
            p.project_id(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "New",
            false
        ),
        Err(Error::Conflict)
    ));
    assert!(s.project(local).unwrap().is_some());
    assert!(s.collaboration_import(p.import_id()).unwrap().is_some());
}

#[test]
fn schema_twenty_two_migrates_without_changing_existing_identity_and_bad_migrations_roll_back() {
    for malformed in [false, true] {
        let (dir, s, rig) = store();
        let instance = s.instance_id();
        drop(s);
        let path = dir.path().join("meta.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE collaboration_capture; DROP TABLE collaboration_outbox; DROP TABLE collaboration_revision; DROP TABLE collaboration_import; DROP TABLE collaboration_project; PRAGMA user_version=22;").unwrap();
        if malformed {
            conn.execute_batch("CREATE TABLE collaboration_project(wrong TEXT)")
                .unwrap();
            assert!(matches!(
                MetaStore::open(&path),
                Err(Error::CorruptDatabase)
            ));
            assert_eq!(
                conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
                    .unwrap(),
                22
            );
            assert!(conn.prepare("SELECT * FROM collaboration_import").is_err());
        } else {
            let migrated = MetaStore::open(&path).unwrap();
            assert_eq!(migrated.instance_id(), instance);
            assert!(migrated.rig(rig).unwrap().is_some());
            assert_eq!(
                conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
                    .unwrap(),
                27
            );
        }
    }
}

#[test]
fn corrupted_history_or_outbox_is_not_replayed() {
    let (dir, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let r = report(&mut s, &p, &[1]);
    let conn = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    assert!(conn
        .execute(
            "UPDATE collaboration_outbox SET source_digest=?1 WHERE id=?2",
            ["f".repeat(64), r.id.to_string()],
        )
        .is_err());
    // Bypass the write guard only to exercise read-time corruption detection.
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute(
        "UPDATE collaboration_outbox SET source_digest=?1 WHERE id=?2",
        ["f".repeat(64), r.id.to_string()],
    )
    .unwrap();
    assert!(matches!(
        s.pending_collaboration_reports(&source(), 200),
        Err(Error::CorruptDatabase)
    ));
    conn.execute("UPDATE collaboration_revision SET payload=x'7b7d'", [])
        .unwrap();
    assert!(matches!(
        s.collaboration_import_revision(p.import_id(), p.digest()),
        Err(Error::CorruptDatabase)
    ));
}
