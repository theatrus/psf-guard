use super::{
    annotated_cache_key, image_file_identity_cache_token, preview_cache_key, psf_multi_cache_key,
    serve_cached_png, star_detection_cache_key,
};
use crate::models::AcquiredImage;

fn image() -> AcquiredImage {
    AcquiredImage {
        id: 42,
        project_id: 7,
        target_id: 3,
        acquired_date: Some(1_750_000_000),
        filter_name: "OSC".into(),
        grading_status: 0,
        metadata: "{}".into(),
        reject_reason: None,
        profile_id: None,
        guid: None,
    }
}

#[test]
fn colour_and_greyscale_are_different_artifacts() {
    // They are rendered differently, so serving one for the other shows
    // the wrong pixels rather than a stale copy of the right ones.
    let mono = preview_cache_key(&image(), "one.fits", "screen", true, 0.2, -2.8, false, None);
    let color = preview_cache_key(&image(), "one.fits", "screen", true, 0.2, -2.8, true, None);
    assert_ne!(mono, color);
    assert!(color.ends_with("_color"), "{color}");
}

#[test]
fn greyscale_keys_are_unchanged_by_the_colour_option() {
    // Previews already on disk were written before colour existed. The
    // marker only appears on the colour variant so those stay valid.
    assert!(
        !preview_cache_key(&image(), "one.fits", "screen", true, 0.2, -2.8, false, None,)
            .contains("color"),
    );
}

#[test]
fn pre_generation_warms_the_key_the_request_path_looks_up() {
    // Pre-generation used to build this key with its own `format!`, which
    // drifted the moment colour arrived: it wrote a colour PNG under the
    // greyscale key, so the viewer never found what had been warmed and
    // anything asking for greyscale was served colour.
    //
    // Both renditions are checked, because the colour default is now a
    // server setting: whichever one this server serves, that is the one
    // pre-generation must warm.
    for color in [true, false] {
        let warmed = preview_cache_key(
            &image(),
            "one.fits",
            "screen",
            true,
            crate::server::preview_pregeneration::MIDTONE,
            crate::server::preview_pregeneration::SHADOW,
            color,
            None,
        );
        // What `get_image_preview` computes for a caller taking every
        // default on a server configured that way.
        let requested =
            preview_cache_key(&image(), "one.fits", "screen", true, 0.2, -2.8, color, None);
        assert_eq!(warmed, requested);
    }
}

#[test]
fn remote_mapping_changes_move_inflight_jobs_to_a_new_artifact() {
    let first_preview = preview_cache_key(
        &image(),
        "one.fits",
        "screen",
        true,
        0.2,
        -2.8,
        false,
        Some("mapping-a"),
    );
    let remapped_preview = preview_cache_key(
        &image(),
        "one.fits",
        "screen",
        true,
        0.2,
        -2.8,
        false,
        Some("mapping-b"),
    );
    let first_annotated =
        annotated_cache_key(&image(), "one.fits", "screen", 1000, Some("mapping-a"));
    let remapped_annotated =
        annotated_cache_key(&image(), "one.fits", "screen", 1000, Some("mapping-b"));

    assert_ne!(first_preview, remapped_preview);
    assert_ne!(first_annotated, remapped_annotated);
    assert!(first_preview.contains("_mapping_mapping-a"));
    assert!(remapped_preview.contains("_mapping_mapping-b"));
}

#[tokio::test]
async fn rendered_artifacts_require_browser_revalidation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preview.png");
    tokio::fs::write(&path, b"cached pixels").await.unwrap();

    let response = serve_cached_png(&path, &axum::http::HeaderMap::new())
        .await
        .unwrap();

    assert_eq!(
        response.headers()[axum::http::header::CACHE_CONTROL],
        "no-cache"
    );
    let etag = response.headers()[axum::http::header::ETAG].clone();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(axum::http::header::IF_NONE_MATCH, etag);
    let response = serve_cached_png(&path, &headers).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::NOT_MODIFIED);
}

#[test]
fn scheduler_file_identity_changes_invalidate_derived_artifact_keys() {
    let mut first = image();
    first.metadata =
        r#"{"FileName":"D:\\old\\one.fits","ExposureStartTime":"2025-06-15T00:00:00Z"}"#.into();
    let mut moved = first.clone();
    moved.metadata =
        r#"{"FileName":"E:\\new\\one.fits","ExposureStartTime":"2025-06-15T00:00:00Z"}"#.into();

    assert_ne!(
        image_file_identity_cache_token(&first),
        image_file_identity_cache_token(&moved)
    );
    assert_ne!(
        preview_cache_key(&first, "one.fits", "screen", true, 0.2, -2.8, false, None,),
        preview_cache_key(&moved, "one.fits", "screen", true, 0.2, -2.8, false, None,)
    );
    assert_ne!(
        annotated_cache_key(&first, "one.fits", "screen", 1000, None),
        annotated_cache_key(&moved, "one.fits", "screen", 1000, None)
    );
    assert_ne!(
        star_detection_cache_key(&first, "one.fits", "source-a"),
        star_detection_cache_key(&moved, "one.fits", "source-a")
    );
    assert_ne!(
        psf_multi_cache_key(&first, "one.fits", 9, "moffat", "r2", "top-n", None, "source-a",),
        psf_multi_cache_key(&moved, "one.fits", 9, "moffat", "r2", "top-n", None, "source-a",)
    );
}
