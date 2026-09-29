use psf_guard_director_meta::{templates::ExposureTemplate, Error, MetaStore};
use uuid::Uuid;

fn scratch() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("director-meta-templates-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("meta.sqlite")
}

fn template(name: &str, filter: &str) -> ExposureTemplate {
    ExposureTemplate {
        id: Uuid::new_v4(),
        revision: 0,
        name: name.to_owned(),
        filter_name: filter.to_owned(),
        gain: Some(100),
        offset: Some(30),
        bin: Some(1),
        readout_mode: None,
        default_exposure_seconds: 300.0,
        updated_at_ms: 1,
    }
}

#[test]
fn the_library_saves_lists_updates_and_deletes_with_compare_and_set() {
    let path = scratch();
    let mut store = MetaStore::create(&path).unwrap();
    assert!(store.templates().unwrap().is_empty());

    let ha = store.save_template(&template("Ha 300", "Ha"), 0).unwrap();
    assert_eq!(ha.revision, 1);
    let lum = store.save_template(&template("Lum 90", "L"), 0).unwrap();
    // Listed by filter, then name.
    let names: Vec<_> = store
        .templates()
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["Ha 300", "Lum 90"]);

    // Saving the same thing again is not a new revision; a change is.
    assert_eq!(store.save_template(&ha, 1).unwrap().revision, 1);
    let mut longer = ha.clone();
    longer.default_exposure_seconds = 600.0;
    let saved = store.save_template(&longer, 1).unwrap();
    assert_eq!(saved.revision, 2);
    assert_eq!(
        store
            .template(ha.id)
            .unwrap()
            .unwrap()
            .default_exposure_seconds,
        600.0
    );
    // A stale revision conflicts, on save and on delete.
    assert!(matches!(
        store.save_template(&longer, 1),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store.delete_template(ha.id, 1),
        Err(Error::Conflict)
    ));
    store.delete_template(ha.id, 2).unwrap();
    assert!(store.template(ha.id).unwrap().is_none());
    assert!(matches!(
        store.delete_template(ha.id, 2),
        Err(Error::NotFound)
    ));
    assert_eq!(store.templates().unwrap().len(), 1);
    assert_eq!(store.templates().unwrap()[0].id, lum.id);

    // Bad input is refused before it is stored.
    let mut blank = template("", "Ha");
    assert!(matches!(
        store.save_template(&blank, 0),
        Err(Error::InvalidInput)
    ));
    blank.name = "x".into();
    blank.bin = Some(0);
    assert!(matches!(
        store.save_template(&blank, 0),
        Err(Error::InvalidInput)
    ));
    blank.bin = Some(2);
    blank.default_exposure_seconds = 0.0;
    assert!(matches!(
        store.save_template(&blank, 0),
        Err(Error::InvalidInput)
    ));

    // The library survives a reopen and a fresh reader sees it.
    drop(store);
    let again = MetaStore::open(&path).unwrap();
    assert_eq!(again.templates().unwrap()[0].name, "Lum 90");
    assert_eq!(
        MetaStore::open_reader(&path)
            .unwrap()
            .templates()
            .unwrap()
            .len(),
        1
    );
}
