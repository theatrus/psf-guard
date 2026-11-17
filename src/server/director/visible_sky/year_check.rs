//! The year's fill-in held against the truth: every night of a year
//! measured, beside the one night a week the endpoint measures and the
//! nights it works out between them.

use super::*;
use psf_guard_director_core::visibility::HorizonPoint;

pub(super) struct Case {
    pub(super) name: &'static str,
    pub(super) latitude: f64,
    pub(super) longitude: f64,
    pub(super) horizon: Horizon,
    pub(super) minimum_degrees: f64,
}

fn point(azimuth_degrees: f64, altitude_degrees: f64) -> HorizonPoint {
    HorizonPoint {
        azimuth_degrees,
        altitude_degrees,
    }
}

/// The scratch site's horizon: 10° all round, 65° from azimuth 120° to 150°.
pub(super) fn wall() -> Horizon {
    Horizon::Custom {
        points: vec![
            point(0.0, 10.0),
            point(119.0, 10.0),
            point(120.0, 65.0),
            point(150.0, 65.0),
            point(151.0, 10.0),
            point(360.0, 10.0),
        ],
    }
}

/// A mast one degree wide, due south.
fn mast() -> Horizon {
    Horizon::Custom {
        points: vec![
            point(0.0, 5.0),
            point(179.5, 5.0),
            point(180.0, 70.0),
            point(181.0, 70.0),
            point(181.5, 5.0),
            point(360.0, 5.0),
        ],
    }
}

/// Jagged every 7°, with ends that differ at north.
fn jagged() -> Horizon {
    let mut points = vec![point(0.0, 12.0)];
    for azimuth in (7..360).step_by(7) {
        points.push(point(
            f64::from(azimuth),
            8.0 + f64::from(17 * azimuth % 31),
        ));
    }
    points.push(point(360.0, 25.0));
    Horizon::Custom { points }
}

pub(super) fn cases() -> Vec<Case> {
    let case = |name, latitude, longitude, horizon, minimum_degrees| Case {
        name,
        latitude,
        longitude,
        horizon,
        minimum_degrees,
    };
    vec![
        case("40° N, the wall", 40.0, -105.3, wall(), 20.0),
        case("52° N, flat", 52.0, 0.0, Horizon::FixedMinimum {}, 25.0),
        case("Tromsø, flat", 69.6, 18.9, Horizon::FixedMinimum {}, 20.0),
        case("Cape Town, the wall", -33.9, 18.5, wall(), 15.0),
        case("equator, jagged", 0.5, -78.5, jagged(), 10.0),
        case("40° N, a mast", 40.0, -105.3, mast(), 0.0),
    ]
}

/// What the comparison found.
#[derive(Debug)]
pub(super) struct Found {
    pub(super) nights: usize,
    /// Share of the sky in view every night and on any: true, then filled.
    pub(super) every: (f64, f64),
    pub(super) any: (f64, f64),
    /// Nodes the fill-in calls in view every night that are not, and nodes
    /// in view every night that it leaves out.
    pub(super) every_claimed: usize,
    pub(super) every_missed: usize,
    pub(super) any_claimed: usize,
    pub(super) any_missed: usize,
    /// The largest difference in a node's count, as a share of the nights,
    /// the mean difference over every node, and the largest count the
    /// fill-in gives beyond the true one.
    pub(super) worst_count: f64,
    pub(super) mean_count: f64,
    pub(super) worst_overcount: i32,
    /// Of the nodes wrongly called in view every night, the most nights
    /// one is out of view.
    pub(super) claimed_short: usize,
    /// The furthest any point of the filled every-night line lies from
    /// the true one, or the true one from it, in degrees; `None` when
    /// either has no line.
    pub(super) line_apart: Option<f64>,
    /// The same for the line through the measured nights alone.
    pub(super) measured_line_apart: Option<f64>,
    /// A month at a time, as a client adds one up for each week of the
    /// year: over every such month, the largest difference in a node's
    /// count of 31 nights, the mean difference in nights, the largest
    /// count beyond the true one, and the most nodes wrongly counted in
    /// view on every night of a month. The same four for the first month
    /// alone, which is the month Now measures night by night.
    pub(super) months: (i32, f64, i32, usize),
    pub(super) first_month: (i32, f64, i32, usize),
    /// The largest zigzag of each every-night line's declination within
    /// 7° of right ascension, in degrees: true, filled, measured alone.
    pub(super) wiggle: (f64, f64, f64),
}

fn share(seen: &[bool]) -> f64 {
    let (mut inside, mut whole) = (0.0, 0.0);
    for row in 0..ROWS {
        let weight = (-90.0 + row as f64 * GRID_DEGREES).to_radians().cos();
        let count = seen[row * COLS..(row + 1) * COLS]
            .iter()
            .filter(|seen| **seen)
            .count();
        inside += weight * count as f64;
        whole += weight * COLS as f64;
    }
    inside / whole
}

/// How far a point lies from a line's nearest stretch, in degrees, on the
/// flat sky around the point.
fn from_line(at: [f64; 2], line: &[[f64; 2]]) -> f64 {
    let squeeze = at[1].to_radians().cos().max(0.01);
    let local = |p: [f64; 2]| {
        let across = (p[0] - at[0] + 540.0).rem_euclid(360.0) - 180.0;
        (across * squeeze, p[1] - at[1])
    };
    line.windows(2)
        .map(|pair| {
            let (a, b) = (local(pair[0]), local(pair[1]));
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let length = dx * dx + dy * dy;
            let along = if length > 0.0 {
                (-(a.0 * dx + a.1 * dy) / length).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (a.0 + along * dx).hypot(a.1 + along * dy)
        })
        .fold(f64::MAX, f64::min)
}

/// The furthest any point of either set of lines lies from the other set.
fn apart(one: &[Vec<[f64; 2]>], other: &[Vec<[f64; 2]>]) -> Option<f64> {
    if one.is_empty() || other.is_empty() {
        return None;
    }
    let furthest = |from: &[Vec<[f64; 2]>], to: &[Vec<[f64; 2]>]| {
        from.iter()
            .flatten()
            .map(|at| {
                to.iter()
                    .map(|line| from_line(*at, line))
                    .fold(f64::MAX, f64::min)
            })
            .fold(0.0, f64::max)
    };
    Some(furthest(one, other).max(furthest(other, one)))
}

/// The largest zigzag of a line's declination within 7° of right
/// ascension: half of how much further it travels up and down than it
/// gets. A straight or steadily curving line has none; a tooth of height
/// `h` has `h`.
pub(super) fn wiggle(lines: &[Vec<[f64; 2]>]) -> f64 {
    let mut worst: f64 = 0.0;
    for line in lines {
        for start in 0..line.len() {
            let (mut travelled, mut end) = (0.0, start);
            let mut across = 0.0;
            while end + 1 < line.len() {
                let step = (line[end + 1][0] - line[end][0] + 540.0).rem_euclid(360.0) - 180.0;
                if (across + step).abs() > 7.0 {
                    break;
                }
                across += step;
                travelled += (line[end + 1][1] - line[end][1]).abs();
                end += 1;
            }
            let got = (line[end][1] - line[start][1]).abs();
            worst = worst.max((travelled - got) / 2.0);
        }
    }
    worst
}

/// Measure every night of `measured` weeks from `first` at a site, and
/// hold the fill-in from one night a week against it.
pub(super) fn compare(case: &Case, first: &str, measured: usize, needed: u32) -> Found {
    let site = Site {
        latitude_degrees: case.latitude,
        longitude_degrees: case.longitude,
        elevation_meters: 100.0,
    };
    let limits = AltitudeLimits {
        rig_minimum_degrees: case.minimum_degrees,
        project_minimum_degrees: -90.0,
        horizon_offset_degrees: 0.0,
        rig_maximum_degrees: 90.0,
        project_maximum_degrees: 90.0,
    };
    let nights = fill::nights_counted(measured);
    let first_noon = first
        .parse::<NightDate>()
        .unwrap()
        .span(site.longitude_degrees)
        .start_ms;
    let skies: Vec<NightSky> = (0..nights as u64)
        .into_par_iter()
        .map(|night| NightSky::new(site, first_noon + night * 86_400_000, STEP_MS).unwrap())
        .collect();
    let runs: Vec<DarkRuns<'_>> = skies
        .iter()
        .map(|sky| DarkRuns::new(sky, &case.horizon, limits, DARK_BELOW_DEGREES).unwrap())
        .collect();
    // The truth: every node on every night.
    let truth: Vec<Vec<bool>> = runs
        .par_iter()
        .map(|runs| {
            (0..ROWS)
                .flat_map(|row| {
                    let cols = if row == 0 || row == ROWS - 1 { 1 } else { COLS };
                    let mut seen: Vec<bool> = (0..cols)
                        .map(|col| runs.reaches(node(col, row), needed).unwrap())
                        .collect();
                    if cols == 1 {
                        seen = vec![seen[0]; COLS];
                    }
                    seen
                })
                .collect()
        })
        .collect();
    let true_counts: Vec<usize> = (0..COLS * ROWS)
        .map(|index| truth.iter().filter(|night| night[index]).count())
        .collect();
    let true_every: Vec<bool> = true_counts.iter().map(|count| *count == nights).collect();
    let true_any: Vec<bool> = true_counts.iter().map(|count| *count > 0).collect();
    // The estimate: one night a week, filled in.
    let sampled: Vec<usize> = (0..measured).map(|week| week * fill::EVERY).collect();
    let sampled_runs: Vec<DarkRuns<'_>> = sampled
        .iter()
        .map(|night| {
            DarkRuns::new(&skies[*night], &case.horizon, limits, DARK_BELOW_DEGREES).unwrap()
        })
        .collect();
    let grids: Vec<Grid> = sampled
        .iter()
        .map(|night| Arc::new(truth[*night].clone()))
        .collect();
    let slices: Vec<&[bool]> = grids.iter().map(|grid| grid.as_slice()).collect();
    let counts = fill::counts(&slices, COLS);
    let every: Vec<bool> = counts
        .iter()
        .map(|count| *count as usize == nights)
        .collect();
    let any: Vec<bool> = counts.iter().map(|count| *count > 0).collect();
    // The measured nights alone, as the endpoint drew them before.
    let alone: Vec<bool> = (0..COLS * ROWS)
        .map(|index| grids.iter().all(|grid| grid[index]))
        .collect();
    let lines = |region: Result<Region, Failure>| region.unwrap().lines;
    let true_line = lines(region(&true_every, &|at| {
        for runs in &runs {
            if !runs.reaches(at, needed)? {
                return Ok(false);
            }
        }
        Ok(true)
    }));
    let filled_line = lines(region(&every, &|at| {
        Ok(year_all_or_any(&sampled_runs, &grids, at, needed, true)?)
    }));
    let alone_line = lines(region(&alone, &|at| {
        for runs in &sampled_runs {
            if !runs.reaches(at, needed)? {
                return Ok(false);
            }
        }
        Ok(true)
    }));
    let differ = |filled: &[bool], truth: &[bool]| {
        (
            filled
                .iter()
                .zip(truth)
                .filter(|(f, t)| **f && !**t)
                .count(),
            filled
                .iter()
                .zip(truth)
                .filter(|(f, t)| !**f && **t)
                .count(),
        )
    };
    let (every_claimed, every_missed) = differ(&every, &true_every);
    let (any_claimed, any_missed) = differ(&any, &true_any);
    let gaps = counts
        .iter()
        .zip(&true_counts)
        .map(|(filled, truth)| i32::from(*filled) - *truth as i32);
    // A month at a time from each week of the year.
    let filled_nights: Vec<Vec<bool>> = (0..nights)
        .map(|night| {
            (0..COLS * ROWS)
                .map(|index| fill::night_seen(&slices, COLS, night, index))
                .collect()
        })
        .collect();
    let month = |first: usize| {
        let count = |from: &[Vec<bool>], index: usize| {
            from[first..first + 31]
                .iter()
                .filter(|night| night[index])
                .count() as i32
        };
        let (mut worst, mut sum, mut over, mut claimed) = (0, 0, 0, 0);
        for index in 0..COLS * ROWS {
            let (filled, truth) = (count(&filled_nights, index), count(&truth, index));
            worst = worst.max((filled - truth).abs());
            sum += (filled - truth).abs();
            over = over.max(filled - truth);
            claimed += usize::from(filled == 31 && truth < 31);
        }
        (worst, f64::from(sum) / (COLS * ROWS) as f64, over, claimed)
    };
    let first_month = if nights >= 31 {
        month(0)
    } else {
        (0, 0.0, 0, 0)
    };
    let months = (0..nights.saturating_sub(30))
        .step_by(fill::EVERY)
        .map(month)
        .fold((0, 0.0_f64, 0, 0), |all, one| {
            (
                all.0.max(one.0),
                all.1.max(one.1),
                all.2.max(one.2),
                all.3.max(one.3),
            )
        });
    Found {
        months,
        first_month,
        nights,
        every: (share(&true_every), share(&every)),
        any: (share(&true_any), share(&any)),
        every_claimed,
        every_missed,
        any_claimed,
        any_missed,
        worst_count: gaps.clone().map(|gap| gap.abs()).max().unwrap_or(0) as f64 / nights as f64,
        mean_count: gaps.clone().map(|gap| f64::from(gap.abs())).sum::<f64>()
            / (COLS * ROWS * nights) as f64,
        claimed_short: every
            .iter()
            .zip(&true_counts)
            .filter(|(claimed, _)| **claimed)
            .map(|(_, truth)| nights - truth)
            .max()
            .unwrap_or(0),
        worst_overcount: gaps.max().unwrap_or(0),
        line_apart: apart(&filled_line, &true_line),
        measured_line_apart: apart(&alone_line, &true_line),
        wiggle: (
            wiggle(&true_line),
            wiggle(&filled_line),
            wiggle(&alone_line),
        ),
    }
}

/// The whole year at the six sites: about twelve seconds on sixteen
/// cores, a minute or two on two.
/// `cargo test -p psf-guard --lib year_check -- --ignored --nocapture`.
#[test]
#[ignore]
fn the_year_filled_in_against_every_night_measured() {
    for case in cases() {
        let began = std::time::Instant::now();
        let found = compare(&case, "2026-11-16", 53, 6);
        eprintln!("{}: {found:?} in {:?}", case.name, began.elapsed());
        hold(case.name, &found);
        // Over a year no node is more than a fifth of the nights out.
        assert!(found.worst_count < 0.2, "{}: {found:?}", case.name);
    }
}

/// What the fill-in is held to, at any site and over any stretch.
fn hold(name: &str, found: &Found) {
    // It may leave a night out and hardly ever adds one: a node's count
    // is at most two nights over, and a handful of nodes at most are
    // called in view every night when they miss a night or two.
    assert!(found.worst_overcount <= 2, "{name}: {found:?}");
    assert!(found.every_claimed <= 4, "{name}: {found:?}");
    assert!(found.claimed_short <= 3, "{name}: {found:?}");
    assert_eq!(found.any_claimed, 0, "{name}: {found:?}");
    // What it leaves out is a strip a node wide along some of an edge: of
    // the 16,380 nodes, a few dozen.
    assert!(found.every_missed <= 80, "{name}: {found:?}");
    // A month added up from the weekly nights adds at most two nights to
    // a node.
    assert!(found.months.2 <= 2, "{name}: {found:?}");
    assert!(found.first_month.2 <= 2, "{name}: {found:?}");
    assert!(found.any_missed <= 80, "{name}: {found:?}");
    // Its shares of the sky are within half a percent of the true ones,
    // and never over for the sky in view every night by more than a
    // twentieth of a percent.
    assert!(
        (found.every.1 - found.every.0).abs() < 0.005,
        "{name}: {found:?}"
    );
    assert!(found.every.1 - found.every.0 < 0.0005, "{name}: {found:?}");
    assert!(
        (found.any.1 - found.any.0).abs() < 0.005,
        "{name}: {found:?}"
    );
    // A node's count is out by a third of a percent of the nights on
    // average. Where the dark comes and goes within weeks it is more: at
    // Tromsø 1.3%, all of it under, because the nights between a measured
    // night with no dark and the next one with some are all left out.
    assert!(found.mean_count < 0.015, "{name}: {found:?}");
    // The line is within about a cell of the true one, nearer than the
    // measured nights alone put it, and has no teeth: it wiggles less
    // than the measured nights' line and no more than the true one does.
    if let (Some(filled), Some(alone)) = (found.line_apart, found.measured_line_apart) {
        assert!(filled < 2.5, "{name}: {found:?}");
        assert!(filled < alone, "{name}: {found:?}");
        assert!(found.wiggle.1 < found.wiggle.2, "{name}: {found:?}");
        assert!(found.wiggle.1 <= found.wiggle.0 + 0.5, "{name}: {found:?}");
    }
}

/// Ten weeks at the scratch site, every night measured: run with the
/// rest, in a few seconds.
#[test]
fn ten_weeks_filled_in_against_every_night_measured() {
    let cases = cases();
    let found = compare(&cases[0], "2026-11-16", 11, 6);
    assert_eq!(found.nights, 71);
    eprintln!("{}: {found:?}", cases[0].name);
    hold(cases[0].name, &found);
    // There was a line to compare, and teeth to take out of it.
    assert!(found.line_apart.is_some());
    assert!(found.wiggle.2 > 1.0, "{found:?}");
}
