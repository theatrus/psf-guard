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
