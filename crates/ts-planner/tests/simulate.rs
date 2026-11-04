// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use psf_guard_director_core::moon::MoonPolicy;
use psf_guard_director_core::visibility::{Horizon, IcrsPosition, Site};
use psf_guard_director_core::windows::Interval;
use psf_guard_ts_planner::snapshot::*;
use psf_guard_ts_planner::*;

const HOUR: u64 = 3_600_000;
/// Local mean noon at Los Angeles on 2026-09-24, about 19:53 UTC.
const NOON: u64 = 1_790_279_640_000;

fn site() -> Site {
    Site {
        latitude_degrees: 34.2,
        longitude_degrees: -118.3,
        elevation_meters: 400.0,
    }
}

fn template(id: i64, filter: &str) -> ExposureTemplate {
    ExposureTemplate {
        id,
        name: filter.into(),
        filter_name: filter.into(),
        gain: None,
        offset: None,
        binning: None,
        readout_mode: None,
        twilight_level: TwilightLevel::Nighttime,
        twilight_offset_minutes: 0,
        moon: MoonPolicy::default(),
        maximum_humidity: 0.0,
        default_exposure_seconds: 60.0,
        dither_every: -1,
    }
}

fn plan(id: i64, template_id: i64, desired: u32) -> ExposurePlan {
    ExposurePlan {
        id,
        guid: None,
        template_id,
        exposure_seconds: 300.0,
        desired,
        acquired: 0,
        accepted: 0,
        enabled: true,
    }
}

fn target(
    id: i64,
    name: &str,
    ra_degrees: f64,
    dec_degrees: f64,
    plans: Vec<ExposurePlan>,
) -> Target {
    Target {
        id,
        guid: None,
        name: name.into(),
        active: true,
        position: IcrsPosition {
            ra_degrees,
            dec_degrees,
        },
        rotation_degrees: 0.0,
        roi_percent: 100.0,
        exposure_plans: plans,
        override_order: vec![],
        filter_cadence: vec![],
    }
}

fn project(id: i64, name: &str, targets: Vec<Target>) -> Project {
    Project {
        id,
        guid: None,
        name: name.into(),
        state: ProjectState::Active,
        priority: ProjectPriority::Normal,
        minimum_time_minutes: 30,
        minimum_altitude_degrees: 25.0,
        maximum_altitude_degrees: 0.0,
        use_custom_horizon: false,
        horizon_offset_degrees: 0.0,
        meridian_window_minutes: 0,
        filter_switch_frequency: 0,
        dither_every: 0,
        enable_grader: false,
        is_mosaic: false,
        smart_exposure_order: false,
        rule_weights: vec![],
        targets,
    }
}

fn snapshot(projects: Vec<Project>, templates: Vec<ExposureTemplate>) -> Snapshot {
    Snapshot {
        profile_id: "profile-a".into(),
        preferences: Preferences::default(),
        templates,
        projects,
    }
}

fn nights(count: u64) -> Vec<Interval> {
    (0..count)
        .map(|i| Interval {
            start_ms: NOON + i * 24 * HOUR,
            end_ms: NOON + (i + 1) * 24 * HOUR,
        })
        .collect()
}

fn run(snapshot: &Snapshot, count: u64, available: Option<&[Interval]>, mode: Mode) -> Simulation {
    simulate(&Request {
        snapshot,
        profile: NinaProfile {
            site: site(),
            horizon: Horizon::FixedMinimum {},
            pause_before_meridian_minutes: 0.0,
            minutes_after_meridian: 0.0,
        },
        // Pacific daylight time.
        clock: Clock {
            utc_offset_ms: -7 * HOUR as i64,
        },
        nights: nights(count),
        start_ms: NOON,
        available,
        mode,
    })
}

fn heart() -> Snapshot {
    snapshot(
        vec![project(
            1,
            "Heart",
            vec![target(10, "IC 1805", 38.2, 61.45, vec![plan(100, 1, 500)])],
        )],
        vec![template(1, "Ha")],
    )
}

#[test]
fn a_lone_target_is_shot_from_astronomical_dark_to_dawn_in_its_minimum_time_stints() {
    let simulation = run(&heart(), 1, None, Mode::Preview);
    let night = &simulation.nights[0];
    assert!(!night.blocks.is_empty(), "{night:?}");
    let first = &night.blocks[0];
    let last = night.blocks.last().unwrap();
    // Nighttime is Sun below −18°: after about 03:10 UTC, before about 12:30 UTC.
    assert!(
        first.start_ms > NOON + 6 * HOUR && first.start_ms < NOON + 8 * HOUR,
        "{}",
        first.start_ms
    );
    assert!(
        last.end_ms > NOON + 16 * HOUR && last.end_ms < NOON + 18 * HOUR,
        "{}",
        last.end_ms
    );
    assert!(first.slew);
    assert_eq!(first.runs[0].filter, "Ha");
    // The preview counts exposure time only: blocks are whole 300 s frames.
    let frames: u32 = night
        .blocks
        .iter()
        .flat_map(|b| &b.runs)
        .map(|r| r.frames)
        .sum();
    assert_eq!(frames, night.frames);
    assert!(frames > 90 && frames < 120, "{frames}");
    assert_eq!(night.gaps.first().unwrap().reason, GapReason::Twilight);
    assert_eq!(night.gaps.last().unwrap().reason, GapReason::Twilight);
    let progress = &simulation.progress[0];
    assert_eq!(progress.planned, vec![frames]);
    assert_eq!(progress.completes_on, None);
}

#[test]
fn a_finished_plan_ends_the_night_complete_and_carries_into_the_next() {
    let mut owed = heart();
    owed.projects[0].targets[0].exposure_plans[0].desired = 120;
    let simulation = run(&owed, 2, None, Mode::Preview);
    let planned = &simulation.progress[0].planned;
    // 125 % of 120 frames with the grader off.
    assert_eq!(planned.iter().sum::<u32>(), 150, "{planned:?}");
    assert!(planned[0] > 0 && planned[1] > 0);
    assert_eq!(simulation.progress[0].completes_on, Some(1));
    assert_eq!(
        simulation.nights[1].gaps.last().unwrap().reason,
        GapReason::Complete
    );
}

#[test]
fn estimate_mode_adds_overheads_and_counts_frames_at_the_acceptance_ratio() {
    let mut graded = heart();
    graded.projects[0].enable_grader = true;
    graded.projects[0].targets[0].exposure_plans[0].desired = 40;
    let preview = run(&graded, 1, None, Mode::Preview);
    let estimate = Estimate {
        acceptance: [(100, 0.5)].into_iter().collect(),
        ..Estimate::default()
    };
    let live = run(&graded, 1, None, Mode::Estimate(estimate));
    // The preview accepts every frame: 40 frames finish the plan.
    assert_eq!(preview.nights[0].frames, 40);
    // At half accepted, 80 frames are owed and the night has room for fewer.
    let frames = live.nights[0].frames;
    assert!(frames > 40 && frames <= 80, "{frames}");
    let block = &live.nights[0].blocks[0];
    let run = &block.runs[0];
    // Each frame takes its 300 s and 15 s to save.
    let per_frame = (run.end_ms - run.start_ms) as f64 / run.frames as f64 / 1000.0;
    assert!(per_frame > 314.0 && per_frame < 330.0, "{per_frame}");
}

#[test]
fn the_switch_penalty_holds_a_target_and_priority_wins_without_it() {
    let mut two = snapshot(
        vec![
            project(
                1,
                "Heart",
                vec![target(10, "IC 1805", 38.2, 61.45, vec![plan(100, 1, 500)])],
            ),
            project(
                2,
                "Soul",
                vec![target(20, "IC 1848", 42.8, 60.4, vec![plan(200, 1, 500)])],
            ),
        ],
        vec![template(1, "Ha")],
    );
    two.projects[1].priority = ProjectPriority::High;
    // The Heart clears 25° first and starts alone; once both are up, staying
    // scores 67 against the Soul's 50 for priority.
    let simulation = run(&two, 1, None, Mode::Preview);
    let night = &simulation.nights[0];
    assert_eq!(night.blocks[0].target.target_name, "IC 1805");
    assert_eq!(night.blocks[0].decision.kind, DecisionKind::Only);
    let soul = night.blocks[0]
        .decision
        .candidates
        .iter()
        .find(|c| c.target.target_name == "IC 1848")
        .unwrap();
    assert_eq!(soul.rejected, Some(TargetReason::NotYetVisible));
    assert!(soul.could_start_ms.unwrap() > night.blocks[0].start_ms);
    assert!(night
        .blocks
        .iter()
        .all(|b| b.target.target_name == "IC 1805"));
    // With the penalty off, the Soul takes over after the Heart's 30 minutes.
    for project in &mut two.projects {
        project.rule_weights = vec![RuleWeight {
            name: "Target Switch Penalty".into(),
            weight: 0.0,
        }];
    }
    let simulation = run(&two, 1, None, Mode::Preview);
    let night = &simulation.nights[0];
    let soul = night
        .blocks
        .iter()
        .find(|b| b.target.target_name == "IC 1848")
        .expect("the Soul is shot");
    assert!(soul.slew);
    assert!(soul.start_ms - night.blocks[0].start_ms >= 30 * 60_000);
    assert_eq!(soul.decision.kind, DecisionKind::Scored);
    let heart = soul
        .decision
        .candidates
        .iter()
        .find(|c| c.target.target_name == "IC 1805")
        .unwrap();
    assert_eq!(heart.rejected, Some(TargetReason::LowerScore));
    let scored = soul
        .decision
        .candidates
        .iter()
        .find(|c| c.target.target_name == "IC 1848")
        .unwrap();
    let rules = &scored.score.as_ref().unwrap().rules;
    let priority = rules
        .iter()
        .find(|r| r.rule == Rule::ProjectPriority)
        .unwrap();
    assert_eq!((priority.raw, priority.weight), (1.0, 50.0));
    assert!(rules.iter().all(|r| r.rule != Rule::TargetSwitchPenalty));
}

#[test]
fn availability_keeps_the_night_inside_its_clear_windows() {
    let window = [Interval {
        start_ms: NOON + 10 * HOUR,
        end_ms: NOON + 12 * HOUR,
    }];
    let simulation = run(&heart(), 1, Some(&window), Mode::Preview);
    let night = &simulation.nights[0];
    assert!(night
        .blocks
        .iter()
        .all(|b| b.start_ms >= window[0].start_ms && b.end_ms <= window[0].end_ms));
    assert!(night.frames >= 22 && night.frames <= 24, "{}", night.frames);
    assert!(night
        .gaps
        .iter()
        .any(|g| g.reason == GapReason::Unavailable));
}

#[test]
fn the_basic_cadence_alternates_filters_within_one_block() {
    let mut lrgb = snapshot(
        vec![project(
            1,
            "Heart",
            vec![target(
                10,
                "IC 1805",
                38.2,
                61.45,
                vec![plan(100, 1, 100), plan(101, 2, 100)],
            )],
        )],
        vec![template(1, "L"), template(2, "R")],
    );
    lrgb.projects[0].filter_switch_frequency = 2;
    lrgb.projects[0].dither_every = 1;
    let simulation = run(&lrgb, 1, None, Mode::Preview);
    let block = &simulation.nights[0].blocks[0];
    let filters: Vec<(&str, u32)> = block
        .runs
        .iter()
        .take(4)
        .map(|r| (r.filter.as_str(), r.frames))
        .collect();
    assert_eq!(filters, [("L", 2), ("R", 2), ("L", 2), ("R", 2)]);
    // Dither every frame: before the second use of a filter since the last dither.
    assert!(block.runs[1].dithers >= 1, "{:?}", block.runs[1]);
}

#[test]
fn a_meridian_window_keeps_the_target_to_the_hours_around_transit() {
    let mut windowed = heart();
    windowed.projects[0].meridian_window_minutes = 60;
    windowed.projects[0].minimum_time_minutes = 10;
    let simulation = run(&windowed, 1, None, Mode::Preview);
    let night = &simulation.nights[0];
    let start = night.blocks.first().unwrap().start_ms;
    let end = night.blocks.last().unwrap().end_ms;
    assert!(
        end - start <= 2 * HOUR + 10 * 60_000,
        "{}",
        (end - start) as f64 / HOUR as f64
    );
    assert!(night
        .gaps
        .iter()
        .any(|g| g.reason == GapReason::Meridian || g.reason == GapReason::BelowLimits));
}

#[test]
fn a_strict_moon_rule_on_a_full_moon_night_waits_for_the_moon() {
    let mut moonlit = heart();
    moonlit.templates[0].moon = MoonPolicy {
        enabled: true,
        separation_degrees: 120.0,
        width_days: 14.0,
        ..MoonPolicy::default()
    };
    let open = run(&heart(), 1, None, Mode::Preview).nights[0].frames;
    let simulation = run(&moonlit, 1, None, Mode::Preview);
    let night = &simulation.nights[0];
    assert!(night.frames < open, "{} of {open}", night.frames);
    assert!(
        night.gaps.iter().any(|g| g.reason == GapReason::Moon),
        "{:?}",
        night.gaps
    );
}

#[test]
fn each_run_carries_the_moon_over_the_run() {
    let simulation = run(&heart(), 1, None, Mode::Preview);
    let runs: Vec<_> = simulation.nights[0]
        .blocks
        .iter()
        .flat_map(|block| &block.runs)
        .collect();
    assert!(!runs.is_empty());
    for run in runs {
        let moon = run.moon.unwrap_or_else(|| panic!("{run:?}"));
        assert!(
            (-90.0..=90.0).contains(&moon.max_altitude_degrees),
            "{moon:?}"
        );
        assert!(
            (0.0..=180.0).contains(&moon.min_separation_degrees),
            "{moon:?}"
        );
    }
}

#[test]
fn a_run_s_moon_samples_its_ends_and_each_quarter_hour_between() {
    let minute = 60_000;
    assert_eq!(
        RunMoon::sample_times(7 * minute, 40 * minute),
        vec![7 * minute, 15 * minute, 30 * minute, 40 * minute]
    );
    assert_eq!(
        RunMoon::sample_times(15 * minute, 30 * minute),
        vec![15 * minute, 30 * minute]
    );
    let moon = RunMoon::over([(-4.0, 50.0), (2.5, 41.0), (f64::NAN, 1.0), (1.0, 44.0)]).unwrap();
    assert_eq!(moon.max_altitude_degrees, 2.5);
    assert_eq!(moon.min_separation_degrees, 41.0);
    assert_eq!(RunMoon::over([]), None);
}

/// Run with `cargo test --release -p psf-guard-ts-planner --test simulate timing -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing_thirty_targets_over_a_month() {
    let projects = (0..30)
        .map(|i| {
            let ra = (i as f64 * 12.0) % 360.0;
            let dec = -20.0 + (i as f64 * 7.0) % 80.0;
            project(
                i + 1,
                &format!("P{i}"),
                vec![target(
                    100 + i,
                    &format!("T{i}"),
                    ra,
                    dec,
                    vec![plan(1000 + i * 2, 1, 400), plan(1001 + i * 2, 2, 400)],
                )],
            )
        })
        .collect();
    let mut many = snapshot(projects, vec![template(1, "Ha"), template(2, "OIII")]);
    for p in &mut many.projects {
        p.filter_switch_frequency = 3;
    }
    many.templates[1].moon = MoonPolicy {
        enabled: true,
        separation_degrees: 60.0,
        width_days: 7.0,
        ..MoonPolicy::default()
    };
    let started = std::time::Instant::now();
    let simulation = run(&many, 31, None, Mode::Estimate(Estimate::default()));
    let frames: u32 = simulation.nights.iter().map(|n| n.frames).sum();
    println!("estimate: {frames} frames in {:?}", started.elapsed());
    let started = std::time::Instant::now();
    let simulation = run(&many, 31, None, Mode::Preview);
    let frames: u32 = simulation.nights.iter().map(|n| n.frames).sum();
    println!("preview: {frames} frames in {:?}", started.elapsed());
}
