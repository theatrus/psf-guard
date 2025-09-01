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
    type Future = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

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
            match serve_file(&file_path).await {
                Ok(response) => Ok(response),
                Err(_) => {
                    // For SPA, fall back to index.html for non-API routes
                    if !path.starts_with("api/") && index_file.exists() {
                        match serve_file(&index_file).await {
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

async fn serve_file(file_path: &PathBuf) -> Result<Response<Body>, std::io::Error> {
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
    
    // Add caching headers for static assets
    if let Some(path_str) = file_path.to_str() {
        if path_str.contains("/assets/") 
            || path_str.ends_with(".js") 
            || path_str.ends_with(".css") 
            || path_str.ends_with(".wasm") {
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
    }
    
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(contents))
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "Failed to build response"))?;
        
    *response.headers_mut() = headers;
    
    Ok(response)
}

fn not_found_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("File not found"))
        .unwrap()
}