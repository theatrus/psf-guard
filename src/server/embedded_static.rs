use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, Response, StatusCode, Uri},
    response::IntoResponse,
};
use include_dir::{include_dir, Dir};
use mime_guess::from_path;

// Embed the static files at compile time
static STATIC_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/static/dist");

pub async fn serve_embedded_file(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // If path is empty, serve index.html
    let path = if path.is_empty() { "index.html" } else { path };

    // Try to get the file from embedded assets
    if let Some(file) = STATIC_DIR.get_file(path) {
        let mime_type = from_path(path).first_or_octet_stream();
        let mut headers = HeaderMap::new();

        // Log the detected MIME type for debugging
        tracing::trace!("Serving embedded file '{}' with MIME type: {}", path, mime_type);

        // Set content type
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime_type.as_ref())
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
        );

        // Add caching headers for static assets
        if path.contains("/assets/")
            || path.ends_with(".js")
            || path.ends_with(".css")
            || path.ends_with(".wasm")
        {
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
        } else {
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            );
        }

        let body = Body::from(file.contents());
        let mut response_builder = Response::builder().status(StatusCode::OK);
        
        // Add each header to the response builder
        for (key, value) in headers.iter() {
            response_builder = response_builder.header(key, value);
        }
        
        response_builder
            .body(body)
            .unwrap()
            .into_response()
    } else {
        // For SPA, fall back to index.html for non-API routes
        if !path.starts_with("api/") {
            if let Some(index_file) = STATIC_DIR.get_file("index.html") {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                );
                headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));

                let body = Body::from(index_file.contents());
                return Response::builder()
                    .status(StatusCode::OK)
                    .body(body)
                    .unwrap()
                    .into_response();
            }
        }

        // Return 404 for missing files
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("File not found"))
            .unwrap()
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_dir_exists() {
        // This will fail at compile time if the static directory doesn't exist
        assert!(
            STATIC_DIR.get_file("index.html").is_some(),
            "index.html should exist in embedded static files"
        );
    }
    
    #[test]
    fn test_mime_type_detection() {
        // Test MIME type detection for common file types
        assert_eq!(from_path("test.js").first_or_octet_stream().to_string(), "text/javascript");
        assert_eq!(from_path("test.css").first_or_octet_stream().to_string(), "text/css");
        assert_eq!(from_path("test.html").first_or_octet_stream().to_string(), "text/html");
        assert_eq!(from_path("assets/index-Be-oaiRe.css").first_or_octet_stream().to_string(), "text/css");
        assert_eq!(from_path("assets/index-DvXiWCNI.js").first_or_octet_stream().to_string(), "text/javascript");
    }
}
