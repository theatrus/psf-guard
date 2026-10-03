use psf_guard_director_core::program::Program;
use psf_guard_director_meta::{
    equipment_report::EquipmentReport,
    profile::{Reported, RigProfile, Source},
    CatalogIdentity, Error, MetaStore, Uuid,
};
use rusqlite::Connection;
use tempfile::TempDir;

fn fixture() -> (TempDir, MetaStore, EquipmentReport) {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let catalog = CatalogIdentity {
        id: Uuid::new_v4(),
        origin_instance_id: store.instance_id(),
    };
    let rig = store
        .bind_catalog_rig_after(catalog, "Rig", true, || Ok(()))
        .unwrap()
        .rig
        .id;
    let profile = Uuid::new_v4();
    store
        .issue_client_pairing(catalog.id, rig, &"a".repeat(64), 100, 200)
        .unwrap();
    let client = store
        .redeem_client_pairing(&"a".repeat(64), &"b".repeat(64), profile, "NINA", 101)
        .unwrap();
    let mut program: Program = serde_json::from_str(include_str!(
        "../../director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    program.configuration.rig_id = rig.to_string();
    let report = EquipmentReport {
        report_id: Uuid::new_v4(),
        client_id: client.client_id,
        catalog_id: catalog.id,
        rig_id: rig,
        profile_id: profile,
        observed_at_ms: 1000,
        received_at_ms: 1010,
        configuration: program.configuration,
        filter_names: [("ha".into(), "H-alpha".into())].into(),
        accepted_revision: None,
        accepted_from_revision: None,
    };
    (dir, store, report)
}

#[test]
fn staging_is_bound_non_authorizing_and_retries_do_not_refresh_evidence() {
    let (_dir, mut store, report) = fixture();
    assert_eq!(store.report_equipment(&report).unwrap(), report);
    assert!(store.rig_profile(report.rig_id).unwrap().is_none());
    let mut retry = report.clone();
    retry.received_at_ms = 2000;
    assert_eq!(store.report_equipment(&retry).unwrap(), report);
    retry.filter_names.insert("ha".into(), "Changed".into());
    assert!(matches!(
        store.report_equipment(&retry),
        Err(Error::Conflict)
    ));
    let mut wrong = report.clone();
    wrong.profile_id = Uuid::new_v4();
    assert!(matches!(
        store.report_equipment(&wrong),
        Err(Error::Conflict)
    ));
    let mut next = report.clone();
    next.report_id = Uuid::new_v4();
    assert!(matches!(
        store.report_equipment(&next),
        Err(Error::Conflict)
    ));
    next.observed_at_ms += 1;
    store.report_equipment(&next).unwrap();
    assert!(matches!(
        store.accept_equipment_report(report.rig_id, report.client_id, report.report_id, 0, 1100),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store.report_equipment(&report),
        Err(Error::Conflict)
    ));
}

#[test]
fn acceptance_preserves_manual_settings_is_atomic_and_idempotent() {
    let (dir, mut store, report) = fixture();
    let mut profile = RigProfile::empty(report.rig_id, 1000);
    profile.limits.value.minimum_altitude_degrees = 35.0;
    let profile = store.save_rig_profile(&profile, 0).unwrap();
    store.report_equipment(&report).unwrap();
    assert!(matches!(
        store.accept_equipment_report(report.rig_id, report.client_id, report.report_id, 0, 1100),
        Err(Error::Conflict)
    ));
    let conn = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_report BEFORE UPDATE ON equipment_report BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    assert!(store
        .accept_equipment_report(
            report.rig_id,
            report.client_id,
            report.report_id,
            profile.revision,
            1100
        )
        .is_err());
    assert_eq!(
        store.rig_profile(report.rig_id).unwrap(),
        Some(profile.clone())
    );
    assert!(store.equipment_reports(report.rig_id).unwrap()[0]
        .accepted_revision
        .is_none());
    conn.execute_batch("DROP TRIGGER fail_report").unwrap();
    let accepted = store
        .accept_equipment_report(
            report.rig_id,
            report.client_id,
            report.report_id,
            profile.revision,
            1100,
        )
        .unwrap();
    assert_eq!(accepted.limits, profile.limits);
    assert_eq!(
        accepted.configuration.as_ref().unwrap().value,
        report.configuration
    );
    assert_eq!(
        store
            .accept_equipment_report(
                report.rig_id,
                report.client_id,
                report.report_id,
                profile.revision,
                1200
            )
            .unwrap(),
        accepted
    );
    let mut retry = report.clone();
    retry.received_at_ms = 1200;
    assert_eq!(
        store.report_equipment(&retry).unwrap().accepted_revision,
        Some(accepted.revision)
    );
    store
        .revoke_client(report.rig_id, report.client_id)
        .unwrap();
    assert!(store.equipment_reports(report.rig_id).unwrap().is_empty());
    assert_eq!(store.rig_profile(report.rig_id).unwrap(), Some(accepted));
}

#[test]
fn no_op_acceptance_retries_and_stale_reports_fail_closed() {
    let (_dir, mut store, report) = fixture();
    let mut profile = RigProfile::empty(report.rig_id, 1000);
    profile.configuration = Some(Reported {
        value: report.configuration.clone(),
        source: Source::Plugin {},
        reported_at_ms: report.observed_at_ms,
    });
    profile.filter_names = report.filter_names.clone();
    let profile = store.save_rig_profile(&profile, 0).unwrap();
    store.report_equipment(&report).unwrap();
    let accepted = store
        .accept_equipment_report(
            report.rig_id,
            report.client_id,
            report.report_id,
            profile.revision,
            1100,
        )
        .unwrap();
    assert_eq!(accepted.revision, profile.revision);
    assert_eq!(
        store
            .accept_equipment_report(
                report.rig_id,
                report.client_id,
                report.report_id,
                profile.revision,
                1200
            )
            .unwrap(),
        accepted
    );
    let mut changed = accepted.clone();
    changed.limits.value.minimum_altitude_degrees = 30.0;
    store.save_rig_profile(&changed, accepted.revision).unwrap();
    assert!(matches!(
        store.accept_equipment_report(
            report.rig_id,
            report.client_id,
            report.report_id,
            profile.revision,
            1300
        ),
        Err(Error::Conflict)
    ));
    let mut next = report.clone();
    next.report_id = Uuid::new_v4();
    next.observed_at_ms = 2000;
    next.received_at_ms = 2010;
    store.report_equipment(&next).unwrap();
    assert!(matches!(
        store.accept_equipment_report(next.rig_id, next.client_id, next.report_id, 2, 902001),
        Err(Error::Conflict)
    ));
    next.report_id = Uuid::new_v4();
    next.observed_at_ms = 1;
    next.received_at_ms = 902000;
    assert!(matches!(
        store.report_equipment(&next),
        Err(Error::InvalidInput)
    ));
}

#[test]
fn globally_reused_ids_conflict_and_schema_sixteen_upgrades() {
    let (dir, mut store, report) = fixture();
    store.report_equipment(&report).unwrap();
    store
        .issue_client_pairing(report.catalog_id, report.rig_id, &"c".repeat(64), 100, 200)
        .unwrap();
    let client = store
        .redeem_client_pairing(
            &"c".repeat(64),
            &"d".repeat(64),
            Uuid::new_v4(),
            "Other",
            101,
        )
        .unwrap();
    let mut reused = report.clone();
    reused.client_id = client.client_id;
    reused.profile_id = client.profile_id;
    assert!(matches!(
        store.report_equipment(&reused),
        Err(Error::Conflict)
    ));
    let instance = store.instance_id();
    drop(store);
    Connection::open(dir.path().join("meta.sqlite"))
        .unwrap()
        .execute_batch("DROP TABLE observing_preferences; DROP TABLE workload_budget; DROP TABLE workload_history; DROP TABLE workload_policy; DROP TABLE equipment_report; PRAGMA user_version=16;")
        .unwrap();
    let store = MetaStore::open(&dir.path().join("meta.sqlite")).unwrap();
    assert_eq!(store.instance_id(), instance);
    assert_eq!(store.clients(report.rig_id).unwrap().len(), 2);
    assert!(store.equipment_reports(report.rig_id).unwrap().is_empty());
}

#[test]
fn outstanding_allocations_block_acceptance_and_reports_survive_backup() {
    let (dir, mut store, report) = fixture();
    store.report_equipment(&report).unwrap();
    let id = Uuid::new_v4();
    let mut program: serde_json::Value = serde_json::from_str(include_str!(
        "../../director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    program["assignment"]["id"] = serde_json::json!(format!("allocation-{id}"));
    program["assignment"]["rig_id"] = serde_json::json!(report.rig_id);
    program["configuration"]["rig_id"] = serde_json::json!(report.rig_id);
    let allocation = psf_guard_director_meta::allocation::Allocation {
        schema_version: 1,
        allocation_id: id,
        coordinator_instance_id: store.instance_id(),
        catalog_id: report.catalog_id,
        rig_id: report.rig_id,
        client_id: report.client_id,
        profile_id: report.profile_id,
        preview_revision: "a".repeat(64),
        admitted_at_ms: 1001,
        snapshot: serde_json::json!({"coordinator_instance_id":store.instance_id(),"catalog_id":report.catalog_id,"rig_id":report.rig_id,"program":program}),
    };
    store.admit_allocation(&allocation).unwrap();
    assert!(matches!(
        store.accept_equipment_report(report.rig_id, report.client_id, report.report_id, 0, 1100),
        Err(Error::Conflict)
    ));
    assert!(store.rig_profile(report.rig_id).unwrap().is_none());
    let backup = dir.path().join("backup.sqlite");
    store.backup(&backup).unwrap();
    let copy = MetaStore::open(&backup).unwrap();
    assert_eq!(copy.equipment_reports(report.rig_id).unwrap(), vec![report]);
    assert_eq!(
        copy.allocation(allocation.rig_id).unwrap(),
        Some(allocation)
    );
}

#[test]
fn failed_schema_seventeen_upgrade_rolls_back() {
    let (dir, store, report) = fixture();
    drop(store);
    let conn = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    conn.execute_batch("DROP TABLE observing_preferences; DROP TABLE workload_budget; DROP TABLE workload_history; DROP TABLE workload_policy; DROP TABLE equipment_report; PRAGMA user_version=16; CREATE VIEW equipment_report AS SELECT 1;").unwrap();
    assert!(MetaStore::open(&dir.path().join("meta.sqlite")).is_err());
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        16
    );
    conn.execute_batch("DROP VIEW equipment_report;").unwrap();
    assert!(MetaStore::open(&dir.path().join("meta.sqlite"))
        .unwrap()
        .client_for_token(&"b".repeat(64))
        .unwrap()
        .is_some_and(|c| c.client_id == report.client_id));
}
