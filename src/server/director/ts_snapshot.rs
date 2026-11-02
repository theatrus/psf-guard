//! A read-only snapshot of what Target Scheduler plans from on one rig: the
//! projects, targets, exposure plans, templates, rule weights, cadences and
//! profile preferences of the database's N.I.N.A. profile. Night plans read
//! it for a Target Scheduler rig's summaries and schedule. Nothing here
//! writes to the database.
//!
//! Columns an older schema lacks read as Target Scheduler's defaults, as the
//! vendored migrations in `src/ts_schema` add them.

use psf_guard_director_core::{moon::MoonPolicy, visibility::IcrsPosition};
use psf_guard_ts_planner::snapshot::{
    CadenceItem, ExposurePlan, ExposureTemplate, OrderItem, Preferences, Project, ProjectPriority,
    ProjectState, RuleWeight, Snapshot, Target, TwilightLevel,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// A snapshot and what was left out of it, and why.
pub(super) struct ReadSnapshot {
    pub(super) snapshot: Snapshot,
    pub(super) warnings: Vec<String>,
}

/// Read the database at `path` for its N.I.N.A. profile: the profile that
/// owns most projects, as activation chooses it. `None` when the database
/// has no profile at all.
pub(super) fn read(path: &str) -> rusqlite::Result<Option<ReadSnapshot>> {
    let mut connection = crate::server::database_context::open_scheduler_connection_with_flags(
        Path::new(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    // One read transaction, so Target Scheduler writing counts or a project
    // between the queries cannot leave the snapshot half old, half new.
    let snapshot = connection.transaction()?;
    let Some(profile_id) = super::activation::resolve_profile(&snapshot)? else {
        return Ok(None);
    };
    let read = read_profile(&snapshot, &profile_id)?;
    snapshot.commit()?;
    Ok(Some(read))
}

struct Columns(HashSet<String>);

impl Columns {
    fn of(connection: &Connection, table: &str) -> rusqlite::Result<Self> {
        let names = connection
            .prepare(&format!("PRAGMA table_info({table})"))?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self(
            names.into_iter().map(|n| n.to_ascii_lowercase()).collect(),
        ))
    }

    fn exists(&self) -> bool {
        !self.0.is_empty()
    }

    /// `COALESCE(alias.column, default)`, or the default when the schema has
    /// no such column.
    fn or(&self, alias: &str, column: &str, default: &str) -> String {
        if self.0.contains(&column.to_ascii_lowercase()) {
            format!("COALESCE({alias}.\"{column}\", {default})")
        } else {
            default.to_owned()
        }
    }
}

fn read_profile(connection: &Connection, profile_id: &str) -> rusqlite::Result<ReadSnapshot> {
    let mut warnings = Vec::new();
    let others: i64 = connection.query_row(
        "SELECT COUNT(*) FROM project WHERE state=1 AND IFNULL(profileId,'')<>?1",
        [profile_id],
        |row| row.get(0),
    )?;
    if others > 0 {
        warnings.push(format!(
            "{others} active project{} of another N.I.N.A. profile {} left out; Target Scheduler plans one profile at a time.",
            if others == 1 { "" } else { "s" },
            if others == 1 { "is" } else { "are" },
        ));
    }
    let preferences = read_preferences(connection, profile_id)?;
    let (templates, unusable) = read_templates(connection, profile_id)?;
    let mut projects = read_projects(connection, profile_id)?;
    let weights = read_rule_weights(connection, profile_id)?;
    let mut targets = read_targets(connection, profile_id, &mut warnings)?;
    let mut plans = read_plans(connection, profile_id)?;
    let mut orders = read_items(connection, profile_id, "overrideexposureorderitem", false)?;
    let mut cadences = read_items(connection, profile_id, "filtercadenceitem", true)?;
    for (target_id, target) in targets.iter_mut() {
        let mut found = plans.remove(target_id).unwrap_or_default();
        let before = found.len();
        found.retain(|plan| !unusable.contains(&plan.template_id));
        if found.len() < before {
            warnings.push(format!(
                "{}: exposure plans whose template has unusable Moon avoidance settings are left out.",
                target.1.name
            ));
        }
        target.1.exposure_plans = found;
        target.1.override_order = orders
            .remove(target_id)
            .unwrap_or_default()
            .into_iter()
            .map(|item| OrderItem {
                order: item.order,
                action: item.action,
                reference_index: item.reference_index,
            })
            .collect();
        target.1.filter_cadence = cadences
            .remove(target_id)
            .unwrap_or_default()
            .into_iter()
            .map(|item| CadenceItem {
                order: item.order,
                next: item.next,
                action: item.action,
                reference_index: item.reference_index,
            })
            .collect();
    }
    for project in &mut projects {
        project.rule_weights = weights.get(&project.id).cloned().unwrap_or_default();
    }
    let mut by_project: BTreeMap<i64, Vec<Target>> = BTreeMap::new();
    for (_, (project_id, target)) in targets {
        by_project.entry(project_id).or_default().push(target);
    }
    for project in &mut projects {
        project.targets = by_project.remove(&project.id).unwrap_or_default();
    }
    Ok(ReadSnapshot {
        snapshot: Snapshot {
            profile_id: profile_id.to_owned(),
            preferences,
            templates,
            projects,
        },
        warnings,
    })
}

fn read_preferences(connection: &Connection, profile_id: &str) -> rusqlite::Result<Preferences> {
    let columns = Columns::of(connection, "profilepreference")?;
    let defaults = Preferences::default();
    if !columns.exists() {
        return Ok(defaults);
    }
    let sql = format!(
        "SELECT {}, {}, {}, {} FROM profilepreference p WHERE p.profileId=?1 ORDER BY p.Id LIMIT 1",
        columns.or("p", "enableSlewCenter", "1"),
        columns.or("p", "parkonwait", "0"),
        columns.or("p", "exposurethrottle", "125"),
        columns.or("p", "delayGrading", "80"),
    );
    Ok(connection
        .query_row(&sql, [profile_id], |row| {
            Ok(Preferences {
                enable_slew_center: row.get::<_, i64>(0)? != 0,
                park_on_wait: row.get::<_, i64>(1)? != 0,
                exposure_throttle_percent: row.get(2)?,
                delay_grading_percent: row.get(3)?,
            })
        })
        .optional()?
        .unwrap_or(defaults))
}

/// The profile's templates, and the ids of those whose Moon settings no
/// planner can use.
fn read_templates(
    connection: &Connection,
    profile_id: &str,
) -> rusqlite::Result<(Vec<ExposureTemplate>, HashSet<i64>)> {
    let c = Columns::of(connection, "exposuretemplate")?;
    let sql = format!(
        "SELECT t.Id, t.name, t.filtername, t.gain, t.offset, t.bin, t.readoutmode,
                {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}
         FROM exposuretemplate t WHERE t.profileId=?1 ORDER BY t.Id",
        c.or("t", "twilightlevel", "0"),
        c.or("t", "minutesOffset", "0"),
        c.or("t", "moonavoidanceenabled", "0"),
        c.or("t", "moonavoidanceseparation", "60"),
        c.or("t", "moonavoidancewidth", "7"),
        c.or("t", "moonrelaxscale", "0"),
        c.or("t", "moonrelaxminaltitude", "-15"),
        c.or("t", "moonrelaxmaxaltitude", "5"),
        c.or("t", "moondownenabled", "0"),
        c.or("t", "maximumhumidity", "0"),
        c.or("t", "defaultexposure", "60"),
        c.or("t", "ditherevery", "-1"),
    );
    let mut unusable = HashSet::new();
    let mut templates = Vec::new();
    let rows = connection
        .prepare(&sql)?
        .query_map([profile_id], |row| {
            let mut moon = MoonPolicy {
                enabled: row.get::<_, i64>(9)? != 0,
                separation_degrees: row.get(10)?,
                width_days: row.get(11)?,
                relax_degrees_per_degree: row.get(12)?,
                relax_min_altitude_degrees: row.get(13)?,
                relax_max_altitude_degrees: row.get(14)?,
                moon_down: row.get::<_, i64>(15)? != 0,
            };
            // As the program pull reads them: with avoidance off, values
            // out of range stand for nothing.
            if !moon.enabled && moon.validate().is_err() {
                moon = MoonPolicy {
                    moon_down: moon.moon_down,
                    ..MoonPolicy::default()
                };
            }
            let default_exposure: f64 = row.get(17)?;
            Ok(ExposureTemplate {
                id: row.get(0)?,
                name: row.get(1)?,
                filter_name: row.get(2)?,
                gain: row.get(3)?,
                offset: row.get(4)?,
                binning: row.get(5)?,
                readout_mode: row.get(6)?,
                twilight_level: TwilightLevel::from_column(row.get(7)?),
                twilight_offset_minutes: row.get(8)?,
                moon,
                maximum_humidity: row.get(16)?,
                default_exposure_seconds: if default_exposure > 0.0 {
                    default_exposure
                } else {
                    60.0
                },
                dither_every: row.get(18)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for template in rows {
        if template.moon.validate().is_err() {
            unusable.insert(template.id);
        } else {
            templates.push(template);
        }
    }
    Ok((templates, unusable))
}

fn read_projects(connection: &Connection, profile_id: &str) -> rusqlite::Result<Vec<Project>> {
    let c = Columns::of(connection, "project")?;
    let sql = format!(
        "SELECT p.Id, {}, p.name, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}
         FROM project p WHERE p.profileId=?1 ORDER BY p.Id",
        c.or("p", "guid", "NULL"),
        c.or("p", "state", "0"),
        c.or("p", "priority", "1"),
        c.or("p", "minimumtime", "30"),
        c.or("p", "minimumaltitude", "0"),
        c.or("p", "maximumAltitude", "0"),
        c.or("p", "usecustomhorizon", "0"),
        c.or("p", "horizonoffset", "0"),
        c.or("p", "meridianwindow", "0"),
        c.or("p", "filterswitchfrequency", "0"),
        c.or("p", "ditherevery", "0"),
        c.or("p", "enablegrader", "1"),
        c.or("p", "isMosaic", "0"),
        c.or("p", "smartexposureorder", "0"),
    );
    let count = |value: i64| u32::try_from(value.max(0)).unwrap_or(u32::MAX);
    connection
        .prepare(&sql)?
        .query_map([profile_id], |row| {
            Ok(Project {
                id: row.get(0)?,
                guid: row.get(1)?,
                name: row.get(2)?,
                state: ProjectState::from_column(row.get(3)?),
                priority: ProjectPriority::from_column(row.get(4)?),
                minimum_time_minutes: count(row.get(5)?),
                minimum_altitude_degrees: row.get(6)?,
                maximum_altitude_degrees: row.get(7)?,
                use_custom_horizon: row.get::<_, i64>(8)? != 0,
                horizon_offset_degrees: row.get(9)?,
                meridian_window_minutes: count(row.get(10)?),
                filter_switch_frequency: count(row.get(11)?),
                dither_every: count(row.get(12)?),
                enable_grader: row.get::<_, i64>(13)? != 0,
                is_mosaic: row.get::<_, i64>(14)? != 0,
                smart_exposure_order: row.get::<_, i64>(15)? != 0,
                rule_weights: Vec::new(),
                targets: Vec::new(),
            })
        })?
        .collect()
}

fn read_rule_weights(
    connection: &Connection,
    profile_id: &str,
) -> rusqlite::Result<BTreeMap<i64, Vec<RuleWeight>>> {
    if !Columns::of(connection, "ruleweight")?.exists() {
        return Ok(BTreeMap::new());
    }
    let mut found: BTreeMap<i64, Vec<RuleWeight>> = BTreeMap::new();
    let rows = connection
        .prepare(
            "SELECT r.projectid, r.name, r.weight FROM ruleweight r
             JOIN project p ON p.Id=r.projectid WHERE p.profileId=?1 ORDER BY r.Id",
        )?
        .query_map([profile_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                RuleWeight {
                    name: row.get(1)?,
                    weight: row.get(2)?,
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (project, weight) in rows {
        found.entry(project).or_default().push(weight);
    }
    Ok(found)
}

/// Targets by id, each with its project's id.
fn read_targets(
    connection: &Connection,
    profile_id: &str,
    warnings: &mut Vec<String>,
) -> rusqlite::Result<BTreeMap<i64, (i64, Target)>> {
    let c = Columns::of(connection, "target")?;
    let sql = format!(
        "SELECT t.Id, {}, t.name, t.active, t.ra, t.dec, {}, {}, t.projectid
         FROM target t JOIN project p ON p.Id=t.projectid WHERE p.profileId=?1 ORDER BY t.Id",
        c.or("t", "guid", "NULL"),
        c.or("t", "rotation", "0"),
        c.or("t", "roi", "100"),
    );
    let rows = connection
        .prepare(&sql)?
        .query_map([profile_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? != 0,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut targets = BTreeMap::new();
    for (id, guid, name, active, ra, dec, rotation, roi, project) in rows {
        let position = ra.zip(dec).filter(|(ra, dec)| {
            ra.is_finite()
                && dec.is_finite()
                && (0.0..24.0).contains(ra)
                && (-90.0..=90.0).contains(dec)
        });
        let Some((ra_hours, dec_degrees)) = position else {
            if active {
                warnings.push(format!("{name}: no usable coordinates, so it is left out."));
            }
            continue;
        };
        targets.insert(
            id,
            (
                project,
                Target {
                    id,
                    guid,
                    name,
                    active,
                    position: IcrsPosition {
                        ra_degrees: (ra_hours * 15.0).rem_euclid(360.0),
                        dec_degrees,
                    },
                    rotation_degrees: rotation,
                    roi_percent: roi,
                    exposure_plans: Vec::new(),
                    override_order: Vec::new(),
                    filter_cadence: Vec::new(),
                },
            ),
        );
    }
    Ok(targets)
}

fn read_plans(
    connection: &Connection,
    profile_id: &str,
) -> rusqlite::Result<BTreeMap<i64, Vec<ExposurePlan>>> {
    let c = Columns::of(connection, "exposureplan")?;
    let sql = format!(
        "SELECT e.Id, {}, e.targetid, e.exposureTemplateId, e.exposure,
                COALESCE(e.desired, 0), COALESCE(e.acquired, 0), COALESCE(e.accepted, 0), {}
         FROM exposureplan e JOIN target t ON t.Id=e.targetid JOIN project p ON p.Id=t.projectid
         WHERE p.profileId=?1 ORDER BY e.Id",
        c.or("e", "guid", "NULL"),
        c.or("e", "enabled", "1"),
    );
    let count = |value: i64| u32::try_from(value.max(0)).unwrap_or(u32::MAX);
    let rows = connection
        .prepare(&sql)?
        .query_map([profile_id], |row| {
            Ok((
                row.get::<_, i64>(2)?,
                ExposurePlan {
                    id: row.get(0)?,
                    guid: row.get(1)?,
                    template_id: row.get(3)?,
                    exposure_seconds: row.get::<_, Option<f64>>(4)?.unwrap_or(-1.0),
                    desired: count(row.get(5)?),
                    acquired: count(row.get(6)?),
                    accepted: count(row.get(7)?),
                    enabled: row.get::<_, i64>(8)? != 0,
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut plans: BTreeMap<i64, Vec<ExposurePlan>> = BTreeMap::new();
    for (target, plan) in rows {
        plans.entry(target).or_default().push(plan);
    }
    Ok(plans)
}

struct Item {
    order: i64,
    next: bool,
    action: i64,
    reference_index: Option<i64>,
}

/// A target's override order or filter cadence rows, by target, in order.
fn read_items(
    connection: &Connection,
    profile_id: &str,
    table: &str,
    with_next: bool,
) -> rusqlite::Result<BTreeMap<i64, Vec<Item>>> {
    let c = Columns::of(connection, table)?;
    if !c.exists() {
        return Ok(BTreeMap::new());
    }
    let next = if with_next {
        c.or("i", "next", "0")
    } else {
        "0".into()
    };
    let sql = format!(
        "SELECT i.targetid, i.\"order\", {next}, i.action, i.referenceIdx
         FROM \"{table}\" i JOIN target t ON t.Id=i.targetid JOIN project p ON p.Id=t.projectid
         WHERE p.profileId=?1 ORDER BY i.targetid, i.\"order\", i.Id"
    );
    let rows = connection
        .prepare(&sql)?
        .query_map([profile_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                Item {
                    order: row.get(1)?,
                    next: row.get::<_, i64>(2)? != 0,
                    action: row.get(3)?,
                    reference_index: row.get(4)?,
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut items: BTreeMap<i64, Vec<Item>> = BTreeMap::new();
    for (target, item) in rows {
        items.entry(target).or_default().push(item);
    }
    Ok(items)
}
