#[path = "../../director-interop/tests/support/mod.rs"]
mod support;
use psf_guard_director_interop::collaboration::*;
use psf_guard_director_meta::{
    collaboration_connection::{BackgroundPolicy, ConnectionBinding, ConnectionState},
    collaboration_leave::{NightlyIntake, RemoteTask},
    transfer::State,
    Error, MetaStore, Uuid,
};
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

/// A second remote project on the same server, with its own task.
fn other_project() -> PreparedImport {
    let mut v = wire();
    v.as_object_mut().unwrap().remove("task");
    v["tasks"][0]["id"] = json!("000000000005");
    v["tasks"][0]["project"] = json!("000000000003");
    v["tasks"][0]["projectName"] = json!("NGC 7000 wall");
    let requirements = v["requirementsByProject"]["000000000002"].clone();
    v["requirementsByProject"]["000000000003"] = requirements;
    prepared(&v)
}

/// A registered connection for the fixtures' agent whose nightly intake
/// lists `projects`.
fn connection(s: &mut MetaStore, rig: Uuid, projects: &[&str]) -> ConnectionBinding {
    let binding = ConnectionBinding {
        id: Uuid::new_v4(),
        rig_id: rig,
        base_url: source().base_url().into(),
        name: "Community".into(),
        allow_loopback_http: false,
        agent_id: None,
        state: ConnectionState::New,
        settings: None,
        background: None,
    };
    s.create_collaboration_connection(&binding).unwrap();
    let registered = ConnectionBinding {
        agent_id: Some(source().agent_id().into()),
        state: ConnectionState::Registered,
        background: Some(BackgroundPolicy {
            enabled: true,
            catalog_id: Uuid::new_v4(),
            project_ids: projects.iter().map(|p| p.to_string()).collect(),
            interval_minutes: 15,
            activate: false,
            automatic_reports: true,
        }),
        ..binding.clone()
    };
    s.update_collaboration_connection(&binding, &registered)
        .unwrap();
    s.complete_collaboration_night(registered.id, NIGHT, 5000)
        .unwrap();
    registered
}

fn report(s: &mut MetaStore, p: &PreparedImport, frames: &[u128], now: u64) -> Uuid {
    let frames: Vec<_> = frames.iter().map(|n| frame(p, *n)).collect();
    s.queue_collaboration_report(&finalize_contribution(p, &frames).unwrap(), now)
        .unwrap()
        .id
}

#[test]
fn leaving_removes_one_plans_collaboration_records_and_keeps_the_plan() {
    let (_dir, mut s, rig) = store();
    let p = import();
    let q = other_project();
    apply(&mut s, &p, rig);
    apply(&mut s, &q, rig);
    let b = connection(&mut s, rig, &["000000000002", "000000000003"]);
    // One report sent and acknowledged, and an extension of it still queued.
    let sent = report(&mut s, &p, &[1, 2], 2000);
    let reply = serde_json::to_vec(&json!({"recorded":[reply(1, true)]})).unwrap();
    s.acknowledge_collaboration_reports(&source(), &[sent], &reply, 3000)
        .unwrap();
    report(&mut s, &p, &[1, 2, 3], 4000);
    // A file transfer for the plan's import, and one for the other plan's.
    let request =
        |import: Uuid| json!({"selection":{"import_id":import,"catalog":"rig","panel":0}});
    let ours = s
        .create_transfer(b.id, request(p.import_id()), 4000)
        .unwrap();
    let theirs = s
        .create_transfer(b.id, request(q.import_id()), 4000)
        .unwrap();

    let preview = s.plan_collaboration(p.project_id()).unwrap().unwrap();
    assert_eq!(preview.base_url, source().base_url());
    assert_eq!(preview.remote_project_id, "000000000002");
    assert_eq!(
        preview.remote_project_name.as_deref(),
        Some("M31 halo in narrowband")
    );
    assert_eq!(preview.import_ids, vec![p.import_id()]);
    assert_eq!((preview.sent_reports, preview.unsent_reports), (1, 1));
    assert_eq!(preview.reported_frames, 3);
    assert_eq!(preview.transfers, 1);
    assert_eq!(
        preview.nightly_intake,
        vec![NightlyIntake {
            connection_id: b.id,
            name: "Community".into()
        }]
    );
    assert_eq!(
        preview.tasks,
        vec![RemoteTask {
            agent_id: "000000000001".into(),
            task_id: "000000000004".into(),
            rig: "Rig".into()
        }]
    );

    let left = s.leave_collaboration(p.project_id()).unwrap();
    assert_eq!(left, preview);

    // The plan stays, as a plan of its own.
    assert!(s.project(p.project_id()).unwrap().is_some());
    assert!(s.framing_draft(p.project_id()).unwrap().is_some());
    assert!(s.plan_draft(p.project_id()).unwrap().is_some());
    assert!(s.plan_collaboration(p.project_id()).unwrap().is_none());
    assert!(!s.plan_from_collaboration(p.project_id()).unwrap());
    assert!(!s.collaboration_requires_admission(p.project_id()).unwrap());
    // Nothing treats it as collaboration work.
    assert!(s.collaboration_import(p.import_id()).unwrap().is_none());
    assert!(s.collaboration_report(sent).unwrap().is_none());
    assert!(s
        .pending_collaboration_reports(&source(), 16)
        .unwrap()
        .is_empty());
    assert!(s.reported_image_guids().unwrap().is_empty());
    let imports = s.collaboration_imports_for_connection(b.id).unwrap();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].plan.import_id(), q.import_id());
    let projects = s.collaboration_connection_projects(b.id).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].0, "000000000003");
    assert_eq!(
        s.transfer(ours.id).unwrap().unwrap().state,
        State::Cancelled
    );
    assert_eq!(
        s.transfer(theirs.id).unwrap().unwrap().state,
        State::Preparing
    );

    // Nightly intake no longer lists the project, and fetches again.
    let policy = s
        .collaboration_connection(b.id)
        .unwrap()
        .unwrap()
        .background
        .unwrap();
    assert_eq!(policy.project_ids, vec!["000000000003".to_string()]);
    assert!(policy.enabled);
    assert!(s.collaboration_nightly_run(b.id).unwrap().is_none());

    // The other plan keeps its collaboration.
    assert!(s.plan_collaboration(q.project_id()).unwrap().is_some());
    assert!(s.collaboration_import(q.import_id()).unwrap().is_some());

    // Leaving twice, or a plan of its own, is refused.
    assert!(matches!(
        s.leave_collaboration(p.project_id()),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        s.leave_collaboration(Uuid::new_v4()),
        Err(Error::NotFound)
    ));
}

#[test]
fn leaving_the_last_listed_project_turns_nightly_intake_off() {
    let (_dir, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let b = connection(&mut s, rig, &["000000000002"]);
    s.leave_collaboration(p.project_id()).unwrap();
    let policy = s
        .collaboration_connection(b.id)
        .unwrap()
        .unwrap()
        .background
        .unwrap();
    assert!(policy.project_ids.is_empty());
    assert!(!policy.enabled);
    // Automatic reports keep their own consent.
    assert!(policy.automatic_reports);
}

#[test]
fn deleting_an_imported_plan_leaves_its_collaboration_unless_frames_were_reported() {
    let (_dir, mut s, rig) = store();
    let p = import();
    let q = other_project();
    apply(&mut s, &p, rig);
    apply(&mut s, &q, rig);
    let b = connection(&mut s, rig, &["000000000002", "000000000003"]);

    // Nothing reported: the plan and its collaboration records go together.
    // A caller that previewed a plan of its own is refused.
    assert!(matches!(
        s.delete_plan(q.project_id(), &[], false),
        Err(Error::Conflict)
    ));
    let left = s.delete_plan(q.project_id(), &[], true).unwrap().unwrap();
    assert_eq!(left.remote_project_id, "000000000003");
    assert!(s.project(q.project_id()).unwrap().is_none());
    assert!(s.collaboration_import(q.import_id()).unwrap().is_none());
    assert_eq!(
        s.collaboration_connection(b.id)
            .unwrap()
            .unwrap()
            .background
            .unwrap()
            .project_ids,
        vec!["000000000002".to_string()]
    );
    assert!(s.collaboration_import(p.import_id()).unwrap().is_some());

    // Queued frames keep the plan until it leaves the collaboration.
    report(&mut s, &p, &[1], 2000);
    assert!(matches!(
        s.delete_plan(p.project_id(), &[], true),
        Err(Error::Conflict)
    ));
    assert!(s.plan_collaboration(p.project_id()).unwrap().is_some());
    s.leave_collaboration(p.project_id()).unwrap();
    assert_eq!(s.delete_plan(p.project_id(), &[], false).unwrap(), None);
    assert!(s.project(p.project_id()).unwrap().is_none());
}

#[test]
fn importing_a_project_again_rejoins_the_plan_that_left_it() {
    let (_dir, mut s, rig) = store();
    let p = import();
    apply(&mut s, &p, rig);
    let plan = s.project(p.project_id()).unwrap().unwrap();
    s.rename_project(plan.id, plan.revision, "My M31").unwrap();
    let framing = s.framing_draft(p.project_id()).unwrap().unwrap().revision;
    s.leave_collaboration(p.project_id()).unwrap();

    let preview = s.preview_collaboration_import(&p, rig).unwrap();
    assert_eq!(preview.rejoins.as_deref(), Some("My M31"));
    s.apply_collaboration_import(&p, rig, &preview.review_digest, 6000)
        .unwrap();
    assert!(s.plan_from_collaboration(p.project_id()).unwrap());
    assert!(s.collaboration_import(p.import_id()).unwrap().is_some());
    // The plan keeps its own name and framing.
    assert_eq!(s.project(p.project_id()).unwrap().unwrap().name, "My M31");
    assert_eq!(
        s.framing_draft(p.project_id()).unwrap().unwrap().revision,
        framing
    );
    // Once linked, a later import is an ordinary refresh.
    assert_eq!(
        s.preview_collaboration_import(&p, rig).unwrap().rejoins,
        None
    );
}
