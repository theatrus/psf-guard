use psf_guard_director_core::priority::{Factor, Scope};
use psf_guard_director_meta::{preferences::Settings, Error, MetaStore, Uuid};
use tempfile::TempDir;

#[test]
fn inherited_preferences_are_revisioned_and_survive_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let site = store.create_site(Uuid::new_v4(), "Site").unwrap().id;
    let project = store.create_project(Uuid::new_v4(), "Project").unwrap().id;
    assert!(
        !store
            .effective_observing_preferences(rig, Some(project))
            .unwrap()
            .enabled
    );
    let mut global = Settings::empty(Scope::Global, store.instance_id());
    global.enabled = Some(true);
    let global = store.save_observing_settings(&global).unwrap();
    let mut settings = Settings::empty(Scope::Site, site);
    settings
        .overrides
        .weights
        .insert(Factor::MoonOpportunity, 80);
    store.save_observing_settings(&settings).unwrap();
    let mut settings = Settings::empty(Scope::Rig, rig);
    settings.site_id = Some(site);
    store.save_observing_settings(&settings).unwrap();
    let mut settings = Settings::empty(Scope::Project, project);
    settings.overrides.importance = Some(0);
    settings.overrides.minimum_dwell_ms = Some(0);
    let saved = store.save_observing_settings(&settings).unwrap();
    assert!(matches!(
        store.save_observing_settings(&settings),
        Err(Error::Conflict)
    ));
    assert_eq!(
        store.observing_settings(Scope::Project, project).unwrap(),
        saved
    );
    let before = store
        .effective_observing_preferences(rig, Some(project))
        .unwrap();
    assert!(before.enabled);
    assert_eq!(before.resolved.policy().importance, 0);
    assert_eq!(
        before.resolved.policy().weights[&Factor::MoonOpportunity],
        80
    );
    assert_eq!(
        before.resolved.provenance().importance.scope,
        Scope::Project
    );
    assert_eq!(
        before.resolved.provenance().weights[&Factor::MoonOpportunity].scope,
        Scope::Site
    );
    let mut global = global;
    global.enabled = Some(false);
    store.save_observing_settings(&global).unwrap();
    assert!(before.enabled, "issued snapshot is immutable");
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    let after = store
        .effective_observing_preferences(rig, Some(project))
        .unwrap();
    assert!(!after.enabled);
    assert_eq!(after.resolved.policy(), before.resolved.policy());
}

#[test]
fn settings_reject_wrong_scope_unknown_site_and_invalid_numbers() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let project = store.create_project(Uuid::new_v4(), "Project").unwrap().id;
    for case in 0..5 {
        let mut settings = Settings::empty(Scope::Rig, rig);
        match case {
            0 => settings.site_id = Some(Uuid::new_v4()),
            1 => settings.scope = Scope::Global,
            2 => settings.overrides.importance = Some(101),
            3 => {
                settings.scope = Scope::Project;
                settings.scope_id = project;
                settings.enabled = Some(true);
            }
            _ => settings
                .overrides
                .weights
                .insert(Factor::Altitude, 1001)
                .map(|_| ())
                .unwrap_or(()),
        }
        assert!(store.save_observing_settings(&settings).is_err());
    }
    assert_eq!(
        store.observing_settings(Scope::Rig, rig).unwrap().revision,
        0
    );
}

#[test]
fn parent_edit_that_zeroes_a_child_is_rolled_back() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let mut child = Settings::empty(Scope::Rig, rig);
    child.overrides.weights.insert(Factor::Altitude, 0);
    store.save_observing_settings(&child).unwrap();
    let mut global = Settings::empty(Scope::Global, store.instance_id());
    global.overrides.weights = Factor::ALL
        .into_iter()
        .map(|f| (f, if f == Factor::Altitude { 100 } else { 0 }))
        .collect();
    assert!(matches!(
        store.save_observing_settings(&global),
        Err(Error::InvalidInput)
    ));
    assert_eq!(
        store
            .observing_settings(Scope::Global, store.instance_id())
            .unwrap()
            .revision,
        0
    );
    assert!(store.effective_observing_preferences(rig, None).is_ok());
}

#[test]
fn project_attachment_preserves_preferences_without_orphaning_records() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let keep = store.create_project(Uuid::new_v4(), "Keep").unwrap().id;
    for importance in [80, 20] {
        let from = store.create_project(Uuid::new_v4(), "Absorbed").unwrap().id;
        let mut settings = Settings::empty(Scope::Project, from);
        settings.overrides.importance = Some(importance);
        store.save_observing_settings(&settings).unwrap();
        store.attach_project(keep, from).unwrap();
        assert_eq!(
            store
                .effective_observing_preferences(rig, Some(keep))
                .unwrap()
                .resolved
                .policy()
                .importance,
            80
        );
        let mut global = store
            .observing_settings(Scope::Global, store.instance_id())
            .unwrap();
        global.enabled = Some(true);
        store.save_observing_settings(&global).unwrap();
    }
}

#[test]
fn project_order_inherits_and_overrides_without_project_weights() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let site = store.create_site(Uuid::new_v4(), "Site").unwrap().id;
    let a = store.create_project(Uuid::new_v4(), "A").unwrap().id;
    let b = store.create_project(Uuid::new_v4(), "B").unwrap().id;
    let mut global = Settings::empty(Scope::Global, store.instance_id());
    global.project_order = Some(vec![a, b]);
    store.save_observing_settings(&global).unwrap();
    let mut rig_settings = Settings::empty(Scope::Rig, rig);
    rig_settings.site_id = Some(site);
    let mut rig_settings = store.save_observing_settings(&rig_settings).unwrap();
    let effective = store.effective_observing_preferences(rig, None).unwrap();
    assert_eq!(effective.project_order, Some(vec![a, b]));
    assert_eq!(effective.order_source.unwrap().scope, Scope::Global);
    let mut site_settings = Settings::empty(Scope::Site, site);
    site_settings.project_order = Some(vec![b, a]);
    store.save_observing_settings(&site_settings).unwrap();
    let effective = store.effective_observing_preferences(rig, Some(a)).unwrap();
    assert_eq!(effective.project_order, Some(vec![b, a]));
    assert_eq!(effective.order_source.unwrap().scope, Scope::Site);
    rig_settings.project_order = Some(vec![a]);
    let mut rig_settings = store.save_observing_settings(&rig_settings).unwrap();
    assert_eq!(
        store
            .effective_observing_preferences(rig, None)
            .unwrap()
            .project_order,
        Some(vec![a])
    );
    rig_settings.project_order = None;
    store.save_observing_settings(&rig_settings).unwrap();
    assert!(matches!(
        store.save_observing_settings(&rig_settings),
        Err(Error::Conflict)
    ));
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store
            .effective_observing_preferences(rig, None)
            .unwrap()
            .project_order,
        Some(vec![b, a])
    );
}

#[test]
fn project_orders_validate_identity_duplicates_limits_and_scope() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Project").unwrap().id;
    for order in [
        vec![project, project],
        vec![Uuid::nil()],
        vec![Uuid::new_v4()],
        (0..257).map(|_| Uuid::new_v4()).collect(),
    ] {
        let mut settings = Settings::empty(Scope::Global, store.instance_id());
        settings.project_order = Some(order);
        assert!(store.save_observing_settings(&settings).is_err());
    }
    let mut settings = Settings::empty(Scope::Project, project);
    settings.project_order = Some(vec![project]);
    assert!(matches!(
        store.save_observing_settings(&settings),
        Err(Error::InvalidInput)
    ));
}

#[test]
fn attaching_projects_preserves_order_and_invalidates_stale_editors() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let keep = store.create_project(Uuid::new_v4(), "Keep").unwrap().id;
    let first = store.create_project(Uuid::new_v4(), "First").unwrap().id;
    let second = store.create_project(Uuid::new_v4(), "Second").unwrap().id;
    let mut global = Settings::empty(Scope::Global, store.instance_id());
    global.project_order = Some(vec![first, second]);
    let stale = store.save_observing_settings(&global).unwrap();
    store.attach_project(keep, first).unwrap();
    let saved = store
        .observing_settings(Scope::Global, store.instance_id())
        .unwrap();
    assert_eq!(saved.project_order, Some(vec![keep, second]));
    assert_eq!(saved.revision, stale.revision + 1);
    store.attach_project(keep, second).unwrap();
    let saved = store
        .observing_settings(Scope::Global, store.instance_id())
        .unwrap();
    assert_eq!(saved.project_order, Some(vec![keep]));
    assert_eq!(saved.revision, stale.revision + 2);
    assert!(store.save_observing_settings(&stale).is_err());
}

#[test]
fn schema_19_preferences_without_order_keep_legacy_behavior() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let mut global = Settings::empty(Scope::Global, store.instance_id());
    global.enabled = Some(true);
    store.save_observing_settings(&global).unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE observing_preferences SET payload=json_remove(payload,'$.project_order')",
        [],
    )
    .unwrap();
    conn.pragma_update(None, "user_version", 19).unwrap();
    drop(conn);
    let store = MetaStore::open(&path).unwrap();
    let effective = store.effective_observing_preferences(rig, None).unwrap();
    assert!(effective.enabled);
    assert!(effective.project_order.is_none());
}

#[test]
fn scheduling_limits_inherit_and_say_where_each_came_from() {
    use psf_guard_director_meta::preferences::{
        resolve_scheduling, SchedulingOverrides, SchedulingValues,
    };
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let site = store.create_site(Uuid::new_v4(), "Site").unwrap().id;
    let project = store.create_project(Uuid::new_v4(), "Project").unwrap().id;
    let untouched = store
        .effective_observing_preferences(rig, Some(project))
        .unwrap()
        .scheduling;
    assert_eq!(untouched.values, SchedulingValues::default());
    assert!(
        untouched.sources.is_empty(),
        "nothing set, so activation leaves existing projects alone"
    );

    let mut global = Settings::empty(Scope::Global, store.instance_id());
    global.scheduling.minimum_altitude_degrees = Some(20.0);
    global.scheduling.dither_every = Some(5);
    store.save_observing_settings(&global).unwrap();
    let mut settings = Settings::empty(Scope::Site, site);
    settings.scheduling.horizon_offset_degrees = Some(4.5);
    settings.scheduling.use_custom_horizon = Some(true);
    store.save_observing_settings(&settings).unwrap();
    let mut settings = Settings::empty(Scope::Rig, rig);
    settings.site_id = Some(site);
    settings.scheduling.dither_every = Some(2);
    store.save_observing_settings(&settings).unwrap();
    let mut settings = Settings::empty(Scope::Project, project);
    settings.scheduling.minimum_altitude_degrees = Some(35.0);
    store.save_observing_settings(&settings).unwrap();

    let resolved = store
        .effective_observing_preferences(rig, Some(project))
        .unwrap()
        .scheduling;
    assert_eq!(resolved.values.minimum_altitude_degrees, 35.0);
    assert_eq!(resolved.values.dither_every, 2);
    assert_eq!(resolved.values.horizon_offset_degrees, 4.5);
    assert!(resolved.values.use_custom_horizon);
    assert_eq!(
        resolved.values.minimum_time_minutes, 30,
        "Target Scheduler's default"
    );
    let scope = |field: &str| resolved.sources.get(field).map(|source| source.scope);
    assert_eq!(scope("minimum_altitude_degrees"), Some(Scope::Project));
    assert_eq!(scope("dither_every"), Some(Scope::Rig));
    assert_eq!(scope("horizon_offset_degrees"), Some(Scope::Site));
    assert_eq!(scope("minimum_time_minutes"), None);

    // Another project on the same rig gets the defaults, not this override.
    let other = store.create_project(Uuid::new_v4(), "Other").unwrap().id;
    let defaults = store
        .effective_observing_preferences(rig, Some(other))
        .unwrap()
        .scheduling;
    assert_eq!(defaults.values.minimum_altitude_degrees, 20.0);

    // Out of range or out of order is refused.
    for bad in [
        SchedulingOverrides {
            minimum_altitude_degrees: Some(95.0),
            ..Default::default()
        },
        SchedulingOverrides {
            minimum_altitude_degrees: Some(40.0),
            maximum_altitude_degrees: Some(30.0),
            ..Default::default()
        },
        SchedulingOverrides {
            horizon_offset_degrees: Some(f64::NAN),
            ..Default::default()
        },
        SchedulingOverrides {
            meridian_window_minutes: Some(24 * 60),
            ..Default::default()
        },
    ] {
        let mut settings = store
            .observing_settings(Scope::Global, store.instance_id())
            .unwrap();
        settings.scheduling = bad;
        assert!(matches!(
            store.save_observing_settings(&settings),
            Err(Error::InvalidInput)
        ));
    }
    // A maximum of 0 means no upper limit, so it never conflicts.
    let mut settings = store
        .observing_settings(Scope::Global, store.instance_id())
        .unwrap();
    settings.scheduling.maximum_altitude_degrees = Some(0.0);
    store.save_observing_settings(&settings).unwrap();

    // Settings with no limits serialise as before.
    let empty = Settings::empty(Scope::Rig, rig);
    assert!(!serde_json::to_string(&empty)
        .unwrap()
        .contains("scheduling"));
    assert!(resolve_scheduling(&[]).sources.is_empty());
}
