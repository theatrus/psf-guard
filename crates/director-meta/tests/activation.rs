use psf_guard_director_meta::{
    activation::{
        ActivatedPlan, ActivatedRig, ActivatedTarget, Activation, InactiveRig, RemotePush,
    },
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
                intent: None,
            }],
        }],
        inactive_rigs: vec![InactiveRig {
            rig_id: Uuid::new_v4(),
            catalog_id: Uuid::new_v4(),
            project_guid: Uuid::new_v4(),
        }],
        pushes: vec![],
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
    // A record written before rigs could be turned off reads with none.
    let mut old = serde_json::to_value(&record).unwrap();
    old.as_object_mut().unwrap().remove("inactive_rigs");
    let old: Activation = serde_json::from_value(old).unwrap();
    assert!(old.inactive_rigs.is_empty());
    // And an empty list is left out, so older builds still read new records.
    let none = Activation {
        inactive_rigs: vec![],
        ..record
    };
    assert!(serde_json::to_value(&none)
        .unwrap()
        .get("inactive_rigs")
        .is_none());
}

#[test]
fn validate_refuses_a_record_the_store_would_refuse() {
    let rig = ActivatedRig {
        rig_id: Uuid::new_v4(),
        catalog_id: Uuid::new_v4(),
        project_guid: Uuid::new_v4(),
        profile_id: "profile".into(),
        targets: vec![],
        plans: (0..5000)
            .map(|_| ActivatedPlan {
                contribution_id: Uuid::new_v4(),
                objective_id: Uuid::new_v4(),
                target_guid: Uuid::new_v4(),
                exposureplan_guid: Uuid::new_v4(),
                required_frames: 1,
                intent: None,
            })
            .collect(),
    };
    let record = Activation {
        project_id: Uuid::new_v4(),
        revision: 0,
        framing_revision: 1,
        plan_revision: 1,
        coordinator_instance_id: Uuid::new_v4(),
        applied_at_ms: 1,
        rigs: vec![rig],
        inactive_rigs: vec![],
        pushes: vec![],
    };
    assert!(record.validate().is_err());
    let small = Activation {
        rigs: vec![],
        ..record
    };
    assert!(small.validate().is_ok());
}

#[test]
fn a_push_lands_on_its_own_activation_and_never_on_a_newer_one() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Heart").unwrap();
    let (remote, local) = (Uuid::new_v4(), Uuid::new_v4());
    let rig = |rig_id| ActivatedRig {
        rig_id,
        catalog_id: Uuid::new_v4(),
        project_guid: Uuid::new_v4(),
        profile_id: "profile".into(),
        targets: vec![],
        plans: vec![],
    };
    let waiting = RemotePush {
        rig_id: remote,
        peer_id: "obs".into(),
        peer_name: "Observatory".into(),
        applied: false,
        attempted_at_ms: None,
        error: None,
    };
    let record = Activation {
        project_id: project.id,
        revision: 0,
        framing_revision: 1,
        plan_revision: 1,
        coordinator_instance_id: store.instance_id(),
        applied_at_ms: 1,
        rigs: vec![rig(remote), rig(local)],
        inactive_rigs: vec![],
        pushes: vec![waiting.clone()],
    };
    let first = store.record_activation(&record).unwrap();
    assert_eq!(store.pending_pushes("obs").unwrap(), vec![first.clone()]);
    assert!(store.pending_pushes("elsewhere").unwrap().is_empty());

    // A failed push says when and why, and still waits.
    let failed = RemotePush {
        attempted_at_ms: Some(10),
        error: Some("connection refused".into()),
        ..waiting.clone()
    };
    assert!(store
        .record_pushes(project.id, 1, std::slice::from_ref(&failed))
        .unwrap());
    let read = store.activation(project.id).unwrap().unwrap();
    assert_eq!(read.revision, 1, "a push leaves the revision alone");
    assert_eq!(read.pushes, vec![failed.clone()]);
    assert_eq!(store.pending_pushes("obs").unwrap().len(), 1);

    // A newer activation: a push of the old one is not recorded on it.
    store.record_activation(&record).unwrap();
    let sent = RemotePush {
        applied: true,
        attempted_at_ms: Some(20),
        error: None,
        ..waiting.clone()
    };
    assert!(!store
        .record_pushes(project.id, 1, std::slice::from_ref(&sent))
        .unwrap());
    assert_eq!(
        store.activation(project.id).unwrap().unwrap().pushes,
        vec![waiting.clone()]
    );
    assert!(store
        .record_pushes(project.id, 2, std::slice::from_ref(&sent))
        .unwrap());
    assert!(store.pending_pushes("obs").unwrap().is_empty());

    // Only rigs the activation reached, once each, with a bounded error.
    let stranger = RemotePush {
        rig_id: Uuid::new_v4(),
        ..waiting.clone()
    };
    assert!(matches!(
        store.record_pushes(project.id, 2, &[stranger]),
        Err(Error::InvalidInput)
    ));
    let twice = Activation {
        pushes: vec![waiting.clone(), waiting.clone()],
        ..record.clone()
    };
    assert!(twice.validate().is_err());
    let long = RemotePush {
        error: Some("x".repeat(501)),
        ..waiting.clone()
    };
    assert!(matches!(
        store.record_pushes(project.id, 2, &[long]),
        Err(Error::InvalidInput)
    ));
    // No record yet: nothing to land on.
    let bare = store.create_project(Uuid::new_v4(), "Bare").unwrap();
    assert!(!store.record_pushes(bare.id, 1, &[sent]).unwrap());
    // A record with no remote rig leaves the list out, as before.
    let none = Activation {
        pushes: vec![],
        ..record
    };
    assert!(serde_json::to_value(&none).unwrap().get("pushes").is_none());
}
