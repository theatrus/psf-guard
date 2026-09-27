use super::*;

pub(super) const RIG_PREVIEW: &str = "/catalogs/catalog/rig/preview";
pub(super) const RIG_APPLY: &str = "/catalogs/catalog/rig/apply";

async fn review(f: &Fixture) -> Value {
    let (status, result) = call(
        &f.app,
        "POST",
        RIG_PREVIEW,
        json!({"catalog_id":f.catalog}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    result["data"].clone()
}
fn request(f: &Fixture, reviewed: &Value) -> Value {
    json!({"plan":{"catalog_id":f.catalog},"preview_digest":reviewed["preview_digest"]})
}

#[tokio::test]
async fn database_rig_preview_apply_and_retry_preserve_grades_and_source_project() {
    let f = Fixture::new();
    let reviewed = review(&f).await;
    assert_eq!(reviewed["binding"]["rig"]["id"], f.catalog.to_string());
    assert_eq!(crate::catalog_identity::read(&f.source).unwrap(), None);
    assert!(f
        .state
        .director
        .as_ref()
        .unwrap()
        .store
        .lock()
        .unwrap()
        .catalog_rig(f.catalog)
        .unwrap()
        .is_none());
    for _ in 0..2 {
        let (status, applied) = call(&f.app, "POST", RIG_APPLY, request(&f, &reviewed), None).await;
        assert_eq!(status, StatusCode::OK, "{applied}");
        assert_eq!(applied["data"]["binding"], reviewed["binding"]);
    }
    let (status, inventory) = call(
        &f.app,
        "GET",
        "/catalogs/catalog/mappings",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(inventory["data"]["rig"]["id"], f.catalog.to_string());
    assert_eq!(inventory["data"]["items"], json!([]));
    assert_eq!(
        f.source
            .query_row("SELECT gradingStatus FROM acquiredimage", [], |r| r
                .get::<_, i32>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        f.source
            .query_row("SELECT name FROM project", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "M31"
    );
}

#[tokio::test]
async fn changed_source_refuses_the_old_review_without_creating_a_rig() {
    let f = Fixture::new();
    let reviewed = review(&f).await;
    f.source
        .execute("UPDATE project SET name='Changed'", [])
        .unwrap();
    assert_eq!(
        call(&f.app, "POST", RIG_APPLY, request(&f, &reviewed), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert!(f
        .state
        .director
        .as_ref()
        .unwrap()
        .store
        .lock()
        .unwrap()
        .rig(f.catalog)
        .unwrap()
        .is_none());
    assert_eq!(crate::catalog_identity::read(&f.source).unwrap(), None);
}

#[tokio::test]
async fn durable_source_identity_after_interruption_allows_the_exact_rig_retry() {
    let mut f = Fixture::new();
    let reviewed = review(&f).await;
    let identity: CatalogIdentity =
        serde_json::from_value(reviewed["binding"]["catalog"].clone()).unwrap();
    let mut tx = f.source.transaction().unwrap();
    crate::catalog_identity::adopt(&mut tx, identity).unwrap();
    tx.commit().unwrap();
    let (status, applied) = call(&f.app, "POST", RIG_APPLY, request(&f, &reviewed), None).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["data"]["binding"], reviewed["binding"]);
}

#[tokio::test]
async fn source_commit_lock_rolls_back_the_rig_and_retries_after_release() {
    let f = Fixture::new();
    f.source
        .execute_batch("PRAGMA journal_mode=DELETE")
        .unwrap();
    let reviewed = review(&f).await;
    f.source
        .execute_batch("BEGIN; SELECT * FROM project;")
        .unwrap();
    assert_eq!(
        call(&f.app, "POST", RIG_APPLY, request(&f, &reviewed), None)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(f
        .state
        .director
        .as_ref()
        .unwrap()
        .store
        .lock()
        .unwrap()
        .catalog_rig(f.catalog)
        .unwrap()
        .is_none());
    f.source.execute_batch("ROLLBACK").unwrap();
    assert_eq!(crate::catalog_identity::read(&f.source).unwrap(), None);
    assert_eq!(
        call(&f.app, "POST", RIG_APPLY, request(&f, &reviewed), None)
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn an_empty_registered_catalog_can_enable_planning_without_a_project_or_profile() {
    let f = Fixture::new();
    f.source
        .execute_batch("DELETE FROM acquiredimage; DELETE FROM project")
        .unwrap();
    let reviewed = review(&f).await;
    assert_eq!(
        call(&f.app, "POST", RIG_APPLY, request(&f, &reviewed), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.source
            .query_row("SELECT COUNT(*) FROM project", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        0
    );
}
