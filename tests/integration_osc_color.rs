//! One-shot-color rendering: the display path keeps the colour that the
//! measurement path deliberately throws away.
//!
//! `FitsImage::from_file` debayers and collapses to luminance, because star
//! metrics on a bare colour filter array are distorted by the per-channel
//! sampling. That is right for grading and wrong for looking, so the viewer
//! has its own path — and these check the two have not been confused.

use std::io::Write;

/// Build a small OSC frame: an RGGB mosaic with a strong red bias, so a
/// correct debayer is obvious in the output and a luminance collapse is too.
/// Fixed star field: enough for the registration minimum, spread out so the
/// matcher is not asked to disambiguate a cluster.
const STAR_POSITIONS: [(f64, f64); 10] = [
    (18.0, 22.0),
    (40.0, 15.0),
    (72.0, 30.0),
    (100.0, 24.0),
    (25.0, 58.0),
    (60.0, 66.0),
    (95.0, 74.0),
    (30.0, 96.0),
    (68.0, 104.0),
    (105.0, 110.0),
];

fn write_fits(path: &std::path::Path, bayer: Option<&str>) {
    const CARD: usize = 80;
    const BLOCK: usize = 2880;
    let (w, h) = (128usize, 128usize);
    let mut cards = vec![
        "SIMPLE  =                    T".to_string(),
        "BITPIX  =                   16".to_string(),
        "NAXIS   =                    2".to_string(),
        format!("NAXIS1  = {:>20}", w),
        format!("NAXIS2  = {:>20}", h),
        "BZERO   =                32768".to_string(),
        "BSCALE  =                    1".to_string(),
    ];
    if let Some(pattern) = bayer {
        cards.push(format!("BAYERPAT= '{pattern:<8}'"));
    }
    cards.push("END".to_string());
    let blocks = (cards.len() * CARD).div_ceil(BLOCK);
    let mut header = vec![b' '; blocks * BLOCK];
    for (i, c) in cards.iter().enumerate() {
        header[i * CARD..i * CARD + c.len()].copy_from_slice(c.as_bytes());
    }
    // A realistic frame, not three spikes: a sky background with noise and a
    // gentle gradient, tinted per channel. An autostretch derives its clip
    // point from the noise, so a histogram with no width makes it crush
    // everything below the brightest spike — pathological input, not a bug,
    // but useless for checking that colour survives.
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            // Deterministic hash noise: no rand dependency, same every run.
            let hash = ((x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663))
                .wrapping_mul(2_654_435_761);
            let noise = (hash >> 20) as f64 / 4096.0 * 900.0;
            let background = 8000.0 + (x + y) as f64 * 12.0 + noise;
            let tint = match (x % 2, y % 2) {
                (0, 0) => 1.35, // red site
                (1, 1) => 0.70, // blue site
                _ => 1.0,       // green sites
            };
            // Stars, so the stacker has something to register on. They span
            // several pixels and therefore land on red, green, and blue
            // sites, which is what makes them survive the debayer as stars
            // rather than as colour fringes.
            let stars = STAR_POSITIONS
                .iter()
                .map(|&(sx, sy)| {
                    let (dx, dy) = (x as f64 - sx, y as f64 - sy);
                    22000.0 * (-(dx * dx + dy * dy) / (2.0 * 1.6 * 1.6)).exp()
                })
                .sum::<f64>();
            let raw = ((background + stars) * tint).clamp(0.0, 65535.0) as u16;
            data.extend_from_slice(&((raw as i32 - 32768) as i16).to_be_bytes());
        }
    }
    data.resize(data.len().div_ceil(BLOCK) * BLOCK, 0);
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&data).unwrap();
}

#[test]
fn colour_render_is_actually_coloured() {
    let dir = tempfile::TempDir::new().unwrap();
    let fits = dir.path().join("osc.fits");
    write_fits(&fits, Some("RGGB"));
    let out = dir.path().join("osc.png");

    let rendered = psf_guard::commands::stretch_to_png::render_color_preview(
        &fits.to_string_lossy(),
        Some(out.to_string_lossy().into_owned()),
        0.2,
        -2.8,
        None,
        psf_guard::preview_format::PreviewEncoding::png(),
    )
    .unwrap();
    assert!(rendered, "a BAYERPAT frame should render in colour");

    let img = image::open(&out).unwrap().to_rgb8();
    let mid = img.get_pixel(img.width() / 2, img.height() / 2);
    println!("centre pixel = {:?}", mid);
    // The mosaic is R > G > B, and one shared transfer preserves that order.
    // Per-channel statistics would flatten it towards grey, and a swapped
    // pattern would reverse it.
    assert!(mid[0] > mid[1], "red should lead green: {mid:?}");
    assert!(mid[1] > mid[2], "green should lead blue: {mid:?}");
    // A per-channel stretch would drag all three toward a common median and
    // return something close to grey, which this spread rules out.
    assert!(
        i32::from(mid[0]) - i32::from(mid[2]) > 40,
        "channels should stay clearly separated, not wash out to grey: {mid:?}"
    );
}

#[test]
fn a_mono_frame_reports_that_it_has_no_colour() {
    let dir = tempfile::TempDir::new().unwrap();
    let fits = dir.path().join("mono.fits");
    write_fits(&fits, None);

    let rendered = psf_guard::commands::stretch_to_png::render_color_preview(
        &fits.to_string_lossy(),
        Some(dir.path().join("mono.png").to_string_lossy().into_owned()),
        0.2,
        -2.8,
        None,
        psf_guard::preview_format::PreviewEncoding::png(),
    )
    .unwrap();
    assert!(
        !rendered,
        "a frame with no BAYERPAT has no colour to render, and the caller \
         must be told so it can fall back to greyscale"
    );
}

/// A stack of one-shot-color frames must come out in colour.
///
/// This is the contract `stack_preview.rs` relies on: it sizes its memory
/// budget for three channels when the reference frame carries a bayer
/// pattern, and its renderer emits RGB for a three-channel result. If the
/// stacker ever stopped debayering on ingest, an OSC stack would silently
/// become greyscale and the memory estimate would be three times too large,
/// with nothing else failing.
#[test]
fn stacking_one_shot_color_frames_yields_three_channels() {
    use seiza_stacking::{FitsFrame, LiveStacker, NormalizationMode, StackOptions};

    let dir = tempfile::TempDir::new().unwrap();
    let first = dir.path().join("osc-1.fits");
    let second = dir.path().join("osc-2.fits");
    write_fits(&first, Some("RGGB"));
    write_fits(&second, Some("RGGB"));

    let reference = FitsFrame::open(&first).unwrap();
    assert!(
        reference.bayer.is_some(),
        "the stacker must recognise BAYERPAT, or nothing downstream debayers"
    );
    // The same test psf-guard makes when it plans the memory budget.
    assert_eq!(
        if reference.bayer.is_some() {
            3
        } else {
            reference.image.channels
        },
        3
    );

    let mut stacker = LiveStacker::new(
        reference,
        Default::default(),
        StackOptions {
            normalization: NormalizationMode::Global,
            ..StackOptions::default()
        },
    )
    .unwrap();
    stacker.push(FitsFrame::open(&second).unwrap()).unwrap();
    let stacked = stacker.into_snapshot().unwrap().image;

    assert_eq!(
        stacked.channels, 3,
        "an OSC stack must stay in colour, not collapse to luminance"
    );
    // And the colour must be the mosaic's, not an averaged grey.
    let centre = (stacked.height / 2) * stacked.width + stacked.width / 2;
    let pixel = &stacked.data[centre * 3..centre * 3 + 3];
    assert!(pixel[0] > pixel[2], "red should lead blue: {pixel:?}");
}
