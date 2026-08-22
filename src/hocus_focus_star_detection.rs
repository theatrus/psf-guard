//! Moved to `seiza-stars`; re-exported so call sites keep their paths.
//!
//! One host-side addition: preset selection from a frame on disk, which needs
//! the shared header reader and therefore could not move with the rest.
pub use seiza_stars::hocus_focus_star_detection::*;

/// [`HocusFocusParams::for_frame_headers`] for a frame on disk: reads
/// FOCALLEN and XPIXSZ through the shared header reader. A missing or
/// unreadable header falls back to the standard defaults, never an error —
/// preset selection must not make a frame undetectable.
pub fn params_for_frame_path(path: &std::path::Path) -> (HocusFocusParams, TelescopeClass) {
    let (focal, pixel) = crate::image_io::read_header(path)
        .map(|headers| {
            let value = |name: &str| {
                headers
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .and_then(|(_, v)| v.as_f64())
            };
            (value("FOCALLEN"), value("XPIXSZ"))
        })
        .unwrap_or((None, None));
    HocusFocusParams::for_frame_headers(focal, pixel)
}
