//! `psf-guard flat-groups`: measure a catalog's flat groups on a copy.
//!
//! The source catalog is opened read-only and copied into the work folder;
//! everything is measured and written there. Frames are read where the
//! copy says they are, after any `--map-prefix` rewrites, and never
//! written. The report lists each family's groups, why neighbouring groups
//! split, the rig-wide change events and the angle term, and with
//! `--lights` how every light's flats would be chosen.

use crate::calibration::flat_groups::{self, model::Book};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
struct SessionReport {
    night: String,
    filter: Option<String>,
    rotation: Option<f64>,
    frames: usize,
    first: String,
    /// Against the session before it in its family, percent.
    dust_vs_previous: Option<f64>,
    broad_vs_previous: Option<f64>,
}

#[derive(Debug, Serialize)]
struct FamilyReport {
    key: String,
    filter: Option<String>,
    rotation: Option<f64>,
    /// Its measured angle range: "100.1–104.4°", "72.6° & 82.2°".
    angles: String,
    groups: Vec<Vec<SessionReport>>,
    splits: Vec<String>,
}

#[derive(Debug, Serialize)]
struct EventReport {
    start: String,
    end: String,
    what: String,
    dust_pct: f64,
    broad_pct: f64,
    camera_side: bool,
    filters: Vec<String>,
    rotations: Vec<Option<f64>>,
    within_flat_run: bool,
}

#[derive(Debug, Serialize)]
struct LightReport {
    id: i64,
    target: String,
    night: String,
    filter: String,
    rotation: Option<f64>,
    flats: Vec<String>,
    /// The nights and recorded angles of the flats its master is built
    /// from: the coherent set nearest the light.
    master: Vec<String>,
    master_angles: Vec<f64>,
    basis: String,
    chosen_by: Option<String>,
    flagged: bool,
    /// The response family its flats come from, when one was chosen.
    family: Option<String>,
    /// How: `angle`, `prior`, `pixels` or `nearest`.
    family_by: Option<String>,
    /// Its flats' recorded angles.
    flat_angles: Vec<f64>,
}

#[derive(Debug, Serialize)]
struct Report {
    catalog: String,
    outcome: flat_groups::job::PassOutcome,
    families: Vec<FamilyReport>,
    events: Vec<EventReport>,
    angle_terms: std::collections::BTreeMap<String, flat_groups::model::AngleTerm>,
    /// Each rig's rotator angle epochs: runs of lights with one sky −
    /// mechanical offset.
    epochs: std::collections::BTreeMap<String, Vec<flat_groups::lights::Epoch>>,
    lights: Vec<LightReport>,
}

fn when(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn pct(value: f64) -> f64 {
    (value * 10_000.0).round() / 100.0
}

/// Run the command.
pub fn run(
    catalog: &Path,
    work: &Path,
    images: &[String],
    map_prefix: &[String],
    report_path: Option<&Path>,
    lights: bool,
    fresh: bool,
) -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("warn")),
        )
        .with_target(false)
        .try_init();
    std::fs::create_dir_all(work)?;
    let copy = work.join("catalog.sqlite");
    // A copy already there keeps what earlier runs measured; `--fresh`
    // starts again from the catalog.
    if fresh || !copy.is_file() {
        let _ = std::fs::remove_file(&copy);
        let source = Connection::open_with_flags(
            catalog,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening {} read-only", catalog.display()))?;
        source
            .backup(rusqlite::MAIN_DB, &copy, None)
            .with_context(|| format!("copying the catalog to {}", copy.display()))?;
    } else {
        println!("Reusing the copy in {}", copy.display());
    }
    let conn = Connection::open(&copy)?;
    for rule in map_prefix {
        let (from, to) = rule
            .split_once('=')
            .with_context(|| format!("--map-prefix wants FROM=TO, got {rule}"))?;
        let changed = conn.execute(
            "UPDATE psf_guard_calibration_frame
             SET source_path = ?2 || substr(source_path, length(?1) + 1)
             WHERE substr(source_path, 1, length(?1)) = ?1",
            rusqlite::params![from, to],
        )?;
        println!("{changed} calibration path(s) now under {to}");
    }
    crate::calibration::migrate_existing(&conn)?;
    flat_groups::store::add_tables(&conn)?;
    let dir = work.join(flat_groups::CACHE_CATEGORY);
    flat_groups::register_response_dir(&copy, dir.clone());
    let roots: Vec<&Path> = images.iter().map(Path::new).collect();
    let tree = if roots.is_empty() {
        None
    } else {
        Some(crate::directory_tree::DirectoryTree::build_multiple(
            &roots,
        )?)
    };
    let find_light = |name: &str| {
        tree.as_ref()
            .and_then(|tree| tree.find_file_first(name).cloned())
    };
    let keep_going = || true;
    let yield_now = || {};
    let mut last_stage = String::new();
    let mut progress = |at: &flat_groups::job::Progress<'_>| {
        if at.stage != last_stage || at.done == at.total {
            println!("{}: {}/{}", at.stage, at.done, at.total);
            last_stage = at.stage.to_string();
        }
    };
    let outcome = flat_groups::job::run(flat_groups::job::Pass {
        conn: &conn,
        dir: &dir,
        scope: flat_groups::job::Scope::everything(),
        keep_going: &keep_going,
        yield_now: &yield_now,
        progress: &mut progress,
        goes_first: &|_| false,
        find_light: &find_light,
        pool: None,
    })?;
    let book = flat_groups::load_book(&conn)?;
    let mut report = Report {
        catalog: catalog.display().to_string(),
        outcome,
        families: families(&book),
        events: book
            .events
            .iter()
            .map(|event| EventReport {
                start: when(event.start),
                end: when(event.end),
                what: flat_groups::describe_spots(&event.spots, event.dust, event.broad),
                dust_pct: pct(event.dust),
                broad_pct: pct(event.broad),
                camera_side: event.camera_side,
                filters: event.filters.iter().cloned().collect(),
                rotations: event.rotations.clone(),
                within_flat_run: event.within_flat_run,
            })
            .collect(),
        angle_terms: book
            .angle_terms
            .iter()
            .map(|(rig, term)| (rig.clone(), *term))
            .collect(),
        epochs: book
            .epochs
            .by_rig
            .iter()
            .map(|(rig, epochs)| (rig.clone(), epochs.clone()))
            .collect(),
        lights: Vec::new(),
    };
    if lights {
        report.lights = light_reports(&conn, &find_light)?;
    }
    print_summary(&report);
    if let Some(path) = report_path {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("Report written to {}", path.display());
    }
    Ok(())
}

fn families(book: &Book) -> Vec<FamilyReport> {
    book.families
        .iter()
        .map(|family| {
            let session = |index: usize, previous: Option<usize>| {
                let s = &book.sessions[index];
                let against = previous.and_then(|p| book.comparison(p, index));
                SessionReport {
                    night: s.row.night.clone(),
                    filter: s.row.filter.clone(),
                    rotation: s.row.rotation,
                    frames: s.row.frame_uuids.len(),
                    first: when(s.first),
                    dust_vs_previous: against.map(|c| pct(c.dust)),
                    broad_vs_previous: against.map(|c| pct(c.broad)),
                }
            };
            let mut previous: Option<usize> = None;
            let groups = family
                .groups
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .map(|index| {
                            let report = session(*index, previous);
                            previous = Some(*index);
                            report
                        })
                        .collect()
                })
                .collect();
            let splits = family
                .boundaries
                .iter()
                .map(|b| {
                    let boundary = &book.boundaries[*b];
                    format!(
                        "{} -> {}: dust {:?}%, broad {:?}%{}",
                        book.sessions[boundary.before].row.night,
                        book.sessions[boundary.after].row.night,
                        boundary.dust.map(pct),
                        boundary.broad.map(pct),
                        boundary
                            .drift
                            .map(|(a, b, d)| format!(
                                " (drift: {} vs {} at {d:.2}× the limit)",
                                book.sessions[a].row.night, book.sessions[b].row.night
                            ))
                            .unwrap_or_default()
                    )
                })
                .collect();
            FamilyReport {
                key: family.key.clone(),
                filter: family.filter.clone(),
                rotation: family.rotation,
                angles: book.angle_label(
                    book.families
                        .iter()
                        .position(|f| f.key == family.key)
                        .unwrap_or(0),
                ),
                groups,
                splits,
            }
        })
        .collect()
}

fn light_reports(
    conn: &Connection,
    find: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<Vec<LightReport>> {
    let mut statement = conn.prepare(
        "SELECT ai.Id, COALESCE(t.name, ''), ai.filtername, ai.metadata FROM acquiredimage ai
         LEFT JOIN target t ON t.Id = ai.targetId
         WHERE ai.gradingStatus IS NOT 2 ORDER BY ai.acquireddate",
    )?;
    let rows: Vec<(i64, String, String, String)> = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let boundary = crate::calibration::catalog_night_boundary(conn);
    let mut out = Vec::new();
    for (id, target, filter, metadata) in rows {
        let Some(name) = crate::utils::extract_filename(&metadata) else {
            continue;
        };
        let Some(path) = find(crate::utils::file_name_of(&name)) else {
            continue;
        };
        let meta = crate::commands::import::headers::read_frame_meta(&path);
        let selected = crate::calibration::select_for_light(conn, &meta)?;
        let mut nights: Vec<String> = selected
            .flat
            .iter()
            .filter_map(|f| f.captured_at)
            .map(|at| crate::calibration::night_of(at, boundary))
            .collect();
        nights.sort();
        nights.dedup();
        let delivered = crate::calibration::delivery_selection(selected.clone());
        let mut master: Vec<String> = delivered
            .flat
            .iter()
            .filter_map(|f| f.captured_at)
            .map(|at| crate::calibration::night_of(at, boundary))
            .collect();
        master.sort();
        master.dedup();
        let mut master_angles: Vec<f64> = delivered
            .flat
            .iter()
            .filter_map(|f| f.rotation)
            .map(|a| (a * 10.0).round() / 10.0)
            .collect();
        master_angles.sort_by(f64::total_cmp);
        master_angles.dedup();
        let mut flat_angles: Vec<f64> = selected
            .flat
            .iter()
            .filter_map(|f| f.rotation)
            .map(|a| (a * 10.0).round() / 10.0)
            .collect();
        flat_angles.sort_by(f64::total_cmp);
        flat_angles.dedup();
        let family = selected.flat_family.as_ref();
        let family_by = family
            .and_then(|family| serde_json::to_value(family).ok())
            .and_then(|value| value.get("by")?.as_str().map(str::to_string));
        let basis = serde_json::to_value(&selected.flat_basis)?
            .get("basis")
            .and_then(|b| b.as_str())
            .unwrap_or("nearest")
            .to_string();
        out.push(LightReport {
            id,
            target,
            night: meta
                .timestamp
                .map(|at| crate::calibration::night_of(at, boundary))
                .unwrap_or_default(),
            filter,
            rotation: meta.rotator_position,
            flats: nights,
            master,
            master_angles,
            chosen_by: [
                family.and_then(flat_groups::describe_family),
                flat_groups::describe_basis(&selected.flat_basis, &selected.rules),
            ]
            .into_iter()
            .flatten()
            .reduce(|a, b| format!("{a}. {b}")),
            flagged: selected.flat_basis.flags(),
            basis,
            family: family.map(|family| family.label.clone()),
            family_by,
            flat_angles,
        });
    }
    Ok(out)
}

fn print_summary(report: &Report) {
    println!(
        "\n{} sessions, {} measured this run, {} unreadable, {} comparisons, {} evidence rows, \
         {} family rows",
        report.outcome.sessions,
        report.outcome.measured,
        report.outcome.unreadable,
        report.outcome.compared,
        report.outcome.evidence,
        report.outcome.families
    );
    for family in &report.families {
        let sessions: usize = family.groups.iter().map(Vec::len).sum();
        println!(
            "{} at {}: {} session(s) in {} group(s)",
            family.filter.as_deref().unwrap_or("-"),
            family.angles,
            sessions,
            family.groups.len()
        );
        for split in &family.splits {
            println!("    split {split}");
        }
    }
    for event in &report.events {
        println!(
            "event {} – {}{}: {}, dust {:.2}%, broad {:.2}%, {} in {:?} at {:?}",
            event.start,
            event.end,
            if event.within_flat_run {
                " (within one flat run)"
            } else {
                ""
            },
            event.what,
            event.dust_pct,
            event.broad_pct,
            if event.camera_side {
                "camera side"
            } else {
                "one filter"
            },
            event.filters,
            event.rotations
        );
    }
    for (rig, term) in &report.angle_terms {
        println!(
            "angle term {rig}: {} pair(s), dust ≤ {:.2}%, broad ≤ {:.2}%",
            term.pairs,
            pct(term.dust),
            pct(term.broad)
        );
    }
    if !report.lights.is_empty() {
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for light in &report.lights {
            *counts.entry(light.basis.as_str()).or_default() += 1;
        }
        println!("lights by basis: {counts:?}");
        let mut families: std::collections::BTreeMap<&str, usize> = Default::default();
        for light in &report.lights {
            *families
                .entry(light.family_by.as_deref().unwrap_or("by angle"))
                .or_default() += 1;
        }
        println!("lights by family choice: {families:?}");
        let flagged = report.lights.iter().filter(|l| l.flagged).count();
        println!("lights flagged for their flats: {flagged}");
    }
}
