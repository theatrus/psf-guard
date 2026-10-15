//! The browser draws the framing rectangle itself with a port of this
//! crate's geometry. This fixture is the contract between the two: the core
//! must keep reproducing it, and `static/src/components/director/__tests__`
//! checks the port against the same file. Regenerate on purpose with
//! `PSF_GUARD_WRITE_FIXTURES=1 cargo test -p psf-guard-director-core --test framing_fixture`.

use psf_guard_director_core::{
    framing::{
        FramingRequest, Layout, LayoutIssueKind, MeasuredGeometry, MeasuredSource, Mosaic, Overlay,
        PanelPlacement, PanelSize, View,
    },
    visibility::IcrsPosition,
};

fn request() -> FramingRequest {
    FramingRequest {
        center: IcrsPosition {
            ra_degrees: 38.2,
            dec_degrees: 61.45,
        },
        position_angle_degrees: 35.0,
        panel: PanelSize {
            width_degrees: 2.0,
            height_degrees: 1.5,
        },
        layout: Layout::grid(Mosaic {
            rows: 2,
            columns: 2,
            overlap_percent: 20,
        }),
        overlays: vec![Overlay {
            id: "wide".into(),
            size: PanelSize {
                width_degrees: 5.0,
                height_degrees: 3.3,
            },
            position_angle_degrees: 35.0,
        }],
        // The view sits away from the target, so view offsets are not trivial.
        view: Some(View {
            center: IcrsPosition {
                ra_degrees: 38.9,
                dec_degrees: 61.1,
            },
            rotation_degrees: 0.0,
        }),
    }
}

/// A framed target kept in place, with panels added around it: one east
/// at the minimum, one north, and one dragged off its neighbour so the
/// fixture carries a thin overlap and a lock.
fn free_request() -> FramingRequest {
    let place = |id: &str, x: f64, y: f64, locked: bool| PanelPlacement {
        id: id.into(),
        x,
        y,
        locked,
        unlocked_by_hand: false,
        measured: None,
        angle_offset_degrees: 0.0,
    };
    FramingRequest {
        layout: Layout {
            min_overlap_percent: 15,
            panels: vec![
                place("r1c1", 0.0, 0.0, true),
                place("p2", -0.85, 0.0, false),
                place("p3", 0.0, 0.85, false),
                place("p4", -0.93, 0.85, false),
            ],
        },
        ..request()
    }
}

/// The framed target measured from its frames, shot with a wider camera a
/// degree and a half off the plan's angle, with planned panels around it
/// at the current camera's size: one east turned by 6°, one north, and one
/// left a gap.
fn measured_request() -> FramingRequest {
    let mut request = free_request();
    request.layout.panels[0].measured = Some(MeasuredGeometry {
        center: IcrsPosition {
            ra_degrees: 38.25,
            dec_degrees: 61.47,
        },
        position_angle_degrees: 36.5,
        size: PanelSize {
            width_degrees: 2.3,
            height_degrees: 1.6,
        },
        source: MeasuredSource::Solved,
        size_source: MeasuredSource::Solved,
        frames: 12,
    });
    request.layout.panels[1].x = -0.95;
    request.layout.panels[1].angle_offset_degrees = 6.0;
    request.layout.panels[2].y = 0.9;
    request.layout.panels[3] = PanelPlacement::at("p4", -1.2, 1.05);
    request
}

/// A large mosaic across 0h in the south, turned past a half turn: a 6 by
/// 6 grid, one panel turned with a real gap to its neighbour, a shot panel
/// across 0h, and one panel left on its own. A view and an overlay away
/// from the target.
fn wide_south_request() -> FramingRequest {
    let mut layout = Layout::grid(Mosaic {
        rows: 6,
        columns: 6,
        overlap_percent: 15,
    });
    // East of the grid's east column, turned 25° and just apart.
    let east = layout
        .panels
        .iter()
        .find(|panel| panel.id == "r3c1")
        .unwrap()
        .clone();
    let mut turned = PanelPlacement::at("p37", east.x - 1.1, east.y);
    turned.angle_offset_degrees = 25.0;
    layout.panels.push(turned);
    layout.panels.push(PanelPlacement::at("p38", 6.5, -4.0));
    let mut request = FramingRequest {
        center: IcrsPosition {
            ra_degrees: 359.6,
            dec_degrees: -38.0,
        },
        position_angle_degrees: 200.0,
        panel: PanelSize {
            width_degrees: 1.6,
            height_degrees: 1.1,
        },
        layout,
        overlays: vec![Overlay {
            id: "wide".into(),
            size: PanelSize {
                width_degrees: 4.0,
                height_degrees: 2.7,
            },
            position_angle_degrees: 200.0,
        }],
        view: Some(View {
            center: IcrsPosition {
                ra_degrees: 0.4,
                dec_degrees: -37.2,
            },
            rotation_degrees: 20.0,
        }),
    };
    // A corner panel shot a few arcminutes off its plan with a slightly
    // larger camera, turned 3°.
    let planned = request.preview().unwrap();
    let corner = planned
        .panels
        .iter()
        .find(|panel| panel.footprint.id == "r6c6")
        .unwrap()
        .footprint
        .center;
    let shot = request
        .layout
        .panels
        .iter_mut()
        .find(|panel| panel.id == "r6c6")
        .unwrap();
    shot.locked = true;
    shot.measured = Some(MeasuredGeometry {
        center: IcrsPosition {
            ra_degrees: (corner.ra_degrees + 0.04).rem_euclid(360.0),
            dec_degrees: corner.dec_degrees - 0.03,
        },
        position_angle_degrees: 203.0,
        size: PanelSize {
            width_degrees: 1.7,
            height_degrees: 1.15,
        },
        source: MeasuredSource::Header,
        size_source: MeasuredSource::Header,
        frames: 6,
    });
    request
}

/// Near the north celestial pole: two pairs of joined panels that never
/// meet, so the only issue is that they fall apart.
fn polar_request() -> FramingRequest {
    FramingRequest {
        center: IcrsPosition {
            ra_degrees: 120.0,
            dec_degrees: 88.6,
        },
        position_angle_degrees: 10.0,
        panel: PanelSize {
            width_degrees: 1.2,
            height_degrees: 0.8,
        },
        layout: Layout {
            min_overlap_percent: 20,
            panels: vec![
                PanelPlacement::at("r1c1", 0.0, 0.0),
                PanelPlacement::at("p2", -0.8, 0.0),
                PanelPlacement::at("p3", 2.5, 0.0),
                PanelPlacement::at("p4", 2.5, 0.75),
            ],
        },
        overlays: vec![],
        view: Some(View {
            center: IcrsPosition {
                ra_degrees: 300.0,
                dec_degrees: 89.5,
            },
            rotation_degrees: 0.0,
        }),
    }
}

/// Write the fixture when asked, then hold the core to it.
fn check(name: &str, request: FramingRequest) -> psf_guard_director_core::framing::FramingPreview {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let preview = request.preview().expect("a valid framing");
    let current = serde_json::json!({ "request": request, "preview": preview });
    if std::env::var_os("PSF_GUARD_WRITE_FIXTURES").is_some() {
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&current).unwrap() + "\n",
        )
        .unwrap();
    }
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture present")).unwrap();
    // Platform libm differs in the last bits of sin, cos and atan2, so the
    // numbers agree to a tolerance, the same one the browser test applies.
    let mut differences = Vec::new();
    compare(&saved, &current, "$", &mut differences);
    assert!(
        differences.is_empty(),
        "regenerate the fixture if this change is meant:\n{}",
        differences.join("\n")
    );
    preview
}

#[test]
fn the_browser_geometry_fixture_matches_the_core() {
    let preview = check("framing-preview.json", request());
    assert_eq!(preview.panels.len(), 4);
    assert!(preview
        .panels
        .iter()
        .all(|p| p.footprint.view_corners.is_some()));
    assert!(preview.issues.is_empty());
}

#[test]
fn the_measured_layout_fixture_matches_the_core() {
    let preview = check("framing-measured-layout.json", measured_request());
    assert_eq!(preview.panels.len(), 4);
    assert!(preview.panels[0].measured.is_some());
    assert_eq!(preview.panels[0].position_angle_degrees, 36.5);
    assert!(!preview.issues.is_empty(), "{:?}", preview.issues);
}

#[test]
fn the_wide_southern_fixture_matches_the_core() {
    let preview = check("framing-wide-south.json", wide_south_request());
    assert_eq!(preview.panels.len(), 38);
    let kinds: Vec<_> = preview.issues.iter().map(|issue| issue.kind).collect();
    assert!(
        kinds.contains(&LayoutIssueKind::Gap),
        "{:?}",
        preview.issues
    );
    assert!(
        kinds.contains(&LayoutIssueKind::Isolated),
        "{:?}",
        preview.issues
    );
    // Across 0h both ways.
    assert!(preview
        .panels
        .iter()
        .any(|panel| panel.footprint.center.ra_degrees < 10.0));
    assert!(preview
        .panels
        .iter()
        .any(|panel| panel.footprint.center.ra_degrees > 350.0));
}

#[test]
fn the_polar_fixture_matches_the_core() {
    let preview = check("framing-polar-disconnected.json", polar_request());
    assert_eq!(preview.issues.len(), 1, "{:?}", preview.issues);
    assert_eq!(preview.issues[0].kind, LayoutIssueKind::Disconnected);
}

#[test]
fn the_free_layout_fixture_matches_the_core() {
    let preview = check("framing-free-layout.json", free_request());
    assert_eq!(preview.panels.len(), 4);
    assert!(preview.panels[0].locked);
    assert_eq!(preview.issues.len(), 1, "{:?}", preview.issues);
}

/// Structural equality with numbers within 1e-9; every mismatch is named.
fn compare(
    saved: &serde_json::Value,
    current: &serde_json::Value,
    at: &str,
    out: &mut Vec<String>,
) {
    use serde_json::Value;
    match (saved, current) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            if (a - b).abs() > 1e-9 {
                out.push(format!("{at}: fixture {a} vs core {b}"));
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                out.push(format!("{at}: {} items vs {}", a.len(), b.len()));
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                compare(x, y, &format!("{at}[{i}]"), out);
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
                match (a.get(key), b.get(key)) {
                    (Some(x), Some(y)) => compare(x, y, &format!("{at}.{key}"), out),
                    _ => out.push(format!("{at}.{key}: present on one side only")),
                }
            }
        }
        _ if saved == current => {}
        _ => out.push(format!("{at}: {saved} vs {current}")),
    }
}
