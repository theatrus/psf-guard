use super::*;
use psf_guard_director_meta::CatalogIdentity;
use rusqlite::{Connection, TransactionBehavior};

const PREVIEW: &str = "/catalogs/catalog/adoption/preview";
const APPLY: &str = "/catalogs/catalog/adoption/apply";

struct Fixture {
    _dir: TempDir,
    state: Arc<AppState>,
    app: Router,
    source: Connection,
    plan: Value,
    catalog: Uuid,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let state = Arc::new(state(&dir, true));
        let path = dir.path().join("catalog.sqlite");
        let source = Connection::open(&path).unwrap();
        source
            .execute_batch(
                "CREATE TABLE project(Id INTEGER PRIMARY KEY,name TEXT,guid TEXT,profileId TEXT);
            CREATE TABLE acquiredimage(Id INTEGER PRIMARY KEY,gradingStatus INTEGER);
            INSERT INTO acquiredimage VALUES(1,2);",
            )
            .unwrap();
        let guid = Uuid::new_v4();
        source
            .execute(
                "INSERT INTO project VALUES(1,'M31',?1,'profile-a')",
                [guid.to_string()],
            )
            .unwrap();
        let context = super::super::super::database_context::DatabaseContext::new(
            "catalog".into(),
            "Catalog".into(),
            path.to_string_lossy().into(),
            vec![dir.path().to_string_lossy().into()],
            None,
            None,
            None,
            dir.path().join("cache").to_string_lossy().into(),
        )
        .unwrap();
        state
            .databases
            .write()
            .unwrap()
            .insert("catalog".into(), Arc::new(context));
        let catalog = Uuid::new_v4();
        let (project, rig) = {
            let mut store = state.director.as_ref().unwrap().store.lock().unwrap();
            (
                store
                    .create_project(Uuid::new_v4(), "Global project")
                    .unwrap()
                    .id,
                store.create_rig(Uuid::new_v4(), "Rig").unwrap().id,
            )
        };
        let plan = json!({"catalog_id":catalog,"mappings":[{
            "catalog_id":catalog,"source_project_guid":guid,"source_profile_id":"profile-a",
            "project_id":project,"rig_id":rig
        }]});
        let app = router(state.clone());
        Self {
            _dir: dir,
            state,
            app,
            source,
            plan,
            catalog,
        }
    }

    async fn preview(&self) -> Value {
        let (status, result) = call(&self.app, "POST", PREVIEW, self.plan.clone(), None).await;
        assert_eq!(status, StatusCode::OK, "{result}");
        result["data"].clone()
    }

    fn request(&self, preview: &Value) -> Value {
        json!({"plan":self.plan,"preview_digest":preview["preview_digest"]})
    }
}

#[tokio::test]
async fn preview_commits_nothing_and_apply_is_idempotent_with_complete_review_context() {
    let fixture = Fixture::new();
    let preview = fixture.preview().await;
    assert_eq!(preview["applied"], false);
    assert_eq!(preview["mappings"][0]["source_name"], "M31");
    assert_eq!(preview["mappings"][0]["project"]["name"], "Global project");
    assert_eq!(preview["mappings"][0]["rig"]["name"], "Rig");
    assert_eq!(
        crate::catalog_identity::read(&fixture.source).unwrap(),
        None
    );
    assert_eq!(
        fixture
            .state
            .director
            .as_ref()
            .unwrap()
            .store
            .lock()
            .unwrap()
            .catalog_identity(fixture.catalog)
            .unwrap(),
        None
    );
    for _ in 0..2 {
        let (status, response) =
            call(&fixture.app, "POST", APPLY, fixture.request(&preview), None).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["data"]["applied"], true);
        assert_eq!(
            response["data"]["preview_digest"],
            preview["preview_digest"]
        );
    }
    let identity = crate::catalog_identity::read(&fixture.source)
        .unwrap()
        .unwrap();
    assert_eq!(identity.id, fixture.catalog);
    {
        let store = fixture
            .state
            .director
            .as_ref()
            .unwrap()
            .store
            .lock()
            .unwrap();
        assert_eq!(store.catalog_identity(identity.id).unwrap(), Some(identity));
        assert_eq!(
            store
                .catalog_project_mappings(identity.id, None, 256)
                .unwrap()
                .items
                .len(),
            1
        );
    }
    let (_, discovery) = call(
        &fixture.app,
        "GET",
        "/catalogs/catalog/discovery",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(
        discovery["data"]["catalog_identity"]["id"],
        fixture.catalog.to_string()
    );
    assert_eq!(
        fixture
            .source
            .query_row("SELECT gradingStatus FROM acquiredimage", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn changed_source_or_destination_names_require_a_new_preview_before_any_write() {
    for source_change in [true, false] {
        let fixture = Fixture::new();
        let preview = fixture.preview().await;
        if source_change {
            fixture
                .source
                .execute_batch("UPDATE project SET name='Changed source'")
                .unwrap();
        } else {
            let id =
                Uuid::parse_str(fixture.plan["mappings"][0]["rig_id"].as_str().unwrap()).unwrap();
            fixture
                .state
                .director
                .as_ref()
                .unwrap()
                .store
                .lock()
                .unwrap()
                .rename_rig(id, 1, "Changed rig")
                .unwrap();
        }
        assert_eq!(
            call(&fixture.app, "POST", APPLY, fixture.request(&preview), None)
                .await
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            crate::catalog_identity::read(&fixture.source).unwrap(),
            None
        );
        let fresh = fixture.preview().await;
        assert_ne!(fresh["preview_digest"], preview["preview_digest"]);
        assert_eq!(
            call(&fixture.app, "POST", APPLY, fixture.request(&fresh), None)
                .await
                .0,
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn profile_guid_and_catalog_scope_cannot_be_fabricated_or_repointed() {
    let fixture = Fixture::new();
    for key in [
        "source_project_guid",
        "source_profile_id",
        "project_id",
        "rig_id",
        "catalog_id",
    ] {
        let mut invalid = fixture.plan.clone();
        invalid["mappings"][0][key] = Uuid::new_v4().to_string().into();
        let (status, _) = call(&fixture.app, "POST", PREVIEW, invalid, None).await;
        assert!(
            matches!(
                status,
                StatusCode::CONFLICT | StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST
            ),
            "{key}: {status}"
        );
    }
    assert_eq!(
        crate::catalog_identity::read(&fixture.source).unwrap(),
        None
    );
    assert_eq!(
        fixture
            .state
            .director
            .as_ref()
            .unwrap()
            .store
            .lock()
            .unwrap()
            .catalog_identity(fixture.catalog)
            .unwrap(),
        None
    );
    fixture
        .source
        .execute(
            "INSERT INTO project SELECT 2,name,guid,profileId FROM project WHERE Id=1",
            [],
        )
        .unwrap();
    assert_eq!(
        call(&fixture.app, "POST", PREVIEW, fixture.plan.clone(), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn interrupted_coordinator_commit_recovers_using_the_same_durable_lineage() {
    let mut fixture = Fixture::new();
    let preview = fixture.preview().await;
    let identity: CatalogIdentity =
        serde_json::from_value(preview["catalog_identity"].clone()).unwrap();
    // Reproduce the durable boundary after catalog commit but before meta commit.
    let mut tx = fixture
        .source
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    crate::catalog_identity::adopt(&mut tx, identity).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        fixture
            .state
            .director
            .as_ref()
            .unwrap()
            .store
            .lock()
            .unwrap()
            .catalog_identity(identity.id)
            .unwrap(),
        None
    );
    let (status, response) =
        call(&fixture.app, "POST", APPLY, fixture.request(&preview), None).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        response["data"]["catalog_identity"],
        preview["catalog_identity"]
    );
}

#[tokio::test]
async fn gates_bounds_and_missing_preview_protect_the_adoption_routes() {
    let fixture = Fixture::new();
    assert_eq!(
        call(
            &fixture.app,
            "POST",
            APPLY,
            json!({"plan":fixture.plan}),
            None
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(
            &fixture.app,
            "POST",
            APPLY,
            json!({"plan":fixture.plan,"preview_digest":"0".repeat(64)}),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let mut oversized = fixture.plan.clone();
    oversized["padding"] = "x"
        .repeat(psf_guard_director_core::MAX_REQUEST_BYTES)
        .into();
    assert_eq!(
        call(&fixture.app, "POST", PREVIEW, oversized, None).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut empty = fixture.plan.clone();
    empty["mappings"] = json!([]);
    assert_eq!(
        call(&fixture.app, "POST", PREVIEW, empty, None).await.0,
        StatusCode::BAD_REQUEST
    );
    let permit = fixture
        .state
        .director
        .as_ref()
        .unwrap()
        .discovery_admission
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    assert_eq!(
        call(&fixture.app, "POST", PREVIEW, fixture.plan.clone(), None)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(permit);
    // A rejected second permit must not strand metadata admission.
    assert_eq!(
        call(&fixture.app, "GET", "/projects", Value::Null, None)
            .await
            .0,
        StatusCode::OK
    );
    fixture.state.set_allow_database_management(false);
    for endpoint in [PREVIEW, APPLY] {
        let body = if endpoint == PREVIEW {
            fixture.plan.clone()
        } else {
            json!({"plan":fixture.plan,"preview_digest":"0".repeat(64)})
        };
        assert_eq!(
            call(&fixture.app, "POST", endpoint, body, None).await.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        crate::catalog_identity::read(&fixture.source).unwrap(),
        None
    );
}

#[tokio::test]
async fn adoption_requires_an_operator_not_a_sync_key_or_read_only_account() {
    let fixture = Fixture::new();
    let mut registry = AuthRegistry::default();
    registry
        .add(
            AuthUserRecord::new("operator", AccessRole::ReadWrite, "test-password-not-real")
                .unwrap(),
            false,
        )
        .unwrap();
    let (reader, record) = AuthTokenRecord::mint("operator", "reader", true, None).unwrap();
    registry.tokens.push(record);
    fixture
        .state
        .set_server_auth(auth::ServerAuth::from_sources(None, &registry, 3000).unwrap());
    for endpoint in [PREVIEW, APPLY] {
        for token in [None, Some("database-sync-key")] {
            assert_eq!(
                call(&fixture.app, "POST", endpoint, json!({}), token)
                    .await
                    .0,
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            call(&fixture.app, "POST", endpoint, json!({}), Some(&reader))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        crate::catalog_identity::read(&fixture.source).unwrap(),
        None
    );
}

#[tokio::test]
async fn new_catalog_cannot_claim_an_already_registered_identity() {
    let fixture = Fixture::new();
    let service = fixture.state.director.as_ref().unwrap();
    service
        .store
        .lock()
        .unwrap()
        .register_catalog(CatalogIdentity {
            id: fixture.catalog,
            origin_instance_id: service.instance_id,
        })
        .unwrap();
    assert_eq!(
        call(&fixture.app, "POST", PREVIEW, fixture.plan.clone(), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        crate::catalog_identity::read(&fixture.source).unwrap(),
        None
    );
}

#[tokio::test]
async fn batch_review_accepts_more_than_four_kib_but_not_duplicate_source_projects() {
    let mut fixture = Fixture::new();
    let mapping = fixture.plan["mappings"][0].clone();
    let mappings = fixture.plan["mappings"].as_array_mut().unwrap();
    for index in 2..=32 {
        let guid = Uuid::new_v4();
        fixture
            .source
            .execute(
                "INSERT INTO project VALUES(?1,'Additional target',?2,'profile-a')",
                rusqlite::params![index, guid.to_string()],
            )
            .unwrap();
        let mut next = mapping.clone();
        next["source_project_guid"] = guid.to_string().into();
        mappings.push(next);
    }
    assert!(fixture.plan.to_string().len() > 4096);
    let preview = fixture.preview().await;
    assert_eq!(preview["mappings"].as_array().unwrap().len(), 32);
    assert_eq!(
        call(&fixture.app, "POST", APPLY, fixture.request(&preview), None)
            .await
            .0,
        StatusCode::OK
    );
    fixture.plan["mappings"]
        .as_array_mut()
        .unwrap()
        .push(mapping);
    assert_eq!(
        call(&fixture.app, "POST", PREVIEW, fixture.plan.clone(), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn mapping_inventory_is_paged_and_does_not_adopt_an_unregistered_catalog() {
    let mut fixture = Fixture::new();
    let path = "/catalogs/catalog/mappings";
    let (status, empty) = call(&fixture.app, "GET", path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty["data"]["catalog_identity"], Value::Null);
    assert_eq!(empty["data"]["items"], json!([]));
    assert_eq!(
        crate::catalog_identity::read(&fixture.source).unwrap(),
        None
    );
    let guid = Uuid::new_v4();
    fixture
        .source
        .execute(
            "INSERT INTO project VALUES(2,'Second',?1,'profile-a')",
            [guid.to_string()],
        )
        .unwrap();
    let mut mapping = fixture.plan["mappings"][0].clone();
    mapping["source_project_guid"] = guid.to_string().into();
    fixture.plan["mappings"]
        .as_array_mut()
        .unwrap()
        .push(mapping);
    let preview = fixture.preview().await;
    assert_eq!(
        call(&fixture.app, "POST", APPLY, fixture.request(&preview), None)
            .await
            .0,
        StatusCode::OK
    );
    let (status, first) = call(
        &fixture.app,
        "GET",
        &format!("{path}?limit=1"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        first["data"]["catalog_identity"]["id"],
        fixture.catalog.to_string()
    );
    let cursor = first["data"]["next_after"].as_str().unwrap();
    let (_, second) = call(
        &fixture.app,
        "GET",
        &format!("{path}?limit=1&after={cursor}"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(second["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["data"]["next_after"], Value::Null);
    assert_ne!(second["data"]["items"][0], first["data"]["items"][0]);
    assert_eq!(
        call(
            &fixture.app,
            "GET",
            &format!("{path}?limit=257"),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn mapping_inventory_allows_authenticated_readers_but_keeps_management_gate() {
    let fixture = Fixture::new();
    let mut registry = AuthRegistry::default();
    registry
        .add(
            AuthUserRecord::new("operator", AccessRole::ReadWrite, "test-password-not-real")
                .unwrap(),
            false,
        )
        .unwrap();
    let (reader, record) = AuthTokenRecord::mint("operator", "reader", true, None).unwrap();
    registry.tokens.push(record);
    fixture
        .state
        .set_server_auth(auth::ServerAuth::from_sources(None, &registry, 3000).unwrap());
    let path = "/catalogs/catalog/mappings";
    assert_eq!(
        call(&fixture.app, "GET", path, Value::Null, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &fixture.app,
            "GET",
            path,
            Value::Null,
            Some("database-sync-key")
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&fixture.app, "GET", path, Value::Null, Some(&reader))
            .await
            .0,
        StatusCode::OK
    );
    fixture.state.set_allow_database_management(false);
    assert_eq!(
        call(&fixture.app, "GET", path, Value::Null, Some(&reader))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        crate::catalog_identity::read(&fixture.source).unwrap(),
        None
    );
}

#[tokio::test]
async fn mapping_inventory_preserves_unregistered_lineage_and_refuses_origin_conflicts() {
    let mut fixture = Fixture::new();
    let identity = CatalogIdentity {
        id: fixture.catalog,
        origin_instance_id: Uuid::new_v4(),
    };
    let mut tx = fixture.source.transaction().unwrap();
    crate::catalog_identity::adopt(&mut tx, identity).unwrap();
    tx.commit().unwrap();
    let path = "/catalogs/catalog/mappings";
    let (status, result) = call(&fixture.app, "GET", path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["data"]["catalog_identity"], json!(identity));
    assert_eq!(result["data"]["items"], json!([]));
    {
        let mut store = fixture
            .state
            .director
            .as_ref()
            .unwrap()
            .store
            .lock()
            .unwrap();
        assert_eq!(store.catalog_identity(identity.id).unwrap(), None);
        store
            .register_catalog(CatalogIdentity {
                id: identity.id,
                origin_instance_id: Uuid::new_v4(),
            })
            .unwrap();
    }
    assert_eq!(
        call(&fixture.app, "GET", path, Value::Null, None).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        crate::catalog_identity::read(&fixture.source).unwrap(),
        Some(identity)
    );
}
