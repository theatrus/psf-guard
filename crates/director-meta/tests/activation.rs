use psf_guard_director_meta::{
    activation::{ActivatedPlan, ActivatedRig, ActivatedTarget, Activation},
    Error, MetaStore, Uuid,
};
use tempfile::TempDir;

#[test]
fn activations_advance_a_revision_and_read_back_after_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Heart").unwrap();
    assert_eq!(store.activation(project.id).unwrap(), None);
    let target = Uuid::new_v4();
    let record = Activation {
        project_id: project.id,
        revision: 0,
        framing_revision: 2,
        plan_revision: 3,
        coordinator_instance_id: store.instance_id(),
        applied_at_ms: 5,
        rigs: vec![ActivatedRig {
            rig_id: Uuid::new_v4(),
            catalog_id: Uuid::new_v4(),
            project_guid: Uuid::new_v4(),
            profile_id: "profile".into(),
            targets: vec![ActivatedTarget {
                panel_id: "r1c1".into(),
                target_guid: target,
            }],
            plans: vec![ActivatedPlan {
                contribution_id: Uuid::new_v4(),
                objective_id: Uuid::new_v4(),
                target_guid: target,
                exposureplan_guid: Uuid::new_v4(),
                required_frames: 72,
            }],
        }],
    };
    assert!(matches!(
        store.record_activation(&Activation {
            project_id: Uuid::new_v4(),
            ..record.clone()
        }),
        Err(Error::NotFound)
    ));
    let first = store.record_activation(&record).unwrap();
    assert_eq!(first.revision, 1);
    let second = store.record_activation(&record).unwrap();
    assert_eq!(second.revision, 2);
    let mut bad = record.clone();
    bad.rigs[0].profile_id = String::new();
    assert!(matches!(
        store.record_activation(&bad),
        Err(Error::InvalidInput)
    ));
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.activation(project.id).unwrap(), Some(second));
}
