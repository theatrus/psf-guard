use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderMap, HeaderValue, Response, StatusCode},
};
use mime_guess::from_path;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tower::Service;

use crate::server::page_build;

#[derive(Clone)]
pub struct StaticFileService {
    root: PathBuf,
    index_file: PathBuf,
}

impl StaticFileService {
    pub fn new(root: PathBuf) -> Self {
        let index_file = root.join("index.html");
        Self { root, index_file }
    }
}

impl<B: Send + 'static> Service<Request<B>> for StaticFileService {
    type Response = Response<Body>;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let root = self.root.clone();
        let index_file = self.index_file.clone();

        Box::pin(async move {
            let path = req.uri().path().trim_start_matches('/');

            // If path is empty, serve index.html
            let path = if path.is_empty() { "index.html" } else { path };

            let file_path = root.join(path);

            // Security check: ensure the path is within our root directory
            if !file_path.starts_with(&root) {
                return Ok(not_found_response());
            }

            // Try to serve the requested file
            match serve_file(&file_path, path).await {
                Ok(response) => Ok(response),
                Err(_) => {
                    // For SPA, fall back to index.html for non-API routes.
                    // A missing asset gets a 404, as in `embedded_static`.
                    if !path.starts_with("api/")
                        && !page_build::is_hashed_asset(path)
                        && index_file.exists()
                    {
                        match serve_file(&index_file, "index.html").await {
                            Ok(mut response) => {
                                // Override content-type for index.html fallback
                                let headers = response.headers_mut();
                                headers.insert(
                                    header::CONTENT_TYPE,
                                    HeaderValue::from_static("text/html; charset=utf-8"),
                                );
                                headers.insert(
                                    header::CACHE_CONTROL,
                                    HeaderValue::from_static("no-cache"),
                                );
                                Ok(response)
                            }
                            Err(_) => Ok(not_found_response()),
                        }
                    } else {
                        Ok(not_found_response())
                    }
                }
            }
        })
    }
}

/// `path` is the request path below the static root.
async fn serve_file(file_path: &PathBuf, path: &str) -> Result<Response<Body>, std::io::Error> {
    let mut file = File::open(file_path).await?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents).await?;

    // Determine MIME type from file extension
    let mime_type = from_path(file_path).first_or_octet_stream();
    let mut headers = HeaderMap::new();

    // Set content type with explicit MIME type
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime_type.as_ref())
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );

    headers.insert(header::CACHE_CONTROL, page_build::cache_control(path));

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(contents))
        .map_err(|_| std::io::Error::other("Failed to build response"))?;

    *response.headers_mut() = headers;

    Ok(response)
}

fn not_found_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("File not found"))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    #[tokio::test]
    async fn the_page_is_checked_each_load_and_assets_kept() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<!doctype html>").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/index-AbC123.js"), "export {}").unwrap();
        let service = StaticFileService::new(dir.path().to_path_buf());
        let get = |path: &'static str| {
            let request = Request::builder().uri(path).body(Body::empty()).unwrap();
            service.clone().oneshot(request)
        };

        for path in ["/", "/index.html", "/some/old/route"] {
            let page = get(path).await.unwrap();
            assert_eq!(page.status(), StatusCode::OK, "{path}");
            assert_eq!(page.headers()[header::CACHE_CONTROL], "no-cache", "{path}");
        }
        let kept = get("/assets/index-AbC123.js").await.unwrap();
        assert_eq!(
            kept.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        let gone = get("/assets/index-0ldBu1ld.js").await.unwrap();
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    }
}
