//! Small rectangles of an image file's samples, for the image viewers'
//! pointer readout. Only the rectangle's rows are read from the file, so a
//! readout over a stack of several gigabytes reads a few kilobytes a tile
//! and holds nothing once it answers.
//!
//! Each route that downloads a linear FITS has a `…/window` route beside it
//! that resolves the same file; frames have `/images/{id}/window`.

use std::path::PathBuf;

use axum::Json;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use super::api::ApiResponse;
use super::handlers::AppError;

/// The largest edge of a window. The readout asks for 64-pixel tiles.
pub(crate) const MAX_WINDOW_EDGE: usize = 256;

/// The rectangle a window route reads, in the file's pixels counted from 0
/// at its first stored row. The route's other parameters, such as `v` or
/// `copy`, are read beside it.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct WindowQuery {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// The samples of the part of a window that lies on the image.
#[derive(Debug, Serialize)]
pub struct PixelWindowResponse {
    pub image_width: usize,
    pub image_height: usize,
    pub planes: usize,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    /// Little-endian f32 samples, base64: plane by plane, then row by row.
    /// Not-a-number marks a blank sample.
    pub values: String,
}

impl From<seiza_fits::ImageWindow> for PixelWindowResponse {
    fn from(window: seiza_fits::ImageWindow) -> Self {
        let bytes: Vec<u8> = window
            .values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        Self {
            image_width: window.image_width,
            image_height: window.image_height,
            planes: window.planes,
            x: window.x,
            y: window.y,
            width: window.width,
            height: window.height,
            values: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }
}

/// Read a window of the image file at `path`, off the async runtime.
pub(crate) async fn read(
    path: PathBuf,
    query: WindowQuery,
) -> Result<Json<ApiResponse<PixelWindowResponse>>, AppError> {
    let edge = 1..=MAX_WINDOW_EDGE;
    if !edge.contains(&query.width) || !edge.contains(&query.height) {
        return Err(AppError::BadRequest(format!(
            "A pixel window is 1 to {MAX_WINDOW_EDGE} pixels on each side"
        )));
    }
    let window = tokio::task::spawn_blocking(move || {
        if !path.is_file() {
            return Err(AppError::NotFound);
        }
        crate::image_io::read_window(&path, query.x, query.y, query.width, query.height).map_err(
            |error| {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                AppError::InternalError(format!(
                    "Pixel values could not be read from {name}: {error}"
                ))
            },
        )
    })
    .await
    .map_err(|_| AppError::InternalError("Reading pixel values stopped".into()))??;
    Ok(Json(ApiResponse::success(window.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(response: &PixelWindowResponse) -> Vec<f32> {
        base64::engine::general_purpose::STANDARD
            .decode(&response.values)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect()
    }

    fn stack(directory: &std::path::Path) -> PathBuf {
        // A 300x2 stack whose sample at (x, y) is y·1000 + x.
        let pixels: Vec<f32> = (0..2)
            .flat_map(|y| (0..300).map(move |x| (y * 1000 + x) as f32))
            .collect();
        let path = directory.join("stack.fits");
        seiza_fits::write_f32_image(&path, 300, 2, seiza_fits::F32ImageData::Mono(&pixels), &[])
            .unwrap();
        path
    }

    #[tokio::test]
    async fn a_window_sends_the_samples_of_its_part_of_the_image() {
        let directory = tempfile::tempdir().unwrap();
        let query = WindowQuery {
            x: 256,
            y: 0,
            width: 64,
            height: 64,
        };
        let Json(response) = read(stack(directory.path()), query).await.unwrap();
        let window = response.data.unwrap();
        assert_eq!(
            (window.image_width, window.image_height, window.planes),
            (300, 2, 1)
        );
        // Clipped to the image: 44 columns of 2 rows.
        assert_eq!(
            (window.x, window.y, window.width, window.height),
            (256, 0, 44, 2)
        );
        let values = decoded(&window);
        assert_eq!(values.len(), 88);
        assert_eq!((values[0], values[43], values[44]), (256.0, 299.0, 1256.0));
    }

    /// The route beside a stack's FITS download reads that FITS.
    #[tokio::test]
    async fn the_window_route_reads_the_stack_its_download_serves() {
        use axum::body::Body;
        use axum::http::Request;
        use std::sync::Arc;
        use tower::ServiceExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rig.sqlite");
        let images = dir.path().join("images");
        std::fs::create_dir_all(&images).unwrap();
        crate::ts_schema::apply_schema(&rusqlite::Connection::open(&path).unwrap()).unwrap();
        let catalog = crate::server::database_context::DatabaseContext::new(
            "rig".into(),
            "Rig".into(),
            path.display().to_string(),
            vec![images.display().to_string()],
            None,
            None,
            None,
            dir.path().join("cache"),
        )
        .unwrap();
        let job_id = "a".repeat(64);
        let job = crate::server::storage::stacks(&catalog.stack_root).join(&job_id);
        std::fs::create_dir_all(&job).unwrap();
        std::fs::rename(stack(dir.path()), job.join("group-0.fits")).unwrap();
        let state = crate::server::state::AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        );
        {
            let mut databases = state.databases.write().unwrap();
            databases.clear();
            databases.insert(catalog.id.clone(), Arc::new(catalog));
        }
        let app = axum::Router::new().nest("/api", crate::server::api_router(Arc::new(state)));
        let get = |uri: String| {
            Request::builder()
                .uri(uri)
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap()
        };

        let response = app
            .clone()
            .oneshot(get(format!(
                "/api/db/rig/stack-previews/{job_id}/0/fits/window?v=rev&x=298&y=1&width=4&height=4"
            )))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let window = &reply["data"];
        assert_eq!(
            (
                &window["x"],
                &window["y"],
                &window["width"],
                &window["height"]
            ),
            (&298.into(), &1.into(), &2.into(), &1.into())
        );
        let values: Vec<f32> = base64::engine::general_purpose::STANDARD
            .decode(window["values"].as_str().unwrap())
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect();
        assert_eq!(values, [1298.0, 1299.0]);

        // Another group of the job has no file, and a malformed job is refused.
        let missing = app
            .clone()
            .oneshot(get(format!(
                "/api/db/rig/stack-previews/{job_id}/1/fits/window?x=0&y=0&width=1&height=1"
            )))
            .await
            .unwrap();
        assert_eq!(missing.status(), 404);
        let malformed = app
            .oneshot(get(
                "/api/db/rig/stack-previews/nope/0/fits/window?x=0&y=0&width=1&height=1".into(),
            ))
            .await
            .unwrap();
        assert_eq!(malformed.status(), 400);
    }

    #[tokio::test]
    async fn a_window_is_small_and_of_a_file_that_exists() {
        let directory = tempfile::tempdir().unwrap();
        let path = stack(directory.path());
        let too_wide = WindowQuery {
            x: 0,
            y: 0,
            width: MAX_WINDOW_EDGE + 1,
            height: 1,
        };
        assert!(matches!(
            read(path.clone(), too_wide).await,
            Err(AppError::BadRequest(_))
        ));
        let empty = WindowQuery {
            width: 0,
            ..too_wide
        };
        assert!(matches!(
            read(path, empty).await,
            Err(AppError::BadRequest(_))
        ));
        let one = WindowQuery {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        assert!(matches!(
            read(directory.path().join("missing.fits"), one).await,
            Err(AppError::NotFound)
        ));
    }
}
