//! Which frontend build the server hands out, and how long a browser may keep
//! each file. Every response a browser cannot keep names the build in
//! [`BUILD_HEADER`], so a tab left open across a server update can tell its
//! page is out of date. A response it may keep, such as a survey image, does
//! not: read back from the browser's cache after an update, it would name the
//! build of the day it was fetched.

use axum::{
    extract::{Request, State},
    http::HeaderValue,
    middleware::Next,
    response::Response,
};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

/// Response header naming the frontend build the server holds.
pub const BUILD_HEADER: &str = "x-psf-guard-build";

/// Vite writes content-hashed file names under `assets/`, so those never
/// change; every other file, `index.html` above all, must be checked with
/// the server on each load.
pub fn is_hashed_asset(path: &str) -> bool {
    path.starts_with("assets/")
}

/// The answer to anything but GET or HEAD outside `/api`. The app is only
/// ever fetched; a POST here is most often an MCP client given the bare
/// host, so say where the endpoint is instead of answering with the page.
pub fn method_not_allowed() -> axum::response::Response {
    use axum::{http::header, response::IntoResponse};
    (
        axum::http::StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "GET, HEAD")],
        "PSF Guard serves its app here. The API is under /api, and the MCP endpoint is /api/mcp.",
    )
        .into_response()
}

pub fn cache_control(path: &str) -> HeaderValue {
    HeaderValue::from_static(if is_hashed_asset(path) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    })
}

/// The build the Vite build stamps into index.html as
/// `<meta name="psf-guard-build" content="…">`.
pub fn build_in(html: &str) -> Option<&str> {
    html.split("<meta").skip(1).find_map(|tag| {
        let tag = &tag[..tag.find('>')?];
        if !tag.contains(r#"name="psf-guard-build""#) {
            return None;
        }
        let content = &tag[tag.find(r#"content=""#)? + r#"content=""#.len()..];
        Some(&content[..content.find('"')?]).filter(|build| !build.is_empty())
    })
}

/// Where the served index.html comes from.
pub enum PageBuild {
    Embedded,
    /// `--static-dir`: read again whenever the file changes, so a rebuilt
    /// frontend is reported without a server restart.
    Dir {
        index: PathBuf,
        seen: Mutex<Option<(SystemTime, Option<HeaderValue>)>>,
    },
}

static EMBEDDED: LazyLock<Option<HeaderValue>> = LazyLock::new(|| {
    crate::server::embedded_static::index_html()
        .and_then(build_in)
        .and_then(|build| HeaderValue::from_str(build).ok())
});

impl PageBuild {
    pub fn in_dir(root: PathBuf) -> Self {
        Self::Dir {
            index: root.join("index.html"),
            seen: Mutex::new(None),
        }
    }

    async fn current(&self) -> Option<HeaderValue> {
        let (index, seen) = match self {
            Self::Embedded => return EMBEDDED.clone(),
            Self::Dir { index, seen } => (index, seen),
        };
        let modified = tokio::fs::metadata(index).await.ok()?.modified().ok()?;
        if let Some((at, build)) = &*seen.lock().unwrap()
            && *at == modified
        {
            return build.clone();
        }
        let html = tokio::fs::read_to_string(index).await.ok()?;
        let build = build_in(&html).and_then(|build| HeaderValue::from_str(build).ok());
        *seen.lock().unwrap() = Some((modified, build.clone()));
        build
    }
}

/// Whether a browser may keep a response with this `Cache-Control` and hand
/// it back later without asking the server.
pub fn kept_by_browser(cache_control: Option<&HeaderValue>) -> bool {
    let Some(value) = cache_control.and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let value = value.to_ascii_lowercase();
    if value.contains("no-store") || value.contains("no-cache") {
        return false;
    }
    value.contains("immutable")
        || value.split(',').any(|part| {
            part.trim()
                .strip_prefix("max-age=")
                .and_then(|seconds| seconds.trim().parse::<u64>().ok())
                .is_some_and(|seconds| seconds > 0)
        })
}

/// Middleware: name the served build on every response a browser cannot
/// keep.
pub async fn stamp(State(page): State<Arc<PageBuild>>, request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    if kept_by_browser(response.headers().get(axum::http::header::CACHE_CONTROL)) {
        return response;
    }
    if let Some(build) = page.current().await {
        response.headers_mut().insert(BUILD_HEADER, build);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_stamped_build() {
        let html = r#"<head><meta charset="UTF-8" /><meta name="viewport" content="width=device-width">
            <meta name="psf-guard-build" content="3f9a12c0d4e5b6a7"></head>"#;
        assert_eq!(build_in(html), Some("3f9a12c0d4e5b6a7"));
        assert_eq!(
            build_in(r#"<meta content="abc" name="psf-guard-build">"#),
            Some("abc")
        );
        assert_eq!(build_in(r#"<meta name="viewport" content="x">"#), None);
        assert_eq!(
            build_in(r#"<meta name="psf-guard-build" content="">"#),
            None
        );
    }

    #[test]
    fn only_hashed_assets_are_kept() {
        assert_eq!(
            cache_control("assets/index-C1YCPQT9.js"),
            "public, max-age=31536000, immutable"
        );
        for path in ["index.html", "psf-guard.svg", "stray.js", "x/assets/y.js"] {
            assert_eq!(cache_control(path), "no-cache", "{path}");
        }
    }

    #[test]
    fn the_embedded_page_is_stamped() {
        // CI builds the frontend before the server, so the embedded page
        // carries a build; a page without one would never ask for a reload.
        assert!(
            EMBEDDED.is_some(),
            "static/dist/index.html has no build stamp"
        );
    }

    #[tokio::test]
    async fn every_response_names_the_build() {
        use axum::{body::Body, http::StatusCode, Router};
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            r#"<meta name="psf-guard-build" content="abc123">"#,
        )
        .unwrap();
        let page = Arc::new(PageBuild::in_dir(dir.path().to_path_buf()));
        let app = Router::new()
            .fallback(|| async { StatusCode::UNAUTHORIZED })
            .layer(axum::middleware::from_fn_with_state(page, stamp));
        let response = app
            .oneshot(Request::get("/api/info").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[BUILD_HEADER], "abc123");
    }

    #[test]
    fn tells_what_a_browser_may_keep() {
        let kept = |value: &str| kept_by_browser(Some(&HeaderValue::from_str(value).unwrap()));
        assert!(kept("private, max-age=86400"));
        assert!(kept("public, max-age=31536000, immutable"));
        assert!(!kept("private, no-store"));
        assert!(!kept("no-cache"));
        assert!(!kept("max-age=0"));
        assert!(!kept("max-age=60, no-cache"));
        assert!(!kept_by_browser(None));
    }

    /// A survey image is kept for a day. Read back from the browser's cache
    /// after an update, a stamped one would name the old build, and the page
    /// would say PSF Guard was updated after every reload.
    #[tokio::test]
    async fn a_response_the_browser_keeps_names_no_build() {
        use axum::{body::Body, http::header::CACHE_CONTROL, routing::get, Router};
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            r#"<meta name="psf-guard-build" content="abc123">"#,
        )
        .unwrap();
        let page = Arc::new(PageBuild::in_dir(dir.path().to_path_buf()));
        let app = Router::new()
            .route(
                "/api/cutout",
                get(|| async { ([(CACHE_CONTROL, "private, max-age=86400")], "jpeg") }),
            )
            .route(
                "/api/plans",
                get(|| async { ([(CACHE_CONTROL, "private, no-store")], "{}") }),
            )
            .layer(axum::middleware::from_fn_with_state(page, stamp));
        let kept = app
            .clone()
            .oneshot(Request::get("/api/cutout").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(kept.headers().get(BUILD_HEADER).is_none());
        let fresh = app
            .oneshot(Request::get("/api/plans").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(fresh.headers()[BUILD_HEADER], "abc123");
    }

    #[tokio::test]
    async fn a_rebuilt_page_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("index.html");
        let page = PageBuild::in_dir(dir.path().to_path_buf());
        assert_eq!(page.current().await, None);

        std::fs::write(&index, r#"<meta name="psf-guard-build" content="one">"#).unwrap();
        assert_eq!(page.current().await.unwrap(), "one");

        std::fs::write(&index, r#"<meta name="psf-guard-build" content="two">"#).unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&index)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(page.current().await.unwrap(), "two");
    }
}
