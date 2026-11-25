use super::test_steps::{BoxBlur, Gain};
use super::*;
use crate::image_steps::{Domain, ImageShape, Region};
use std::sync::Arc;

// Odd sizes that are not multiples of the tile, so every edge case occurs.
const WIDTH: usize = 301;
const HEIGHT: usize = 173;
const TILE: usize = 64;

fn shape(channels: usize) -> ImageShape {
    ImageShape::new(WIDTH, HEIGHT, channels)
}

fn image(channels: usize) -> Arc<MemoryImage> {
    let samples = (0..WIDTH * HEIGHT * channels)
        .map(|index| {
            let pixel = index / channels;
            let (x, y) = ((pixel % WIDTH) as f32, (pixel / WIDTH) as f32);
            0.2 + 0.1 * (x * 0.13).sin() * (y * 0.07).cos()
                + 0.0007 * x
                + 0.05 * (index % channels) as f32
        })
        .collect();
    Arc::new(MemoryImage::new(shape(channels), Domain::Linear, samples).unwrap())
}

fn options(sample: SampleFormat) -> StoreOptions {
    StoreOptions {
        tile: TILE,
        sample,
        budget: MemoryBudget::new(1 << 30),
        key: "source-1/settings-7".to_string(),
        channels: Vec::new(),
    }
}

fn regions() -> [Region; 6] {
    [
        Region::whole(WIDTH, HEIGHT),
        // Inside one tile, across tiles, and on each far edge.
        Region::new(70, 70, 20, 20),
        Region::new(50, 40, 130, 90),
        Region::new(WIDTH - 37, 0, 37, 50),
        Region::new(0, HEIGHT - 45, 80, 45),
        Region::new(WIDTH - 1, HEIGHT - 1, 1, 1),
    ]
}

#[test]
fn a_store_gives_back_every_region_exactly() {
    let folder = tempfile::tempdir().unwrap();
    for channels in [1, 3] {
        let source = image(channels);
        let path = folder.path().join(format!("image-{channels}.pgtiles"));
        let store = TileStore::create(&path, source.as_ref(), &options(SampleFormat::F32)).unwrap();
        assert_eq!(store.shape(), shape(channels));
        assert_eq!(store.key(), "source-1/settings-7");
        let level = store.level(0).unwrap();
        assert_eq!(level.domain(), Domain::Linear);
        for region in regions() {
            assert_eq!(
                level.read(region).unwrap(),
                source.read(region).unwrap(),
                "{region:?}"
            );
        }
        // Opened again from disk, it is the same store.
        let reopened = TileStore::open(&path).unwrap();
        let whole = Region::whole(WIDTH, HEIGHT);
        assert_eq!(
            reopened.level(0).unwrap().read(whole).unwrap(),
            source.read(whole).unwrap()
        );
    }
}

#[test]
fn half_floats_take_half_the_space_and_keep_three_digits() {
    let folder = tempfile::tempdir().unwrap();
    let source = image(3);
    let full = folder.path().join("full.pgtiles");
    let half = folder.path().join("half.pgtiles");
    TileStore::create(&full, source.as_ref(), &options(SampleFormat::F32)).unwrap();
    let store = TileStore::create(&half, source.as_ref(), &options(SampleFormat::F16)).unwrap();
    assert_eq!(store.sample(), SampleFormat::F16);

    let size = |path: &std::path::Path| std::fs::metadata(path).unwrap().len() as f64;
    assert!((size(&half) / size(&full) - 0.5).abs() < 0.01);

    let whole = Region::whole(WIDTH, HEIGHT);
    let stored = store.level(0).unwrap().read(whole).unwrap();
    let worst = stored
        .iter()
        .zip(source.read(whole).unwrap())
        .map(|(a, b)| ((a - b) / b).abs())
        .fold(0.0, f32::max);
    assert!(worst < 5e-4, "half floats were off by a share of {worst}");
}

#[test]
fn each_level_halves_the_one_before() {
    let folder = tempfile::tempdir().unwrap();
    let source = image(3);
    let path = folder.path().join("image.pgtiles");
    let store = TileStore::create(&path, source.as_ref(), &options(SampleFormat::F32)).unwrap();
    // 301 x 173, then 151 x 87, 76 x 44, and 38 x 22, which fits one tile.
    assert_eq!(store.levels(), 4);
    assert!(store.level(4).is_err());

    let whole = |shape: ImageShape| Region::whole(shape.width, shape.height);
    let mut expected = MemoryImage::new(
        shape(3),
        Domain::Linear,
        source.read(whole(shape(3))).unwrap(),
    )
    .unwrap();
    for level in 1..store.levels() {
        expected = expected.reduced(1);
        let stored = store.level(level).unwrap();
        assert_eq!(stored.shape(), expected.shape(), "level {level}");
        let region = whole(stored.shape());
        let (a, b) = (stored.read(region).unwrap(), expected.read(region).unwrap());
        let worst = a
            .iter()
            .zip(&b)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1e-6, "level {level} is {worst} from a halved image");
    }
    assert_eq!(store.level(3).unwrap().shape(), ImageShape::new(38, 22, 3));
}

#[test]
fn missing_samples_stay_missing_and_do_not_darken_the_pyramid() {
    let folder = tempfile::tempdir().unwrap();
    let mut samples = image(1).read(Region::whole(WIDTH, HEIGHT)).unwrap();
    // A registration border: the first two rows, and one lone pixel.
    for sample in &mut samples[..2 * WIDTH] {
        *sample = f32::NAN;
    }
    samples[10 * WIDTH + 10] = f32::NAN;
    let neighbors = [
        samples[10 * WIDTH + 11],
        samples[11 * WIDTH + 10],
        samples[11 * WIDTH + 11],
    ];
    let source = MemoryImage::new(shape(1), Domain::Linear, samples).unwrap();
    let path = folder.path().join("image.pgtiles");
    let store = TileStore::create(&path, &source, &options(SampleFormat::F32)).unwrap();

    let full = store
        .level(0)
        .unwrap()
        .read(Region::new(0, 0, 20, 20))
        .unwrap();
    assert!(full[0].is_nan() && full[20 + 5].is_nan() && full[10 * 20 + 10].is_nan());
    assert!(full[2 * 20].is_finite());

    let reduced = store
        .level(1)
        .unwrap()
        .read(Region::new(0, 0, 10, 10))
        .unwrap();
    // The first reduced row covers only missing rows.
    assert!(reduced[0].is_nan());
    // The pixel over the lone missing sample is the mean of its other three.
    let expected = neighbors.iter().sum::<f32>() / 3.0;
    assert!((reduced[5 * 10 + 5] - expected).abs() < 1e-6);
}

#[test]
fn a_store_is_a_source_for_steps_like_any_other() {
    let folder = tempfile::tempdir().unwrap();
    let source = image(3);
    let path = folder.path().join("image.pgtiles");
    let store = TileStore::create(&path, source.as_ref(), &options(SampleFormat::F32)).unwrap();
    let pipeline = Pipeline::new(vec![Arc::new(BoxBlur { radius: 5 }), Arc::new(Gain(1.5))]);
    let from_disk = pipeline.over(Arc::new(store.level(0).unwrap())).unwrap();
    let from_memory = pipeline.over(source).unwrap();
    for region in regions() {
        assert_eq!(
            from_disk.read(region).unwrap(),
            from_memory.read(region).unwrap(),
            "{region:?}"
        );
    }
}

#[test]
fn a_store_is_written_from_a_chain_within_a_small_budget() {
    let folder = tempfile::tempdir().unwrap();
    let source = image(3);
    let chain = Pipeline::new(vec![Arc::new(BoxBlur { radius: 4 })])
        .over(source)
        .unwrap();
    let whole = Region::whole(WIDTH, HEIGHT);
    let mut tight = options(SampleFormat::F32);
    // Less than one 64-pixel tile's read from the chain needs, so each tile
    // is itself cut into smaller reads.
    tight.budget = MemoryBudget {
        bytes: chain.peak_bytes(Region::new(70, 70, TILE, TILE)) / 3,
        smallest_tile: 8,
    };
    let path = folder.path().join("blurred.pgtiles");
    let store = TileStore::create(&path, chain.as_ref(), &tight).unwrap();
    assert_eq!(
        store.level(0).unwrap().read(whole).unwrap(),
        chain.read(whole).unwrap()
    );
}

#[test]
fn a_read_from_a_store_is_priced_and_bounded() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("image.pgtiles");
    let store = TileStore::create(&path, image(3).as_ref(), &options(SampleFormat::F32)).unwrap();
    let level = store.level(0).unwrap();
    let region = Region::new(10, 10, 100, 50);
    // The region's samples and one tile's bytes: nothing that grows with
    // the image.
    assert_eq!(
        level.peak_bytes(region),
        (100 * 50 * 3 * 4 + TILE * TILE * 3 * 4) as u64
    );
    assert!(level.read(Region::new(WIDTH - 5, 0, 10, 10)).is_err());
}

/// A source that fails after a number of reads.
struct Failing {
    inner: Arc<MemoryImage>,
    reads_left: std::sync::atomic::AtomicI32,
}

impl RegionReader for Failing {
    fn shape(&self) -> ImageShape {
        self.inner.shape()
    }
    fn domain(&self) -> Domain {
        self.inner.domain()
    }
    fn peak_bytes(&self, region: Region) -> u64 {
        self.inner.peak_bytes(region)
    }
    fn read(&self, region: Region) -> Result<Vec<f32>, StudioError> {
        if self
            .reads_left
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed)
            <= 0
        {
            return Err(StudioError::Step {
                step: "Source".to_string(),
                reason: "the disk went away".to_string(),
            });
        }
        self.inner.read(region)
    }
}

#[test]
fn a_failed_write_leaves_the_old_store_and_no_partial_file() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("image.pgtiles");
    let first = image(1);
    TileStore::create(&path, first.as_ref(), &options(SampleFormat::F32)).unwrap();
    let before = std::fs::read(&path).unwrap();

    let failing = Failing {
        inner: image(3),
        reads_left: 4.into(),
    };
    let result = TileStore::create(&path, &failing, &options(SampleFormat::F32));
    assert!(matches!(result, Err(StudioError::Step { .. })));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the old store was touched"
    );
    let names: Vec<_> = std::fs::read_dir(folder.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["image.pgtiles"], "a partial file was left behind");
}

#[test]
fn a_file_that_is_not_a_whole_store_is_refused() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("image.pgtiles");
    TileStore::create(&path, image(1).as_ref(), &options(SampleFormat::F32)).unwrap();
    let bytes = std::fs::read(&path).unwrap();

    let refusal = |bytes: &[u8]| {
        let damaged = folder.path().join("damaged.pgtiles");
        std::fs::write(&damaged, bytes).unwrap();
        match TileStore::open(&damaged) {
            Err(StudioError::Store { reason, .. }) => reason,
            Err(other) => panic!("the wrong refusal: {other}"),
            Ok(_) => panic!("a damaged store was opened"),
        }
    };
    assert!(refusal(b"not a store").contains("does not start"));
    assert!(refusal(&bytes[..bytes.len() - 100]).contains("cut short"));
    assert!(refusal(&bytes[..40]).contains("cut short"));
    let mut newer = bytes.clone();
    let at = newer
        .windows(11)
        .position(|w| w == b"\"version\":1")
        .unwrap();
    newer[at + 10] = b'9';
    assert!(refusal(&newer).contains("format 9"));

    let missing = TileStore::open(&folder.path().join("none.pgtiles"));
    assert!(matches!(missing, Err(StudioError::File { .. })));
}

#[test]
fn a_store_keeps_its_channels_names() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("named.pgtiles");
    let named = StoreOptions {
        channels: vec!["red".into(), "green".into(), "blue".into()],
        ..options(SampleFormat::F32)
    };
    TileStore::create(&path, image(3).as_ref(), &named).unwrap();
    assert_eq!(
        TileStore::open(&path).unwrap().channel_names(),
        ["red", "green", "blue"]
    );
    // A store written without names has none.
    TileStore::create(&path, image(3).as_ref(), &options(SampleFormat::F32)).unwrap();
    assert!(TileStore::open(&path).unwrap().channel_names().is_empty());
    // A name for some channels and not others is refused.
    let short = StoreOptions {
        channels: vec!["red".into()],
        ..options(SampleFormat::F32)
    };
    assert!(TileStore::create(&path, image(3).as_ref(), &short).is_err());
}
