use psf_guard_director_meta::{
    plan::{Contribution, Goal, Objective, PlanDraft, TemplateChoice},
    Error, MetaStore, Uuid,
};
use tempfile::TempDir;

fn plan(project: Uuid, rig: Uuid) -> PlanDraft {
    let objective = Uuid::new_v4();
    PlanDraft {
        project_id: project,
        revision: 0,
        objectives: vec![Objective {
            id: objective,
            bandpass_id: "h_alpha".into(),
            purpose: "faint_detail".into(),
            goal: Goal::Hours { value: 6.0 },
            priority: 1,
        }],
        contributions: vec![Contribution {
            id: Uuid::new_v4(),
            objective_id: objective,
            rig_id: rig,
            template: TemplateChoice {
                template_guid: Some(Uuid::new_v4()),
                template_id: Some(3),
                name: "Ha 300s".into(),
                filter_name: "Ha".into(),
                gain: Some(100),
                offset: Some(30),
                bin: Some(1),
                readout_mode: None,
                moon: None,
            },
            exposure_seconds: 300.0,
            panel_ids: vec![],
            enabled: true,
        }],
        updated_at_ms: 1_000,
    }
}

#[test]
fn a_plan_is_saved_by_compare_and_set_and_survives_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Heart").unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "RedCat").unwrap();
    assert_eq!(store.plan_draft(project.id).unwrap(), None);
    assert!(matches!(
        store.save_plan_draft(&plan(Uuid::new_v4(), rig.id), 0),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.save_plan_draft(&plan(project.id, Uuid::new_v4()), 0),
        Err(Error::NotFound)
    ));
    let first = plan(project.id, rig.id);
    let saved = store.save_plan_draft(&first, 0).unwrap();
    assert_eq!(saved.revision, 1);
    assert!(matches!(
        store.save_plan_draft(&first, 0),
        Err(Error::Conflict)
    ));
    let mut same = saved.clone();
    same.updated_at_ms = 2_000;
    assert_eq!(store.save_plan_draft(&same, 1).unwrap(), saved);
    let mut changed = saved.clone();
    changed.objectives[0].goal = Goal::Frames { value: 80 };
    let second = store.save_plan_draft(&changed, 1).unwrap();
    assert_eq!(second.revision, 2);
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.plan_draft(project.id).unwrap(), Some(second));
}

#[test]
fn dangling_and_malformed_plans_are_refused() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Heart").unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "RedCat").unwrap();
    let mut bad = plan(project.id, rig.id);
    bad.contributions[0].objective_id = Uuid::new_v4();
    assert!(matches!(
        store.save_plan_draft(&bad, 0),
        Err(Error::InvalidInput)
    ));
    let mut bad = plan(project.id, rig.id);
    bad.objectives[0].bandpass_id = "H-alpha".into();
    assert!(matches!(
        store.save_plan_draft(&bad, 0),
        Err(Error::InvalidInput)
    ));
    let mut bad = plan(project.id, rig.id);
    bad.objectives[0].goal = Goal::Hours { value: 0.0 };
    assert!(matches!(
        store.save_plan_draft(&bad, 0),
        Err(Error::InvalidInput)
    ));
    let mut bad = plan(project.id, rig.id);
    bad.contributions[0].exposure_seconds = -1.0;
    assert!(matches!(
        store.save_plan_draft(&bad, 0),
        Err(Error::InvalidInput)
    ));
    let mut bad = plan(project.id, rig.id);
    bad.contributions[0].template.filter_name = String::new();
    assert!(matches!(
        store.save_plan_draft(&bad, 0),
        Err(Error::InvalidInput)
    ));
    let mut empty = PlanDraft::empty(project.id, 0);
    empty.objectives.clear();
    assert_eq!(store.save_plan_draft(&empty, 0).unwrap().revision, 1);
}
