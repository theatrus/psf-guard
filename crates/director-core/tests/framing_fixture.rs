//! The browser draws the framing rectangle itself with a port of this
//! crate's geometry. This fixture is the contract between the two: the core
//! must keep reproducing it, and `static/src/components/director/__tests__`
//! checks the port against the same file. Regenerate on purpose with
//! `PSF_GUARD_WRITE_FIXTURES=1 cargo test -p psf-guard-director-core --test framing_fixture`.

use psf_guard_director_core::{
    framing::{FramingRequest, Mosaic, Overlay, PanelSize, View},
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
        mosaic: Mosaic {
            rows: 2,
            columns: 2,
            overlap_percent: 20,
        },
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

#[test]
fn the_browser_geometry_fixture_matches_the_core() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/framing-preview.json"
    );
    let request = request();
    let preview = request.preview().expect("a valid framing");
    let current = serde_json::json!({ "request": request, "preview": preview });
    if std::env::var_os("PSF_GUARD_WRITE_FIXTURES").is_some() {
        std::fs::write(path, serde_json::to_string_pretty(&current).unwrap() + "\n").unwrap();
    }
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture present")).unwrap();
    // Platform libm differs in the last bits of sin, cos and atan2, so the
    // numbers agree to a tolerance, the same one the browser test applies.
    let mut differences = Vec::new();
    compare(&saved, &current, "$", &mut differences);
    assert!(
        differences.is_empty(),
        "regenerate the fixture if this change is meant:\n{}",
        differences.join("\n")
    );
    assert_eq!(preview.panels.len(), 4);
    assert!(preview
        .panels
        .iter()
        .all(|p| p.footprint.view_corners.is_some()));
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
