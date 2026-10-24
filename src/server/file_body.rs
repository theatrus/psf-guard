//! Response bodies that stream a file from disk.

use axum::body::Body;
use tokio_util::io::ReaderStream;

/// Bytes per read. `ReaderStream` defaults to 4 KiB, and every read of a
/// `tokio::fs::File` is a round trip to the blocking pool: a 244 MB stack FITS
/// took some 60,000 of them, and its download outlasted a client's 5 s timeout
/// on CI (2026-10-23). At 256 KiB it takes about 1,000.
pub const CHUNK: usize = 256 * 1024;

/// Streams `file` as the body, `CHUNK` bytes at a time.
pub fn file_body(file: tokio::fs::File) -> Body {
    Body::from_stream(ReaderStream::with_capacity(file, CHUNK))
}

/// Keep a file's cleanup protection until the response ends or is dropped.
pub fn file_body_with_guard<G: Send + Unpin + 'static>(file: tokio::fs::File, guard: G) -> Body {
    struct Guarded<G> {
        file: tokio::fs::File,
        _guard: G,
    }
    impl<G: Unpin> tokio::io::AsyncRead for Guarded<G> {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.get_mut().file).poll_read(cx, buf)
        }
    }
    Body::from_stream(ReaderStream::with_capacity(
        Guarded {
            file,
            _guard: guard,
        },
        CHUNK,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    #[tokio::test]
    async fn cleanup_guard_lives_until_the_download_is_consumed_or_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("master.xisf");
        std::fs::write(&path, vec![1; CHUNK * 2 + 3]).unwrap();
        for consume in [false, true] {
            let released = Arc::new(AtomicBool::new(false));
            let file = tokio::fs::File::open(&path).await.unwrap();
            let body = file_body_with_guard(file, Guard(released.clone()));
            assert!(!released.load(Ordering::SeqCst));
            if consume {
                assert_eq!(
                    body.collect().await.unwrap().to_bytes().len(),
                    CHUNK * 2 + 3
                );
            } else {
                drop(body);
            }
            assert!(released.load(Ordering::SeqCst));
        }
    }
}
