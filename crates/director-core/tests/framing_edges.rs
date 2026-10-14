//! Framing geometry at its edges: angles everywhere on the sphere, turned
//! panels with real gaps, shot readings with odd frames. Regressions from
//! the review of per-panel angles and shot panels.

use psf_guard_director_core::{
    framing::{
        camera_angle_at,
        measured::{median_geometry, FrameSample},
        plane_angle_at,
        shape::{shape_issues, shape_overlap},
        FramingRequest, Layout, LayoutIssueKind, PanelPlacement, PanelShape, PanelSize,
        TangentPlane,
    },
    visibility::IcrsPosition,
};

fn at(ra: f64, dec: f64) -> IcrsPosition {
    IcrsPosition {
        ra_degrees: ra,
        dec_degrees: dec,
    }
}

fn wrap(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

fn vec3(p: IcrsPosition) -> [f64; 3] {
    let (sa, ca) = p.ra_degrees.to_radians().sin_cos();
    let (sd, cd) = p.dec_degrees.to_radians().sin_cos();
    [cd * ca, cd * sa, sd]
}

/// Step `distance` degrees from `p` along position angle `pa`, on a great
/// circle.
fn step(p: IcrsPosition, pa: f64, distance: f64) -> IcrsPosition {
    let (sd, cd) = p.dec_degrees.to_radians().sin_cos();
    let (sa, ca) = p.ra_degrees.to_radians().sin_cos();
    let r = vec3(p);
    let east = [-sa, ca, 0.0];
    let north = [-sd * ca, -sd * sa, cd];
    let (s, c) = pa.to_radians().sin_cos();
    let t = [
        s * east[0] + c * north[0],
        s * east[1] + c * north[1],
        s * east[2] + c * north[2],
    ];
    let (sd2, cd2) = distance.to_radians().sin_cos();
    let v = [
        cd2 * r[0] + sd2 * t[0],
        cd2 * r[1] + sd2 * t[1],
        cd2 * r[2] + sd2 * t[2],
    ];
    at(
        v[1].atan2(v[0]).to_degrees().rem_euclid(360.0),
        v[2].clamp(-1.0, 1.0).asin().to_degrees(),
    )
}

/// The direction on the framing's plane a camera at `p` shooting at `pa`
/// draws its up along.
fn drawn_angle(plane: &TangentPlane, p: IcrsPosition, pa: f64) -> f64 {
    let a = plane.project(p).unwrap();
    let b = plane.project(step(p, pa, 1e-3)).unwrap();
    (b[0] - a[0]).atan2(b[1] - a[1]).to_degrees()
}

/// Near the poles, across 0h, at 30° reach and over every θ, each panel's
/// angle draws along θ, and the inverse reads it back to rounding.
#[test]
fn camera_angles_draw_along_theta_everywhere_and_read_back_to_rounding() {
    let centers = [
        at(38.2, 61.45),
        at(359.5, 40.0),
        at(0.2, 89.9),
        at(180.0, -89.9),
        at(10.0, 0.0),
    ];
    let offsets = [
        [0.0, 0.0],
        [3.0, 0.0],
        [-3.0, 2.0],
        [0.0, 0.5],
        [30.0, 30.0],
        [-30.0, 30.0],
        [30.0, -30.0],
    ];
    let (mut worst_draw, mut worst_trip): (f64, f64) = (0.0, 0.0);
    for center in centers {
        let plane = TangentPlane::at(center).unwrap();
        for offset in offsets {
            let p = plane.deproject(offset);
            let mut theta = -180.0;
            while theta <= 180.0 {
                let pa = camera_angle_at(center, p, theta);
                assert!((0.0..360.0).contains(&pa), "{pa}");
                worst_draw = worst_draw.max(wrap(drawn_angle(&plane, p, pa) - theta).abs());
                worst_trip = worst_trip.max(wrap(plane_angle_at(center, p, pa) - theta).abs());
                theta += 1.0;
            }
        }
    }
    assert!(worst_draw < 1e-4, "drawn {worst_draw}");
    assert!(worst_trip < 1e-7, "round trip {worst_trip}");
}

#[test]
fn north_leans_the_same_both_ways_across_ra_zero() {
    let a = camera_angle_at(at(359.5, 60.0), at(0.5, 60.0), 0.0);
    let b = camera_angle_at(at(0.5, 60.0), at(359.5, 60.0), 0.0);
    assert!((wrap(a) + wrap(b)).abs() < 1e-9);
    assert!(wrap(a) > 0.0, "east of the center leans positive");
}

/// A camera turned by hand to portrait while Target Scheduler's rotation
/// stays 0: the frames sit on the old fold boundary at ±90°, and still
/// agree on their own axis.
#[test]
fn frames_near_a_quarter_turn_from_the_plan_agree_on_their_own_axis() {
    let sample = |angle: f64| FrameSample {
        center: at(83.8, -5.4),
        position_angle_degrees: Some(angle),
        size: Some(PanelSize {
            width_degrees: 1.8,
            height_degrees: 1.2,
        }),
    };
    let samples: Vec<FrameSample> = [89.8, 90.2, 89.9, 90.1, 270.0].map(sample).to_vec();
    let (_, angle, _) = median_geometry(&samples, 0.0).unwrap();
    let angle = angle.unwrap();
    let off = {
        let d = (angle - 90.0).rem_euclid(180.0);
        d.min(180.0 - d)
    };
    assert!(off < 0.11, "{angle}");
    assert!((0.0..360.0).contains(&angle));
}

/// One wild solve among good ones, newest first, does not pick the plane
/// and drop the rest: the center is the good frames'.
#[test]
fn a_stray_solve_does_not_move_the_center() {
    let sample = |ra: f64, dec: f64| FrameSample {
        center: at(ra, dec),
        position_angle_degrees: Some(10.0),
        size: None,
    };
    let mut samples = vec![sample(200.0, -29.0)];
    for i in 0..9 {
        samples.push(sample(20.0 + 0.01 * i as f64, 30.0));
    }
    let (center, _, _) = median_geometry(&samples, 10.0).unwrap();
    assert!((center.ra_degrees - 20.04).abs() < 0.02, "{center:?}");
    assert!((center.dec_degrees - 30.0).abs() < 0.02, "{center:?}");
}

/// Two panels turned 30°, side by side along their own axis with a 10%
/// gap: their extents overlap on both camera axes but the outlines miss.
/// The gap is measured, not read as touching, at any minimum.
#[test]
fn turned_panels_with_a_real_gap_read_as_a_gap() {
    let t = 30.0_f64;
    let a = PanelShape::turned("a", [0.0, 0.0], 1.0, 1.0, t);
    let (s, c) = t.to_radians().sin_cos();
    let b = PanelShape::turned("b", [1.1 * c, 1.1 * s], 1.0, 1.0, t);
    let pair = shape_overlap(&a, &b).unwrap();
    assert!(pair.thickness < 0.0, "{pair:?}");
    // 0.1 of a side apart, over the turned square's reach along the axis.
    assert!((pair.thickness + 0.1 / (c + s)).abs() < 1e-9, "{pair:?}");
    for minimum in [0, 20] {
        let issues = shape_issues(&[a.clone(), b.clone()], minimum);
        assert_eq!(issues.len(), 1, "{minimum}: {issues:?}");
        assert_eq!(
            issues[0].kind,
            LayoutIssueKind::Gap,
            "{minimum}: {issues:?}"
        );
    }
}

/// The same through a framing at a 0% minimum.
#[test]
fn a_framing_shows_a_turned_gap_at_zero_minimum() {
    let (s, c) = 30f64.to_radians().sin_cos();
    let mut b = PanelPlacement::at("p2", 1.1 * c, 1.1 * s);
    b.angle_offset_degrees = 30.0;
    let mut a = PanelPlacement::at("r1c1", 0.0, 0.0);
    a.angle_offset_degrees = 30.0;
    let request = FramingRequest {
        center: at(38.2, 20.0),
        position_angle_degrees: 0.0,
        panel: PanelSize {
            width_degrees: 1.0,
            height_degrees: 1.0,
        },
        layout: Layout {
            min_overlap_percent: 0,
            panels: vec![a, b],
        },
        overlays: vec![],
        view: None,
    };
    let preview = request.preview().unwrap();
    assert_eq!(preview.issues.len(), 1, "{:?}", preview.issues);
    assert_eq!(preview.issues[0].kind, LayoutIssueKind::Gap);
}

/// Touching outlines still read as a 0% overlap, and panels whose extents
/// are apart give the same answer with or without clipping.
#[test]
fn touching_and_apart_panels_read_as_before() {
    let a = PanelShape::rectangle("a", [0.0, 0.0], 1.0, 1.0);
    let touching = PanelShape::rectangle("b", [1.0, 0.0], 1.0, 1.0);
    let pair = shape_overlap(&a, &touching).unwrap();
    assert_eq!(pair.thickness, 0.0);
    assert_eq!(pair.run, 1.0);
    let apart = PanelShape::rectangle("c", [1.25, 0.3], 1.0, 1.0);
    let pair = shape_overlap(&a, &apart).unwrap();
    assert!((pair.thickness + 0.25).abs() < 1e-12 && (pair.run - 0.7).abs() < 1e-12);
    assert!(shape_overlap(&a, &PanelShape::rectangle("d", [3.0, 3.0], 1.0, 1.0)).is_none());
}

/// A 16 by 16 mosaic's issues come in well under a frame's time: pairs
/// whose extents are apart are not clipped, and a gap is checked for cover
/// only against the panels that reach into it.
#[test]
fn a_large_mosaic_reads_its_issues_quickly() {
    let mut shapes = Vec::new();
    for row in 0..16 {
        for column in 0..16 {
            shapes.push(PanelShape::turned(
                format!("r{}c{}", row + 1, column + 1),
                [column as f64 * 0.8, row as f64 * 0.8],
                1.0,
                1.0,
                if (row + column) % 7 == 0 { 2.0 } else { 0.0 },
            ));
        }
    }
    // The fastest of several reads, so a busy machine does not fail it.
    let fastest = (0..7)
        .map(|_| {
            let started = std::time::Instant::now();
            shape_issues(&shapes, 15);
            started.elapsed()
        })
        .min()
        .unwrap();
    assert!(fastest.as_millis() < 40, "{fastest:?}");
}
