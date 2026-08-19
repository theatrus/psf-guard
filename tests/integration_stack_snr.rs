//! Stacking more frames has to be seen to lower the noise.
//!
//! The progressive curve is the whole feature's evidence: if the estimator
//! did not track the noise of a real accumulation, every verdict and
//! projection built on it would be decoration. So this drives Seiza's live
//! stacker over synthetic frames whose noise is known to be independent from
//! frame to frame, reads the accumulator at the same depths a group build
//! reads it at, and checks that sixteen frames land within reach of the
//! quarter-noise that perfect averaging gives.

use psf_guard::server::stack_preview::snr;
use std::io::Write as _;

const STAR_POSITIONS: [(f64, f64); 9] = [
    (28.0, 31.0),
    (73.0, 22.0),
    (127.0, 46.0),
    (168.0, 51.0),
    (49.0, 93.0),
    (101.0, 117.0),
    (156.0, 98.0),
    (45.0, 151.0),
    (138.0, 164.0),
];

const WIDTH: usize = 200;
const HEIGHT: usize = 200;

/// A registerable mono frame: sky background, noise that is independent of
/// every other frame's, and a star field shifted the way a dithered exposure
/// shifts it.
fn write_light(path: &std::path::Path, shift: (f64, f64), seed: u32) {
    const CARD: usize = 80;
    const BLOCK: usize = 2880;
    let cards = [
        "SIMPLE  =                    T".to_string(),
        "BITPIX  =                   16".to_string(),
        "NAXIS   =                    2".to_string(),
        format!("NAXIS1  = {WIDTH:>20}"),
        format!("NAXIS2  = {HEIGHT:>20}"),
        "BZERO   =                32768".to_string(),
        "BSCALE  =                    1".to_string(),
        "EXPTIME =                 300.".to_string(),
        "END".to_string(),
    ];
    let blocks = (cards.len() * CARD).div_ceil(BLOCK);
    let mut header = vec![b' '; blocks * BLOCK];
    for (index, card) in cards.iter().enumerate() {
        header[index * CARD..index * CARD + card.len()].copy_from_slice(card.as_bytes());
    }
    let mut data = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let hash = ((x as u32).wrapping_mul(73_856_093)
                ^ (y as u32).wrapping_mul(19_349_663)
                ^ seed.wrapping_mul(83_492_791))
            .wrapping_mul(2_654_435_761);
            let noise = (hash >> 20) as f64 / 4096.0 * 900.0 - 450.0;
            let stars = STAR_POSITIONS
                .iter()
                .map(|&(sx, sy)| {
                    let (dx, dy) = (x as f64 - (sx + shift.0), y as f64 - (sy + shift.1));
                    22000.0 * (-(dx * dx + dy * dy) / (2.0 * 1.6 * 1.6)).exp()
                })
                .sum::<f64>();
            let raw = (8000.0 + noise + stars).clamp(0.0, 65535.0) as u16;
            data.extend_from_slice(&((raw as i32 - 32768) as i16).to_be_bytes());
        }
    }
    data.resize(data.len().div_ceil(BLOCK) * BLOCK, 0);
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&data).unwrap();
}

#[test]
fn sixteen_frames_measure_a_quarter_of_one_frame_s_noise() {
    use seiza_stacking::{
        FitsFrame, FrameDisposition, LiveStacker, NormalizationMode, StackOptions,
    };

    let dir = tempfile::TempDir::new().unwrap();
    let count = 16usize;
    let paths: Vec<_> = (0..count)
        .map(|index| dir.path().join(format!("light-{index:02}.fits")))
        .collect();
    for (index, path) in paths.iter().enumerate() {
        // A small deterministic dither, so registration has real work and the
        // frame edges end with uneven coverage the estimator has to skip.
        let angle = index as f64 * 0.7;
        let shift = (angle.cos() * 2.0, angle.sin() * 2.0);
        write_light(path, shift, index as u32 + 1);
    }

    let mut stacker = LiveStacker::new(
        FitsFrame::open(&paths[0]).unwrap(),
        Default::default(),
        StackOptions {
            normalization: NormalizationMode::Global,
            ..StackOptions::default()
        },
    )
    .unwrap();

    // Exactly what a group build does: push to the next depth on the ladder,
    // read the accumulator, push on.
    let depths = snr::checkpoint_depths(count);
    assert_eq!(depths, vec![1, 2, 4, 8, 16]);
    let mut points = Vec::new();
    let mut pushed = 1usize;
    for depth in depths {
        while pushed < depth {
            match stacker
                .push(FitsFrame::open(&paths[pushed]).unwrap())
                .unwrap()
            {
                FrameDisposition::Accepted(_) => {}
                FrameDisposition::Rejected(reason) => {
                    panic!("synthetic frame {pushed} was rejected: {reason}")
                }
            }
            pushed += 1;
        }
        let sample = snr::measure(stacker.view()).expect("a 200-pixel frame is measurable");
        assert_eq!(sample.frames as usize, depth);
        points.push(snr::point(sample, depth as f64 * 300.0));
    }

    let progressive = snr::ProgressiveSnr::new(snr::StackFrameOrder::Capture, points, &[300.0; 16]);
    let points = &progressive.points;
    for pair in points.windows(2) {
        assert!(
            pair[1].noise < pair[0].noise,
            "noise rose from {} frames ({}) to {} frames ({})",
            pair[0].frames,
            pair[0].noise,
            pair[1].frames,
            pair[1].noise
        );
        assert!(pair[1].snr > pair[0].snr, "the ratio has to rise with it");
    }

    let ratio = points[4].noise / points[0].noise;
    assert!(
        (ratio - 0.25).abs() < 0.05,
        "sixteen frames measured {ratio} of one frame's noise; perfect averaging gives 0.25"
    );
    // Every depth reads against one signal, so the ratio rises exactly as the
    // noise falls rather than carrying the shallow depths' inflated signal.
    let ratio_gain = points[4].snr / points[0].snr;
    assert!(
        (ratio_gain - 4.0).abs() < 0.8,
        "sixteen frames measured {ratio_gain} times one frame's ratio"
    );

    let analysis = progressive
        .analysis
        .clone()
        .expect("five depths is a curve");
    assert!(
        (analysis.noise_exponent + 0.5).abs() < 0.08,
        "fitted exponent {}",
        analysis.noise_exponent
    );
    assert_eq!(analysis.verdict, snr::SnrVerdict::Improving);
    assert!(analysis.regressions.is_empty());
    assert_eq!(analysis.measured_frames, 16);
    assert_eq!(analysis.measured_seconds, 4800.0);
    // Still on the ideal curve, so more frames are worth shooting and the
    // projection says how many.
    let projection = analysis
        .projections
        .iter()
        .find(|projection| (projection.gain - 1.10).abs() < 1e-9)
        .expect("a ten percent projection");
    assert!(
        (3..=5).contains(&projection.extra_frames),
        "ten percent more ratio off sixteen frames is about four more, not {}",
        projection.extra_frames
    );
}
