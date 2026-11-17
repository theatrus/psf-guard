//! Downloading a large file a person asked for and checking it: the one
//! place PSF Guard streams an archive to disk and compares it with the
//! size and SHA-256 it has on record. ONNX Runtime and the offline sky maps
//! both download through here.

use std::path::Path;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// What the file must be, when PSF Guard has a record of it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Expected<'a> {
    pub bytes: Option<u64>,
    pub sha256: Option<&'a str>,
}

/// Why a download did not end with the expected file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The server could not be reached, refused, or the transfer broke off.
    Transfer(String),
    /// The file could not be written.
    Write(String),
    /// More than `limit` bytes were offered.
    TooLarge {
        limit: u64,
    },
    /// The file is not the one on record.
    NotOnRecord {
        bytes: u64,
        sha256: String,
    },
    Stopped,
}

/// Stream `url` to `to`, at most `limit` bytes, and check it against
/// `expected`. `progress` is told the bytes received and the total when it
/// is known. `stopped` is read between chunks. Writing goes through tokio's
/// file, so the transfer never blocks a runtime thread.
pub async fn fetch(
    url: &str,
    to: &Path,
    expected: Expected<'_>,
    limit: u64,
    stopped: &(dyn Fn() -> bool + Send + Sync),
    progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
) -> Result<(), Failure> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("psf-guard/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| Failure::Transfer(error.to_string()))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| Failure::Transfer(error.to_string()))?;
    let offered = response.content_length();
    if let (Some(bytes), Some(offered)) = (expected.bytes, offered)
        && bytes != offered
    {
        return Err(Failure::NotOnRecord {
            bytes: offered,
            sha256: String::new(),
        });
    }
    let total = expected.bytes.or(offered);
    if total.is_some_and(|total| total > limit) {
        return Err(Failure::TooLarge { limit });
    }
    let written = |error: std::io::Error| Failure::Write(format!("{}: {error}", to.display()));
    let mut file = tokio::fs::File::create(to).await.map_err(written)?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    progress(0, total);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| Failure::Transfer(error.to_string()))?
    {
        if stopped() {
            return Err(Failure::Stopped);
        }
        received += chunk.len() as u64;
        if received > limit || expected.bytes.is_some_and(|bytes| received > bytes) {
            return Err(match expected.bytes {
                Some(_) => Failure::NotOnRecord {
                    bytes: received,
                    sha256: String::new(),
                },
                None => Failure::TooLarge { limit },
            });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(written)?;
        progress(received, total);
    }
    file.sync_all().await.map_err(written)?;
    drop(file);
    let sha256 = crate::remote_work::hex(&hasher.finalize());
    let size_differs = expected.bytes.is_some_and(|bytes| bytes != received);
    let hash_differs = expected.sha256.is_some_and(|wanted| wanted != sha256);
    if size_differs || hash_differs {
        return Err(Failure::NotOnRecord {
            bytes: received,
            sha256,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn serve(body: Vec<u8>) -> String {
        use axum::routing::get;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/file",
            get(move || {
                let body = body.clone();
                async move { body }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}/file")
    }

    #[tokio::test]
    async fn a_file_passes_only_when_it_is_the_one_on_record() {
        let body = b"the archive".to_vec();
        let hash = crate::remote_work::hex(&Sha256::digest(&body));
        let url = serve(body.clone()).await;
        let folder = tempfile::tempdir().unwrap();
        let to = folder.path().join("file");
        let never = || false;
        let quiet = |_: u64, _: Option<u64>| {};
        let get = |expected: Expected<'static>| {
            let (url, to) = (url.clone(), to.clone());
            async move { fetch(&url, &to, expected, 1 << 20, &never, &quiet).await }
        };
        let hash: &'static str = Box::leak(hash.into_boxed_str());
        let size = body.len() as u64;
        get(Expected {
            bytes: Some(size),
            sha256: Some(hash),
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), body);
        get(Expected::default()).await.unwrap();
        for expected in [
            Expected {
                bytes: Some(size),
                sha256: Some("00"),
            },
            Expected {
                bytes: Some(size + 1),
                sha256: Some(hash),
            },
            Expected {
                bytes: Some(size - 1),
                sha256: Some(hash),
            },
            Expected {
                bytes: None,
                sha256: Some("00"),
            },
        ] {
            let refused = get(expected).await.unwrap_err();
            assert!(
                matches!(refused, Failure::NotOnRecord { .. }),
                "{refused:?}"
            );
        }
        let small = fetch(&url, &to, Expected::default(), 4, &never, &quiet).await;
        assert_eq!(small, Err(Failure::TooLarge { limit: 4 }));
    }

    #[tokio::test]
    async fn a_stopped_download_ends_at_the_next_chunk() {
        let url = serve(vec![7u8; 1 << 16]).await;
        let folder = tempfile::tempdir().unwrap();
        let ended = fetch(
            &url,
            &folder.path().join("file"),
            Expected::default(),
            1 << 20,
            &|| true,
            &|_, _| {},
        )
        .await;
        assert_eq!(ended, Err(Failure::Stopped));
    }
}
