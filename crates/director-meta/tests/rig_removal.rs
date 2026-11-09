use psf_guard_director_meta::{
    activation::{ActivatedRig, Activation},
    catalog::SourceProject,
    framing::FramingDraft,
    plan::PlanDraft,
    profile::RigProfile,
    rig_removal::RemovalBlocker,
    CatalogIdentity, Error, MetaStore, Uuid,
};
use serde_json::json;

struct Fixture {
    store: MetaStore,
    catalog: Uuid,
    rig: Uuid,
    other_rig: Uuid,
    plan: Uuid,
    path: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

fn bind(store: &mut MetaStore, name: &str) -> (Uuid, Uuid) {
    let catalog = CatalogIdentity {
        id: Uuid::new_v4(),
        origin_instance_id: store.instance_id(),
    };
    let rig = store
        .bind_catalog_rig_after(catalog, name, true, || Ok(()))
        .unwrap()
        .rig
        .id;
    (catalog.id, rig)
}

fn plan_with(project: Uuid, rigs: &[Uuid]) -> PlanDraft {
    let objective = Uuid::new_v4();
    let contributions: Vec<_> = rigs
        .iter()
        .map(|rig| {
            json!({"id": Uuid::new_v4(), "objective_id": objective, "rig_id": rig,
                "template": {"template_guid": null, "template_id": null, "name": "Ha 300", "filter_name": "Ha", "gain": 100, "offset": 30, "bin": 1, "readout_mode": null},
                "exposure_seconds": 300.0, "panel_ids": [], "enabled": true})
        })
        .collect();
    serde_json::from_value(json!({
        "project_id": project, "revision": 0, "updated_at_ms": 0,
        "objectives": [{"id": objective, "bandpass_id": "h_alpha", "purpose": "faint_detail", "goal": {"kind": "hours", "value": 6.0}, "priority": 1}],
        "contributions": contributions,
    }))
    .unwrap()
}

fn activated(rig: Uuid, catalog: Uuid) -> ActivatedRig {
    ActivatedRig {
        rows_digest: None,
        rig_id: rig,
        catalog_id: catalog,
        project_guid: Uuid::new_v4(),
        profile_id: "profile".into(),
        targets: vec![],
        plans: vec![],
    }
}

/// A rig with a profile, a plugin and a linked project, in a plan a second
/// rig shoots too.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let (catalog, rig) = bind(&mut store, "Removed rig");
    let (other_catalog, other_rig) = bind(&mut store, "Other rig");
    store
        .save_rig_profile(&RigProfile::empty(rig, 1000), 0)
        .unwrap();
    store
        .issue_client_pairing(catalog, rig, &"a".repeat(64), 100, 200)
        .unwrap();
    store
        .redeem_client_pairing(
            &"a".repeat(64),
            &"b".repeat(64),
            Uuid::new_v4(),
            "NINA",
            101,
        )
        .unwrap();
    let plan = store
        .adopt_source_projects(
            catalog,
            rig,
            &[SourceProject {
                source_project_guid: Uuid::new_v4(),
                source_profile_id: "profile".into(),
                proposed_project_id: Uuid::new_v4(),
                name: "M31".into(),
            }],
        )
        .unwrap()[0];
    store
        .save_plan_draft(&plan_with(plan, &[rig, other_rig]), 0)
        .unwrap();
    let framing: FramingDraft = serde_json::from_value(json!({
        "project_id": plan, "revision": 0, "target_name": "M31",
        "center": {"ra_degrees": 10.68, "dec_degrees": 41.27}, "position_angle_degrees": 0.0,
        "mosaic": {"rows": 1, "columns": 1, "overlap_percent": 20},
        "panel_rig_id": rig, "panel": {"width_degrees": 2.0, "height_degrees": 1.5},
        "shown_rig_ids": [rig, other_rig], "survey_id": "dss2_color", "view_fov_degrees": 4.0,
        "updated_at_ms": 0,
    }))
    .unwrap();
    store.save_framing_draft(&framing, 0).unwrap();
    store
        .record_activation(&Activation {
            project_id: plan,
            revision: 0,
            framing_revision: 1,
            plan_revision: 1,
            coordinator_instance_id: store.instance_id(),
            applied_at_ms: 5,
            rigs: vec![activated(rig, catalog), activated(other_rig, other_catalog)],
            inactive_rigs: vec![],
            pushes: vec![],
        })
        .unwrap();
    Fixture {
        store,
        catalog,
        rig,
        other_rig,
        plan,
        path,
        _dir: dir,
    }
}

/// The tables with a text value that holds `id`.
fn tables_holding(path: &std::path::Path, id: Uuid) -> Vec<String> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut names = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .unwrap();
    let tables: Vec<String> = names
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut found = Vec::new();
    for table in tables {
        let mut columns = conn
            .prepare("SELECT name FROM pragma_table_info(?1)")
            .unwrap();
        let columns: Vec<String> = columns
            .query_map([&table], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let test = columns
            .iter()
            .map(|column| format!("instr(CAST(\"{column}\" AS TEXT),?1)>0"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let holds: bool = conn
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM \"{table}\" WHERE {test})"),
                [id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        if holds {
            found.push(table);
        }
    }
    found
}

#[test]
fn removing_a_rig_leaves_no_trace_of_it_and_keeps_its_plans() {
    let mut f = fixture();
    let review = f.store.rig_removal(f.catalog).unwrap().unwrap();
    assert_eq!(review.rig.name, "Removed rig");
    assert_eq!(review.plans, ["M31"]);
    assert_eq!(review.plugins, ["NINA"]);
    assert_eq!(review.blocker, None);
    assert!(!review.collaboration);

    assert_eq!(f.store.remove_rig(f.catalog).unwrap().unwrap(), review);

    assert_eq!(tables_holding(&f.path, f.rig), Vec::<String>::new());
    assert_eq!(tables_holding(&f.path, f.catalog), Vec::<String>::new());
    assert!(f.store.catalog_rig(f.catalog).unwrap().is_none());
    // The other rig keeps its part of the plan.
    let plan = f.store.plan_draft(f.plan).unwrap().unwrap();
    assert_eq!(plan.revision, 2);
    assert_eq!(plan.contributions.len(), 1);
    assert_eq!(plan.contributions[0].rig_id, f.other_rig);
    let framing = f.store.framing_draft(f.plan).unwrap().unwrap();
    assert_eq!(framing.panel_rig_id, None);
    assert_eq!(framing.shown_rig_ids, [f.other_rig]);
    assert!(framing.panel.is_some());
    let activation = f.store.activation(f.plan).unwrap().unwrap();
    assert_eq!(activation.rigs.len(), 1);
    assert_eq!(activation.rigs[0].rig_id, f.other_rig);
}

#[test]
fn removing_the_only_rig_of_a_plan_clears_its_activation() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let (catalog, rig) = bind(&mut store, "Only rig");
    let plan = store.create_project(Uuid::new_v4(), "M42").unwrap().id;
    store.save_plan_draft(&plan_with(plan, &[rig]), 0).unwrap();
    store
        .record_activation(&Activation {
            project_id: plan,
            revision: 0,
            framing_revision: 1,
            plan_revision: 1,
            coordinator_instance_id: store.instance_id(),
            applied_at_ms: 5,
            rigs: vec![activated(rig, catalog)],
            inactive_rigs: vec![],
            pushes: vec![],
        })
        .unwrap();

    store.remove_rig(catalog).unwrap().unwrap();

    assert_eq!(store.activation(plan).unwrap(), None);
    assert!(store
        .plan_draft(plan)
        .unwrap()
        .unwrap()
        .contributions
        .is_empty());
    assert!(store.project(plan).unwrap().is_some());
}

#[test]
fn open_work_refuses_the_removal_until_it_is_released() {
    let mut f = fixture();
    let conn = rusqlite::Connection::open(&f.path).unwrap();
    let allocation = Uuid::new_v4();
    conn.execute(
        "INSERT INTO execution_allocation VALUES(?1,?2,'{}')",
        [f.rig.to_string(), allocation.to_string()],
    )
    .unwrap();

    let review = f.store.rig_removal(f.catalog).unwrap().unwrap();
    assert_eq!(review.blocker, Some(RemovalBlocker::OpenWork));
    assert!(matches!(
        f.store.remove_rig(f.catalog),
        Err(Error::Conflict)
    ));
    assert!(f.store.catalog_rig(f.catalog).unwrap().is_some());

    conn.execute(
        "INSERT INTO workload_history VALUES(?1,?2,'{\"released\":true}')",
        [allocation.to_string(), f.rig.to_string()],
    )
    .unwrap();
    assert_eq!(
        f.store.rig_removal(f.catalog).unwrap().unwrap().blocker,
        None
    );
    f.store.remove_rig(f.catalog).unwrap().unwrap();
}

#[test]
fn a_catalog_with_no_rig_has_nothing_to_remove() {
    let mut f = fixture();
    let unknown = Uuid::new_v4();
    assert_eq!(f.store.rig_removal(unknown).unwrap(), None);
    assert_eq!(f.store.remove_rig(unknown).unwrap(), None);
}

#[test]
fn a_placed_project_can_take_its_real_profile_but_no_other_change_is_allowed() {
    use psf_guard_director_meta::{catalog::ProjectMapping, SYNC_PLACED_PROFILE};
    let dir = tempfile::tempdir().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let (catalog, rig) = bind(&mut store, "Rig");
    let guid = Uuid::new_v4();
    let project = store
        .adopt_source_projects(
            catalog,
            rig,
            &[SourceProject {
                source_project_guid: guid,
                source_profile_id: SYNC_PLACED_PROFILE.into(),
                proposed_project_id: Uuid::new_v4(),
                name: "M31".into(),
            }],
        )
        .unwrap()[0];
    let mapping = |profile: &str| ProjectMapping {
        catalog_id: catalog,
        source_project_guid: guid,
        source_profile_id: profile.into(),
        project_id: project,
        rig_id: rig,
    };

    store.link_catalog_projects(&[mapping("real")]).unwrap();
    let page = store.catalog_project_mappings(catalog, None, 10).unwrap();
    assert_eq!(page.items[0].source_profile_id, "real");

    assert!(matches!(
        store.link_catalog_projects(&[mapping("another")]),
        Err(Error::Conflict)
    ));
}

#[test]
fn a_rig_with_collaboration_records_keeps_them() {
    let mut f = fixture();
    // A connection row stays after Disconnect, and can hold reports not sent.
    rusqlite::Connection::open(&f.path)
        .unwrap()
        .execute(
            "INSERT INTO collaboration_connection VALUES(?1,?2,'https://collab.example',NULL,'{}')",
            [Uuid::new_v4().to_string(), f.rig.to_string()],
        )
        .unwrap();

    let review = f.store.rig_removal(f.catalog).unwrap().unwrap();
    assert!(review.collaboration);
    assert_eq!(review.blocker, None);
    assert!(matches!(
        f.store.remove_rig(f.catalog),
        Err(Error::Conflict)
    ));
    assert!(f.store.catalog_rig(f.catalog).unwrap().is_some());
}
