use super::test_steps::{BoxBlur, Gain, Sharpen, Stretch};
use super::*;
use crate::image_steps::{Accepts, Domain, ImageShape, Region};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use uuid::Uuid;

const WIDTH: usize = 160;
const HEIGHT: usize = 96;

fn shape(channels: usize) -> ImageShape {
    ImageShape::new(WIDTH, HEIGHT, channels)
}

/// A smooth image with a slope, so that position and neighbors both matter.
fn image(channels: usize) -> Arc<MemoryImage> {
    let samples = (0..WIDTH * HEIGHT * channels)
        .map(|index| {
            let pixel = index / channels;
            let (x, y) = ((pixel % WIDTH) as f32, (pixel / WIDTH) as f32);
            1.0 + 0.3 * (x * 0.21).sin() * (y * 0.17).cos()
                + 0.004 * x
                + 0.1 * (index % channels) as f32
        })
        .collect();
    Arc::new(MemoryImage::new(shape(channels), Domain::Linear, samples).unwrap())
}

fn regions() -> [Region; 5] {
    [
        Region::new(40, 30, 50, 40),
        Region::new(0, 0, 30, 20),
        Region::new(3, 5, 40, 40),
        Region::new(WIDTH - 21, HEIGHT - 13, 21, 13),
        Region::whole(WIDTH, HEIGHT),
    ]
}

/// The largest difference between a region read and the same pixels of a
/// whole-image read.
fn worst_mismatch(reader: &dyn RegionReader) -> f32 {
    let shape = reader.shape();
    let whole = reader.read(shape.region()).unwrap();
    let mut worst = 0f32;
    for region in regions() {
        let part = reader.read(region).unwrap();
        let reference = region.copy_from(&whole, shape).unwrap();
        for (a, b) in part.iter().zip(&reference) {
            worst = worst.max((a - b).abs());
        }
    }
    worst
}

fn dab(x: f32, y: f32, radius: f32, mode: StrokeMode) -> Stroke {
    Stroke {
        points: vec![[x, y]],
        radius,
        hardness: 0.5,
        mode,
    }
}

// ---- readers and steps -------------------------------------------------

#[test]
fn stacked_steps_read_any_region_as_the_whole_image_does() {
    for channels in [1, 3] {
        let pipeline = Pipeline::new(vec![
            Arc::new(BoxBlur { radius: 3 }),
            Arc::new(Gain(1.5)),
            Arc::new(BoxBlur { radius: 2 }),
        ]);
        assert_eq!(pipeline.padding(), 5);
        let reader = pipeline.over(image(channels)).unwrap();
        assert_eq!(worst_mismatch(reader.as_ref()), 0.0);
    }
}

#[test]
fn a_step_is_refused_where_its_samples_are_wrong() {
    let stretched = Pipeline::new(vec![Arc::new(Stretch)])
        .over(image(1))
        .unwrap();
    assert_eq!(stretched.domain(), Domain::Display);
    let refused = Applied::new(stretched, Arc::new(Sharpen));
    let Err(StudioError::Domain(refusal)) = refused else {
        panic!("a linear step was applied to display samples");
    };
    assert_eq!(refusal.expected, Domain::Linear);
    assert_eq!(refusal.given, Domain::Display);
}

// ---- memory ------------------------------------------------------------

/// Records the largest block any read asked a source for.
struct Watched {
    inner: Arc<MemoryImage>,
    largest_read: AtomicU64,
}

impl RegionReader for Watched {
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
        self.largest_read
            .fetch_max(self.inner.peak_bytes(region), Ordering::Relaxed);
        self.inner.read(region)
    }
}

#[test]
fn a_read_reports_its_price_before_it_runs() {
    let source = image(1);
    let region = Region::new(40, 30, 50, 40);
    assert_eq!(source.peak_bytes(region), 50 * 40 * 4);
    // Sharpen pads by 8 and holds its input, its result and 8 working copies.
    let sharpened = Applied::new(source, Arc::new(Sharpen)).unwrap();
    assert_eq!(sharpened.peak_bytes(region), 66 * 56 * 4 * 10);
}

#[test]
fn a_read_over_budget_is_cut_into_tiles_and_gives_the_same_samples() {
    let watched = Arc::new(Watched {
        inner: image(3),
        largest_read: AtomicU64::new(0),
    });
    let pipeline = Pipeline::new(vec![Arc::new(BoxBlur { radius: 4 }), Arc::new(Gain(0.5))]);
    let reader = pipeline.over(watched.clone()).unwrap();
    let whole = shape(3).region();
    let uncut = reader.read(whole).unwrap();
    let uncut_read = watched.largest_read.swap(0, Ordering::Relaxed);

    let budget = MemoryBudget {
        bytes: reader.peak_bytes(whole) / 6,
        smallest_tile: 16,
    };
    let tiled = read_within(reader.as_ref(), whole, budget).unwrap();
    assert_eq!(tiled, uncut);
    // No tile's read from the source came near the whole image.
    let tiled_read = watched.largest_read.load(Ordering::Relaxed);
    assert!(
        tiled_read * 3 < uncut_read,
        "{tiled_read} against {uncut_read}"
    );
}

#[test]
fn a_read_that_cannot_fit_is_refused_with_both_numbers() {
    let reader = Pipeline::new(vec![Arc::new(Sharpen)])
        .over(image(1))
        .unwrap();
    let budget = MemoryBudget {
        bytes: 1024,
        smallest_tile: 32,
    };
    let refused = read_within(reader.as_ref(), shape(1).region(), budget);
    let Err(StudioError::OverBudget {
        tile,
        needed,
        limit,
    }) = refused
    else {
        panic!("a read over budget went ahead");
    };
    assert_eq!((tile, limit), (32, 1024));
    // The price is that of a tile inside the image, padded by 8 on every
    // side, not of the corner tile, which is padded on two.
    assert_eq!(needed, 48 * 48 * 4 * 10);
}

// ---- views at a pyramid level -----------------------------------------

#[test]
fn the_level_follows_the_zoom_and_never_undersamples() {
    assert_eq!(level_for_zoom(1.0, 7), 0);
    assert_eq!(level_for_zoom(2.0, 7), 0);
    assert_eq!(level_for_zoom(0.51, 7), 0);
    assert_eq!(level_for_zoom(0.5, 7), 1);
    assert_eq!(level_for_zoom(0.1, 7), 3);
    assert_eq!(level_for_zoom(0.0001, 7), 6);
    assert_eq!(level_for_zoom(0.0, 7), 6);
}

#[test]
fn a_reduced_view_scales_radii_and_names_the_steps_it_leaves_out() {
    let source = image(1);
    let pipeline = Pipeline::new(vec![
        Arc::new(Sharpen),
        Arc::new(BoxBlur { radius: 8 }),
        Arc::new(Gain(2.0)),
    ]);
    let level = 2;
    let view = pipeline
        .over_level(Arc::new(source.reduced(level)), level)
        .unwrap();
    assert_eq!(view.omitted, ["Sharpen"]);

    // The view is close to the full-resolution result reduced, without the
    // step it left out.
    let shown = view.reader.read(view.reader.shape().region()).unwrap();
    let without_sharpen = Pipeline::new(pipeline.steps()[1..].to_vec())
        .over(source.clone())
        .unwrap();
    let full = without_sharpen.read(shape(1).region()).unwrap();
    let reduced = MemoryImage::new(shape(1), Domain::Linear, full)
        .unwrap()
        .reduced(level);
    let reference = reduced.read(reduced.shape().region()).unwrap();
    let distance = |samples: &[f32]| {
        samples
            .iter()
            .zip(&reference)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max)
    };
    // A box blur's width cannot scale exactly, so the view is close, not
    // equal. It is much closer than a view that did not blur at all.
    let unblurred = Pipeline::new(vec![Arc::new(Gain(2.0))])
        .over(Arc::new(source.reduced(level)))
        .unwrap();
    let unblurred = unblurred.read(unblurred.shape().region()).unwrap();
    assert!(
        distance(&shown) < 0.1,
        "the view is {} off",
        distance(&shown)
    );
    assert!(distance(&shown) * 3.0 < distance(&unblurred));

    // At level 0 nothing is left out.
    let exact = pipeline.over_level(source, 0).unwrap();
    assert!(exact.omitted.is_empty());
}

// ---- painted masks -----------------------------------------------------

#[test]
fn strokes_add_and_subtract_in_order() {
    let mask = PaintedMask {
        strokes: vec![
            dab(50.0, 40.0, 20.0, StrokeMode::Add),
            dab(50.0, 40.0, 6.0, StrokeMode::Subtract),
        ],
    };
    let region = Region::new(20, 10, 60, 60);
    let drawn = mask.mask(region, 0).unwrap();
    let at = |x: usize, y: usize| drawn.values()[(y - region.y) * region.width + (x - region.x)];
    assert_eq!(at(50, 40), 0.0, "the centre was subtracted");
    assert_eq!(at(58, 40), 1.0, "inside the hard part of the first stroke");
    assert!(at(67, 40) > 0.0 && at(67, 40) < 1.0, "the feathered edge");
    assert_eq!(at(75, 40), 0.0, "outside the stroke");
    // The other order leaves the centre selected.
    let reversed = PaintedMask {
        strokes: mask.strokes.iter().rev().cloned().collect(),
    };
    let drawn = reversed.mask(region, 0).unwrap();
    assert_eq!(
        drawn.values()[(40 - region.y) * region.width + (50 - region.x)],
        1.0
    );
}

#[test]
fn a_mask_drawn_for_a_region_matches_the_same_pixels_of_a_larger_one() {
    let mask = PaintedMask {
        strokes: vec![Stroke {
            points: vec![[10.0, 12.0], [90.0, 60.0], [140.0, 20.0]],
            radius: 9.0,
            hardness: 0.3,
            mode: StrokeMode::Add,
        }],
    };
    let whole = mask.mask(Region::whole(WIDTH, HEIGHT), 0).unwrap();
    for region in regions() {
        let part = mask.mask(region, 0).unwrap();
        assert_eq!(part, whole.region(region).unwrap(), "{region:?}");
    }
}

#[test]
fn a_mask_at_a_reduced_level_covers_the_same_part_of_the_image() {
    let mask = PaintedMask {
        strokes: vec![dab(80.0, 48.0, 24.0, StrokeMode::Add)],
    };
    let full = mask.mask(Region::whole(WIDTH, HEIGHT), 0).unwrap();
    let reduced = mask.mask(Region::whole(WIDTH / 4, HEIGHT / 4), 2).unwrap();
    assert!((full.coverage() - reduced.coverage()).abs() < 0.005);
}

#[test]
fn a_masked_step_changes_only_what_the_mask_selects() {
    let source = image(3);
    let mask = Arc::new(PaintedMask {
        strokes: vec![dab(60.0, 50.0, 15.0, StrokeMode::Add)],
    });
    let masked: Arc<dyn Step> = Arc::new(Masked::new(Arc::new(Gain(3.0)), mask));
    let reader = Pipeline::new(vec![masked]).over(source.clone()).unwrap();
    assert_eq!(worst_mismatch(reader.as_ref()), 0.0);

    let before = source.read(shape(3).region()).unwrap();
    let after = reader.read(shape(3).region()).unwrap();
    let sample = |samples: &[f32], x: usize, y: usize| samples[(y * WIDTH + x) * 3];
    assert_eq!(sample(&after, 60, 50), 3.0 * sample(&before, 60, 50));
    assert_eq!(sample(&after, 100, 50), sample(&before, 100, 50));
}

// ---- heal --------------------------------------------------------------

#[test]
fn healing_removes_a_blemish_and_meets_the_surroundings() {
    // A sloped background with one bright blemish.
    let (blemish_x, blemish_y) = (70usize, 40usize);
    let background = |x: usize| 1.0 + 0.01 * x as f32;
    let samples: Vec<f32> = (0..WIDTH * HEIGHT)
        .map(|pixel| {
            let (x, y) = (pixel % WIDTH, pixel / WIDTH);
            // Smaller than the brush's fully covered core, which is 4 pixels.
            let near = x.abs_diff(blemish_x) <= 2 && y.abs_diff(blemish_y) <= 2;
            background(x) + if near { 5.0 } else { 0.0 }
        })
        .collect();
    let source = Arc::new(MemoryImage::new(shape(1), Domain::Linear, samples).unwrap());

    // Copy from 30 pixels to the right, where the background is 0.3 higher.
    let stroke = dab(blemish_x as f32, blemish_y as f32, 8.0, StrokeMode::Add);
    let heal = Heal::resolve(source.as_ref(), stroke, [30, 0]).unwrap();
    assert!((heal.shift()[0] + 0.3).abs() < 1e-3, "{:?}", heal.shift());
    assert_eq!(heal.padding(), 30);

    let reader = Pipeline::new(vec![Arc::new(heal)]).over(source).unwrap();
    let healed = reader.read(shape(1).region()).unwrap();
    for y in blemish_y - 2..=blemish_y + 2 {
        for x in blemish_x - 2..=blemish_x + 2 {
            let value = healed[y * WIDTH + x];
            assert!(
                (value - background(x)).abs() < 1e-3,
                "({x}, {y}) is {value}"
            );
        }
    }
    // A crop that holds the stroke heals exactly as the whole image does.
    assert_eq!(worst_mismatch(reader.as_ref()), 0.0);
}

#[test]
fn healing_from_outside_the_image_leaves_the_pixels_alone() {
    let source = image(1);
    let stroke = dab(5.0, 5.0, 4.0, StrokeMode::Add);
    let heal = Heal::resolve(source.as_ref(), stroke, [-40, 0]).unwrap();
    let reader = Pipeline::new(vec![Arc::new(heal)])
        .over(source.clone())
        .unwrap();
    let whole = shape(1).region();
    assert_eq!(reader.read(whole).unwrap(), source.read(whole).unwrap());
}

// ---- junctions ---------------------------------------------------------

#[test]
fn a_junction_tells_an_edit_from_a_file_that_was_only_rewritten() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("linear.tif");
    std::fs::write(&path, b"as Studio wrote it").unwrap();

    let mut junction = Junction::default();
    assert_eq!(junction.status("key-1"), JunctionStatus::Computed);
    junction.exported(&path, "key-1").unwrap();
    assert_eq!(junction.status("key-1"), JunctionStatus::WaitingForEdit);
    assert!(!junction.check_for_edit().unwrap());

    // Saved again with the same content: a new time, and not an edit.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(later).unwrap();
    assert!(!junction.check_for_edit().unwrap());
    assert_eq!(junction.status("key-1"), JunctionStatus::WaitingForEdit);
    assert_eq!(junction.edited_file(), None);

    // A real edit.
    std::fs::write(&path, b"corrected in an editor").unwrap();
    assert!(junction.check_for_edit().unwrap());
    assert_eq!(junction.status("key-1"), JunctionStatus::Edited);
    assert_eq!(junction.edited_file(), Some(path.as_path()));
    assert!(
        !junction.check_for_edit().unwrap(),
        "the same edit is reported once"
    );

    // A step before the junction changes: the edit is from an older result.
    assert_eq!(
        junction.status("key-2"),
        JunctionStatus::EditedFromOlderResult
    );

    // The state survives a save and a load.
    let json = serde_json::to_string(&junction).unwrap();
    let loaded: Junction = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded, junction);

    // Edited back to what Studio wrote: no longer an edit.
    std::fs::write(&path, b"as Studio wrote it").unwrap();
    assert!(junction.check_for_edit().unwrap());
    assert_eq!(junction.status("key-1"), JunctionStatus::WaitingForEdit);

    junction.use_computed();
    assert_eq!(junction.status("key-1"), JunctionStatus::Computed);
    assert!(path.exists(), "going back keeps the file");
}

#[test]
fn a_missing_file_is_an_error_that_names_the_file() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("gone.tif");
    let mut junction = Junction::default();
    let error = junction.exported(&path, "key").unwrap_err();
    assert!(error.to_string().contains("gone.tif"), "{error}");
}

#[cfg(target_os = "macos")]
#[test]
fn the_editor_command_names_the_application_and_the_file() {
    let file = std::path::Path::new("/tmp/linear.tif");
    let command = open_in_editor_command(Some("Adobe Photoshop 2026"), file);
    let arguments: Vec<_> = command.get_args().collect();
    assert_eq!(command.get_program(), "open");
    assert_eq!(arguments, ["-a", "Adobe Photoshop 2026", "/tmp/linear.tif"]);
    let command = open_in_editor_command(None, file);
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["/tmp/linear.tif"]);
}

// ---- documents ---------------------------------------------------------

fn operation(name: &str, accepts: Accepts) -> StepRecord {
    StepRecord::new(StepKind::Operation {
        operation: name.to_string(),
        accepts,
        settings: serde_json::json!({ "amount": 0.35 }),
    })
}

#[test]
fn a_document_has_every_stage_in_order_and_survives_a_round_trip() {
    let mut document = Document::new("M31, attempt 2");
    let kinds: Vec<StageKind> = document.stages().iter().map(|stage| stage.kind).collect();
    assert_eq!(kinds, StageKind::ALL);

    document
        .insert_step(
            StageKind::Linear,
            0,
            operation("Deconvolution", Accepts::Linear),
        )
        .unwrap();
    let mut curves = operation("Curves", Accepts::Display);
    curves.mask = Some(MaskRecord::Painted(PaintedMask {
        strokes: vec![dab(10.0, 10.0, 5.0, StrokeMode::Add)],
    }));
    document.insert_step(StageKind::Finish, 0, curves).unwrap();
    document
        .insert_step(
            StageKind::Finish,
            0,
            StepRecord::new(StepKind::Heal {
                stroke: dab(40.0, 40.0, 6.0, StrokeMode::Add),
                offset: [12, -3],
                shift: vec![0.01, 0.0, -0.02],
            }),
        )
        .unwrap();
    assert_eq!(
        document.stage(StageKind::Finish).steps[0].kind.name(),
        "Heal"
    );

    let loaded = Document::from_json(&document.to_json()).unwrap();
    assert_eq!(loaded, document);

    let heal = document.stage(StageKind::Finish).steps[0].id;
    assert!(document.remove_step(heal).is_some());
    assert_eq!(document.stage(StageKind::Finish).steps.len(), 1);
    assert!(document.remove_step(heal).is_none());
}

#[test]
fn a_step_is_refused_in_a_stage_whose_samples_it_does_not_work_on() {
    let mut document = Document::new("test");
    let refused = document.insert_step(
        StageKind::Finish,
        0,
        operation("Deconvolution", Accepts::Linear),
    );
    assert!(matches!(refused, Err(StudioError::Domain(_))));
    let refused = document.insert_step(StageKind::Linear, 0, operation("Curves", Accepts::Display));
    assert!(matches!(refused, Err(StudioError::Domain(_))));

    let heal = StepRecord::new(StepKind::Heal {
        stroke: dab(1.0, 1.0, 2.0, StrokeMode::Add),
        offset: [4, 0],
        shift: vec![0.0],
    });
    let refused = document.insert_step(StageKind::Export, 0, heal.clone());
    assert!(matches!(refused, Err(StudioError::Document(_))));
    assert!(document.insert_step(StageKind::Linear, 0, heal).is_ok());
}

#[test]
fn a_document_from_a_newer_build_or_with_broken_stages_is_refused() {
    let document = Document::new("test");
    let newer = document
        .to_json()
        .replace("\"version\": 1", "\"version\": 99");
    let error = Document::from_json(&newer).unwrap_err();
    assert!(error.to_string().contains("newer version"), "{error}");

    let mut value: serde_json::Value = serde_json::from_str(&document.to_json()).unwrap();
    value["stages"].as_array_mut().unwrap().swap(0, 1);
    let error = Document::from_json(&value.to_string()).unwrap_err();
    assert!(error.to_string().contains("out of order"), "{error}");
}

#[test]
fn a_stage_is_spelled_as_a_word() {
    for stage in StageKind::ALL {
        assert_eq!(stage.to_string().parse::<StageKind>().unwrap(), stage);
        assert_eq!(
            serde_json::to_string(&stage).unwrap(),
            format!("\"{stage}\"")
        );
    }
    assert!("develop".parse::<StageKind>().is_err());
    assert_eq!(StageKind::Stretch.works_on(), Domain::Linear);
    assert_eq!(StageKind::Combine.works_on(), Domain::Display);
}

#[test]
fn combine_holds_layers_with_their_own_steps_and_order() {
    let mut document = Document::new("SHO");
    for band in ["SII", "Ha", "OIII"] {
        let layer = LayerRecord::new(
            band,
            LayerSource::Band {
                band: band.to_string(),
            },
        );
        document.insert_layer(usize::MAX, layer).unwrap();
    }
    let names = |document: &Document| -> Vec<String> {
        document
            .layers()
            .iter()
            .map(|layer| layer.name.clone())
            .collect()
    };
    assert_eq!(names(&document), ["SII", "Ha", "OIII"]);

    // An adjustment to one layer alone.
    let ha = document.layers()[1].id;
    document
        .insert_layer_step(ha, operation("Curves", Accepts::Display))
        .unwrap();
    let refused = document.insert_layer_step(ha, operation("Deconvolution", Accepts::Linear));
    assert!(matches!(refused, Err(StudioError::Domain(_))));
    assert_eq!(document.layers()[1].steps.len(), 1);
    assert!(document.stage(StageKind::Finish).steps.is_empty());

    let layer = document.layer_mut(ha).unwrap();
    layer.blend = BlendMode::Screen;
    layer.opacity = 0.6;
    layer.color = Some([1.0, 0.2, 0.1]);

    assert!(document.move_layer(ha, 0));
    assert_eq!(names(&document), ["Ha", "SII", "OIII"]);

    // A layer made from an earlier step's result, kept as it was.
    let stars = LayerRecord::new(
        "Stars",
        LayerSource::Step {
            step: Uuid::new_v4(),
            frozen: true,
        },
    );
    document.insert_layer(usize::MAX, stars).unwrap();
    let mut faint = LayerRecord::new("x", LayerSource::Band { band: "L".into() });
    faint.opacity = 1.5;
    assert!(matches!(
        document.insert_layer(0, faint),
        Err(StudioError::Document(_))
    ));

    let loaded = Document::from_json(&document.to_json()).unwrap();
    assert_eq!(loaded, document);
    assert_eq!(loaded.layers()[0].blend, BlendMode::Screen);
    assert!(document.remove_layer(ha).is_some());
    assert_eq!(document.layers().len(), 3);
}

#[test]
fn a_blend_mode_is_spelled_as_a_word() {
    for mode in BlendMode::ALL {
        assert_eq!(mode.to_string().parse::<BlendMode>().unwrap(), mode);
        assert_eq!(serde_json::to_string(&mode).unwrap(), format!("\"{mode}\""));
    }
    assert!("overlay".parse::<BlendMode>().is_err());
}

#[test]
fn a_step_is_edited_in_place() {
    let mut document = Document::new("test");
    let curves = operation("Curves", Accepts::Display);
    let id = curves.id;
    document.insert_step(StageKind::Finish, 0, curves).unwrap();
    let source = operation("Register", Accepts::Linear);
    let source_id = source.id;
    document.insert_step(StageKind::Sources, 0, source).unwrap();

    assert!(document.set_step_settings(id, serde_json::json!({ "amount": 0.8 })));
    document
        .paint_step_mask(id, dab(5.0, 5.0, 3.0, StrokeMode::Add))
        .unwrap();
    document
        .paint_step_mask(id, dab(9.0, 5.0, 3.0, StrokeMode::Subtract))
        .unwrap();
    assert!(document.set_step_enabled(id, false));

    let (stage, step) = document.step(id).unwrap();
    assert_eq!(stage, StageKind::Finish);
    assert!(!step.enabled);
    let StepKind::Operation { settings, .. } = &step.kind else {
        panic!("the step changed kind");
    };
    assert_eq!(settings["amount"], 0.8);
    let Some(MaskRecord::Painted(mask)) = &step.mask else {
        panic!("the step has no painted mask");
    };
    assert_eq!(mask.strokes.len(), 2);

    // Sources takes no manual edits, and an unknown step is refused.
    let refused = document.paint_step_mask(source_id, dab(1.0, 1.0, 1.0, StrokeMode::Add));
    assert!(matches!(refused, Err(StudioError::Document(_))));
    assert!(!document.set_step_settings(Uuid::new_v4(), serde_json::Value::Null));
    assert!(document
        .paint_step_mask(Uuid::new_v4(), dab(1.0, 1.0, 1.0, StrokeMode::Add))
        .is_err());
}
