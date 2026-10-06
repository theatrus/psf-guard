use super::*;
use psf_guard_director_interop::{astrocollab::Source, collaboration::prepare_import};

#[tokio::test]
async fn imported_collaboration_drafts_cannot_bypass_admission_through_activation() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state(&dir, true));
    let project = state
        .director
        .as_ref()
        .unwrap()
        .clone()
        .run(|store| {
            let rig = store.create_rig(Uuid::new_v4(), "Collaboration rig")?;
            let source = Source::new("https://collab.example", "000000000001", false).unwrap();
            let plan = prepare_import(
                include_bytes!(
                    "../../../../crates/director-interop/tests/fixtures/starfront-tonight.json"
                ),
                &source,
                "2026-10-05",
                "000000000004",
            )
            .unwrap();
            let preview = store.preview_collaboration_import(&plan, rig.id)?;
            store.apply_collaboration_import(&plan, rig.id, &preview.review_digest, 1000)?;
            Ok(plan.project_id())
        })
        .await
        .unwrap();
    let app = router(state);
    for (action, body) in [
        ("preview", json!({})),
        ("apply", json!({"preview_digest":"0".repeat(64)})),
    ] {
        let (status, reply) = call(
            &app,
            "POST",
            &format!("/projects/{project}/activation/{action}"),
            body,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(reply.to_string().contains("inactive draft"));
    }
}
