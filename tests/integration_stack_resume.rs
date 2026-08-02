//! An additive rebuild resumed from a checkpoint must equal the build that
//! integrated everything in one pass.
//!
//! This is the premise the stack-preview resume path stands on: Seiza's
//! context write/reopen round trip preserves the accumulator, its online
//! rejection state, and the registration reference exactly. If a resumed
//! stack ever drifted from an uninterrupted one, resuming would silently
//! change science pixels, so this is checked bit for bit.

use std::io::Write as _;

const STAR_POSITIONS: [(f64, f64); 9] = [
    (18.0, 21.0),
    (43.0, 12.0),
    (77.0, 26.0),
    (108.0, 31.0),
    (29.0, 53.0),
    (61.0, 67.0),
    (96.0, 58.0),
    (25.0, 91.0),
    (88.0, 104.0),
];

/// A registerable mono frame: sky background, deterministic noise, and a
/// star field shifted per frame the way dithered exposures are.
fn write_light(path: &std::path::Path, shift: (f64, f64), seed: u32) {
    const CARD: usize = 80;
    const BLOCK: usize = 2880;
    let (w, h) = (128usize, 128usize);
    let cards = [
        "SIMPLE  =                    T".to_string(),
        "BITPIX  =                   16".to_string(),
        "NAXIS   =                    2".to_string(),
        format!("NAXIS1  = {w:>20}"),
        format!("NAXIS2  = {h:>20}"),
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
    for y in 0..h {
        for x in 0..w {
            let hash = ((x as u32).wrapping_mul(73_856_093)
                ^ (y as u32).wrapping_mul(19_349_663)
                ^ seed.wrapping_mul(83_492_791))
            .wrapping_mul(2_654_435_761);
            let noise = (hash >> 20) as f64 / 4096.0 * 900.0;
            let background = 8000.0 + noise;
            let stars = STAR_POSITIONS
                .iter()
                .map(|&(sx, sy)| {
                    let (dx, dy) = (x as f64 - (sx + shift.0), y as f64 - (sy + shift.1));
                    22000.0 * (-(dx * dx + dy * dy) / (2.0 * 1.6 * 1.6)).exp()
                })
                .sum::<f64>();
            let raw = (background + stars).clamp(0.0, 65535.0) as u16;
            data.extend_from_slice(&((raw as i32 - 32768) as i16).to_be_bytes());
        }
    }
    data.resize(data.len().div_ceil(BLOCK) * BLOCK, 0);
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&data).unwrap();
}

#[test]
fn a_resumed_stack_is_bit_identical_to_an_uninterrupted_one() {
    use seiza_stacking::{
        FitsFrame, FrameDisposition, LiveStacker, NormalizationMode, StackOptions,
    };

    let dir = tempfile::TempDir::new().unwrap();
    let paths: Vec<_> = (0..4)
        .map(|index| dir.path().join(format!("light-{index}.fits")))
        .collect();
    let shifts = [(0.0, 0.0), (2.0, -1.0), (-1.5, 2.5), (1.0, 1.0)];
    for (index, path) in paths.iter().enumerate() {
        write_light(path, shifts[index], index as u32 + 1);
    }
    let options = || StackOptions {
        normalization: NormalizationMode::Global,
        ..StackOptions::default()
    };
    let push_expecting_accept = |stacker: &mut LiveStacker, path: &std::path::Path| match stacker
        .push(FitsFrame::open(path).unwrap())
        .unwrap()
    {
        FrameDisposition::Accepted(_) => {}
        FrameDisposition::Rejected(reason) => {
            panic!("synthetic frame {} was rejected: {reason}", path.display())
        }
    };

    // One uninterrupted pass over every frame.
    let mut uninterrupted = LiveStacker::new(
        FitsFrame::open(&paths[0]).unwrap(),
        Default::default(),
        options(),
    )
    .unwrap();
    for path in &paths[1..] {
        push_expecting_accept(&mut uninterrupted, path);
    }
    let expected = uninterrupted.into_snapshot().unwrap();

    // The same frames split across a checkpoint, the way an additive rebuild
    // sees them: two integrated, a checkpoint saved, two more added later.
    let mut first_half = LiveStacker::new(
        FitsFrame::open(&paths[0]).unwrap(),
        Default::default(),
        options(),
    )
    .unwrap();
    push_expecting_accept(&mut first_half, &paths[1]);
    let context = dir.path().join("checkpoint.seiza-stack");
    first_half.save_context(&context).unwrap();
    drop(first_half);

    let mut resumed = LiveStacker::open_context(&context).unwrap();
    for path in &paths[2..] {
        push_expecting_accept(&mut resumed, path);
    }
    let actual = resumed.into_snapshot().unwrap();

    assert_eq!(expected.accepted_frames, actual.accepted_frames);
    assert_eq!(expected.rejected_frames, actual.rejected_frames);
    assert_eq!(expected.image.width, actual.image.width);
    assert_eq!(expected.image.height, actual.image.height);
    let differing = expected
        .image
        .data
        .iter()
        .zip(&actual.image.data)
        .filter(|(left, right)| left.to_bits() != right.to_bits())
        .count();
    assert_eq!(
        differing, 0,
        "a resumed accumulator must not drift from an uninterrupted one"
    );
}
