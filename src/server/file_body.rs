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
