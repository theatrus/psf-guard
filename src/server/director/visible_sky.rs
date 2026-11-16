//! The sky a rig can shoot for long enough: for one night or a range, the
//! lines around the sky that stays in view, in the dark, for at least a
//! minimum time without a break. The Sky view draws them.
//!
//! In view is what Visibility and Nights count: the Sun below −12°, and the
//! place inside the rig's altitude limits and above its horizon. The
//! horizon is the one resolved for the rig, its own or its site's, whether
//! or not a plan turns Custom horizon on: the line is about what the rig
//! can see, not about one project's Target Scheduler settings.
//!
//! Each night is measured on a grid of the whole sky: at every node,
//! whether some unbroken run of dark samples in view is long enough. A
//! night's grid depends only on the site, the horizon, the limits, the
//! night and the minimum time, so it is kept ([`Grids`]) and another range
//! over the same nights reads it again. The lines are then placed between
//! nodes by measuring there, to a small fraction of a cell.

use super::feasibility::{combined_limits, FeasibilityError};
use super::*;
use psf_guard_director_core::{
    night::{DarkRuns, NightDate, NightError, NightSky, MAX_NIGHTS},
    visibility::{AltitudeLimits, Horizon, IcrsPosition, Site},
};
use psf_guard_director_meta::{
    profile::RigProfile,
    site_profile::{Origin, RigSite},
};
use rayon::prelude::*;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

mod contour;

/// The step and the darkness feasibility and night plans count with, so the
/// line agrees with their hours.
const STEP_MS: u64 = 300_000;
const DARK_BELOW_DEGREES: f64 = -12.0;
/// The grid's spacing in degrees: 180 columns of right ascension from 0h,
/// 91 rows of declination from pole to pole.
const GRID_DEGREES: f64 = 2.0;
const COLS: usize = 180;
const ROWS: usize = 91;
/// How many times a crossing between two nodes is halved: to a sixteenth of
/// a cell, an eighth of a degree.
const HALVINGS: u32 = 4;
/// The longest minimum time: a day, as Scheduling limits allow.
const MAX_MINIMUM_MINUTES: u32 = 24 * 60;
/// The most night grids kept, about 16 kB each: a month of nights for
/// eight rigs or minimum times.
const MAX_KEPT: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    rig: Uuid,
    /// The shortest unbroken time in view that counts; without it, the
    /// rig's Target Scheduler minimum time.
    #[serde(default)]
    minimum_minutes: Option<u32>,
    /// The first night, the evening's date at the rig's site; without it,
    /// the night under way there now.
    #[serde(default)]
    first_night: Option<NightDate>,
    /// How many nights from the first; 1 without it.
    #[serde(default)]
    nights: Option<u32>,
}

/// One night of the range, as Nights dates and times it.
#[derive(Serialize)]
struct NightSpan {
    date: NightDate,
    /// The first dark sample and the end of the last; `None` when the Sun
    /// never gets low enough.
    dusk_ms: Option<u64>,
    dawn_ms: Option<u64>,
}

/// The limits the sky was judged with.
#[derive(Serialize)]
struct Limits {
    minimum_altitude_degrees: f64,
    maximum_altitude_degrees: f64,
    /// Degrees added to the custom horizon.
    horizon_offset_degrees: f64,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum HorizonKind {
    /// A horizon curve, with the altitude limits.
    Custom,
    /// No curve: the minimum altitude all round.
    Flat,
}

/// The sky that meets the minimum time, and the lines around it.
#[derive(Serialize)]
struct Region {
    /// Closed lines of `[ra_degrees, dec_degrees]`, ICRS. The sky that
    /// meets the minimum time is on the left of each as its points run,
    /// seen with north up and right ascension rising to the right.
    lines: Vec<Vec<[f64; 2]>>,
    /// The share of the whole sky inside the lines, 0 to 1.
    sky_fraction: f64,
}

#[derive(Serialize)]
pub(super) struct View {
    rig: NamedIdentity,
    /// The planning site the rig is set to, if any.
    planning_site: Option<NamedIdentity>,
    site: Site,
    location_from: Origin,
    horizon: HorizonKind,
    horizon_from: Origin,
    limits: Limits,
    /// The minimum time used, and the rig's own.
    minimum_minutes: u32,
    rig_minimum_minutes: u32,
    step_minutes: u32,
    dark_below_degrees: f64,
    nights: Vec<NightSpan>,
    /// Sky that meets the minimum time on every night.
    every_night: Region,
    /// Sky that meets it on at least one night; `None` for a single night,
    /// when it is the same sky.
    any_night: Option<Region>,
}

/// What a night's grid is computed from, to the bit.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    site: [u64; 3],
    /// The horizon's points, azimuth then altitude; `None` without a curve.
    horizon: Option<Arc<Vec<u64>>>,
    limits: [u64; 5],
    noon_ms: u64,
    step_ms: u64,
    dark_below: u64,
    /// The run that counts, in samples.
    needed: u32,
    /// The grid's shape: its spacing, columns and rows.
    grid: (u64, usize, usize),
}

/// Whether each node has a long enough run on one night, row by row from
/// the south pole.
type Grid = Arc<Vec<bool>>;

/// The night grids kept across requests. A grid is computed from its key
/// alone, so a kept grid never goes stale; the least recently read go
/// first.
#[derive(Default)]
pub(super) struct Grids {
    kept: Mutex<Kept>,
    /// How many grids were computed.
    #[cfg(test)]
    computed: std::sync::atomic::AtomicUsize,
}

#[derive(Default)]
struct Kept {
    /// Each grid with the count of reads when it was last read.
    grids: HashMap<Key, (Grid, u64)>,
    /// Counts reads, to find the least recently read.
    reads: u64,
}

impl Grids {
    fn get(&self, key: &Key) -> Option<Grid> {
        let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        kept.reads += 1;
        let reads = kept.reads;
        kept.grids.get_mut(key).map(|(grid, read)| {
            *read = reads;
            Arc::clone(grid)
        })
    }

    fn keep(&self, key: Key, grid: Grid) {
        let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        kept.reads += 1;
        let reads = kept.reads;
        kept.grids.insert(key, (grid, reads));
        while kept.grids.len() > MAX_KEPT {
            let Some(oldest) = kept
                .grids
                .iter()
                .min_by_key(|(_, (_, read))| *read)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            kept.grids.remove(&oldest);
        }
        #[cfg(test)]
        self.computed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn computed(&self) -> usize {
        self.computed.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// What the sky is judged with, read from the store.
struct Inputs {
    rig: NamedIdentity,
    placed: RigSite,
    site: Site,
    limits: AltitudeLimits,
    rig_minimum_minutes: u32,
}

fn node(col: usize, row: usize) -> IcrsPosition {
    place(col as f64, row as f64)
}

/// A place in grid units as a sky position.
fn place(across: f64, up: f64) -> IcrsPosition {
    let ra = (across * GRID_DEGREES).rem_euclid(360.0);
    IcrsPosition {
        // The remainder of a value a hair under zero rounds to 360.
        ra_degrees: if ra >= 360.0 { 0.0 } else { ra },
        dec_degrees: (-90.0 + up * GRID_DEGREES).clamp(-90.0, 90.0),
    }
}

/// One row of a night's grid: whether each node has a run of `needed`
/// samples. Each pole is one place, measured once.
fn grid_row(runs: &DarkRuns<'_>, row: usize, needed: u32) -> Result<Vec<bool>, NightError> {
    if row == 0 || row == ROWS - 1 {
        return Ok(vec![runs.reaches(node(0, row), needed)?; COLS]);
    }
    (0..COLS)
        .map(|col| runs.reaches(node(col, row), needed))
        .collect()
}

/// The lines around the nodes `seen` marks, placed by `in_view`, and the
/// share of the sky inside them.
fn region(seen: &[bool], in_view: &(dyn Fn(IcrsPosition) -> bool + Sync)) -> Region {
    let crossing = |from: contour::Node, to: contour::Node| {
        // `from` is in view and `to` is not: halve towards the change.
        let (mut near, mut far) = (0.0_f64, 1.0_f64);
        for _ in 0..HALVINGS {
            let middle = (near + far) / 2.0;
            let across = from.0 as f64 + (to.0 as f64 - from.0 as f64) * middle;
            let up = from.1 as f64 + (to.1 as f64 - from.1 as f64) * middle;
            if in_view(place(across, up)) {
                near = middle;
            } else {
                far = middle;
            }
        }
        (near + far) / 2.0
    };
    let lines = contour::contour(seen, COLS, ROWS, &crossing)
        .into_iter()
        .map(|line| {
            line.into_iter()
                .map(|(across, up)| {
                    let at = place(across, up);
                    [
                        (at.ra_degrees * 100.0).round() / 100.0 % 360.0,
                        (at.dec_degrees * 100.0).round() / 100.0,
                    ]
                })
                .collect()
        })
        .collect();
    // Each row's nodes stand for a band of sky as wide as the cosine of
    // its declination.
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
    Region {
        lines,
        sky_fraction: (inside / whole * 10_000.0).round() / 10_000.0,
    }
}

fn key(inputs: &Inputs, horizon: &Option<Arc<Vec<u64>>>, noon_ms: u64, needed: u32) -> Key {
    let limits = inputs.limits;
    Key {
        site: [
            inputs.site.latitude_degrees.to_bits(),
            inputs.site.longitude_degrees.to_bits(),
            inputs.site.elevation_meters.to_bits(),
        ],
        horizon: horizon.clone(),
        limits: [
            limits.rig_minimum_degrees.to_bits(),
            limits.project_minimum_degrees.to_bits(),
            limits.horizon_offset_degrees.to_bits(),
            limits.rig_maximum_degrees.to_bits(),
            limits.project_maximum_degrees.to_bits(),
        ],
        noon_ms,
        step_ms: STEP_MS,
        dark_below: DARK_BELOW_DEGREES.to_bits(),
        needed,
        grid: (GRID_DEGREES.to_bits(), COLS, ROWS),
    }
}

/// The view for `inputs` over `dates`. Arithmetic only: the skies and the
/// grids are kept or computed across the shared pool.
fn compute(
    service: &Service,
    inputs: Inputs,
    dates: Vec<NightDate>,
    minimum_minutes: u32,
) -> Result<View, FeasibilityError> {
    let site = inputs.site;
    let failed = |error: NightError| {
        tracing::warn!(?error, "visible sky could not be computed");
        FeasibilityError::Api(Error::Internal)
    };
    let noons: Vec<u64> = dates
        .iter()
        .map(|date| date.span(site.longitude_degrees).start_ms)
        .collect();
    let skies: Vec<Arc<NightSky>> = service
        .skies
        .nights(site, &noons, STEP_MS, false)
        .into_iter()
        .collect::<Result<_, _>>()
        .map_err(failed)?;
    let horizon = &inputs.placed.horizon;
    let runs: Vec<DarkRuns<'_>> = skies
        .iter()
        .map(|sky| DarkRuns::new(sky, horizon, inputs.limits, DARK_BELOW_DEGREES))
        .collect::<Result<_, _>>()
        .map_err(failed)?;
    let horizon_key = match horizon {
        Horizon::FixedMinimum {} => None,
        Horizon::Custom { points } => Some(Arc::new(
            points
                .iter()
                .flat_map(|p| [p.azimuth_degrees.to_bits(), p.altitude_degrees.to_bits()])
                .collect(),
        )),
    };
    // A run counts in whole samples; a minimum under one sample still
    // needs one sample in view.
    let needed = (u64::from(minimum_minutes) * 60_000)
        .div_ceil(STEP_MS)
        .max(1) as u32;
    let keys: Vec<Key> = noons
        .iter()
        .map(|noon| key(&inputs, &horizon_key, *noon, needed))
        .collect();
    let mut grids: Vec<Option<Grid>> = keys
        .iter()
        .map(|key| service.visible_sky.get(key))
        .collect();
    // Every row of every night not kept, at once across the shared pool,
    // so one night spreads over it as well as many.
    let missing: Vec<usize> = (0..grids.len())
        .filter(|night| grids[*night].is_none())
        .collect();
    let rows: Vec<(usize, usize)> = missing
        .iter()
        .flat_map(|night| (0..ROWS).map(move |row| (*night, row)))
        .collect();
    let measured: Vec<Result<Vec<bool>, NightError>> = rows
        .par_iter()
        .map(|(night, row)| grid_row(&runs[*night], *row, needed))
        .collect();
    let mut measured = measured.into_iter();
    for night in missing {
        let mut grid = Vec::with_capacity(COLS * ROWS);
        for row in measured.by_ref().take(ROWS) {
            grid.extend(row.map_err(failed)?);
        }
        let grid = Arc::new(grid);
        service
            .visible_sky
            .keep(keys[night].clone(), Arc::clone(&grid));
        grids[night] = Some(grid);
    }
    let grids: Vec<Grid> = grids.into_iter().flatten().collect();
    let every: Vec<bool> = (0..COLS * ROWS)
        .map(|index| grids.iter().all(|grid| grid[index]))
        .collect();
    let reaches = |runs: &DarkRuns<'_>, at| runs.reaches(at, needed).unwrap_or(false);
    let every_night = region(&every, &|at| runs.iter().all(|runs| reaches(runs, at)));
    let any_night = (grids.len() > 1).then(|| {
        let any: Vec<bool> = (0..COLS * ROWS)
            .map(|index| grids.iter().any(|grid| grid[index]))
            .collect();
        region(&any, &|at| runs.iter().any(|runs| reaches(runs, at)))
    });
    let limits = inputs.limits;
    Ok(View {
        rig: inputs.rig,
        planning_site: inputs.placed.site.clone(),
        site,
        location_from: inputs.placed.location_from,
        horizon: match horizon {
            Horizon::Custom { .. } => HorizonKind::Custom,
            Horizon::FixedMinimum {} => HorizonKind::Flat,
        },
        horizon_from: inputs.placed.horizon_from,
        limits: Limits {
            minimum_altitude_degrees: limits
                .rig_minimum_degrees
                .max(limits.project_minimum_degrees),
            maximum_altitude_degrees: limits
                .rig_maximum_degrees
                .min(limits.project_maximum_degrees),
            horizon_offset_degrees: limits.horizon_offset_degrees,
        },
        minimum_minutes,
        rig_minimum_minutes: inputs.rig_minimum_minutes,
        step_minutes: (STEP_MS / 60_000) as u32,
        dark_below_degrees: DARK_BELOW_DEGREES,
        nights: dates
            .iter()
            .zip(&skies)
            .map(|(date, sky)| {
                let dark = sky.darkness(DARK_BELOW_DEGREES);
                NightSpan {
                    date: *date,
                    dusk_ms: dark.map(|(dusk, _)| dusk),
                    dawn_ms: dark.map(|(_, dawn)| dawn),
                }
            })
            .collect(),
        every_night,
        any_night,
    })
}

pub(super) async fn evaluate(
    State(state): State<Arc<AppState>>,
    Json(request): Json<Request>,
) -> Result<Json<ApiResponse<View>>, FeasibilityError> {
    let service = enabled(&state)?;
    let nights = request.nights.unwrap_or(1);
    if nights == 0 || nights > MAX_NIGHTS {
        return Err(FeasibilityError::Rejected(format!(
            "nights must be between 1 and {MAX_NIGHTS}."
        )));
    }
    if request
        .minimum_minutes
        .is_some_and(|minutes| minutes > MAX_MINIMUM_MINUTES)
    {
        return Err(FeasibilityError::Rejected(format!(
            "minimum_minutes must be at most {MAX_MINIMUM_MINUTES}."
        )));
    }
    let rig_id = request.rig;
    let inputs = service
        .clone()
        .with_reader(move |store| {
            let rig = store.rig(rig_id)?.ok_or(Error::Missing)?;
            // A rig with no profile yet can still use its site's location
            // and horizon; its limits are the defaults until it has one.
            let profile = store.rig_profile(rig_id)?;
            let placed = store.rig_site(rig_id, profile.as_ref())?;
            let profile = profile.unwrap_or_else(|| RigProfile::empty(rig_id, 0));
            // Global, then the rig's site, then the rig: the limits its
            // Target Scheduler projects start from.
            let scheduling = store
                .effective_observing_preferences(rig_id, None)?
                .scheduling
                .values;
            let Some(site) = placed.location else {
                return Err(FeasibilityError::NotReady(format!(
                    "{} has no location. Choose a Planning site in Rig setup.",
                    rig.name
                )));
            };
            let limits = combined_limits(
                &profile.limits.value,
                scheduling.minimum_altitude_degrees,
                scheduling.maximum_altitude_degrees,
                scheduling.horizon_offset_degrees,
            )
            .map_err(|limits| {
                FeasibilityError::NotReady(format!(
                    "{}: the altitude limits leave nothing to image ({limits}).",
                    rig.name
                ))
            })?;
            Ok(Inputs {
                rig,
                placed,
                site,
                limits,
                rig_minimum_minutes: scheduling.minimum_time_minutes,
            })
        })
        .await?;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let first = match request.first_night {
        Some(first) => first,
        None => NightDate::at(now_ms, inputs.site.longitude_degrees).ok_or(Error::Internal)?,
    };
    let dates: Vec<NightDate> = std::iter::successors(Some(first), |date| date.next())
        .take(nights as usize)
        .collect();
    // The core times nights only in these years.
    let in_years = |date: &NightDate| {
        let span = date.span(inputs.site.longitude_degrees);
        (psf_guard_director_core::night::EARLIEST_START_MS
            ..=psf_guard_director_core::night::LATEST_START_MS)
            .contains(&span.start_ms)
    };
    if dates.len() != nights as usize || !dates.iter().all(in_years) {
        return Err(FeasibilityError::Rejected(
            "first_night must fall between 1970-01-02 and 2099-01-01.".into(),
        ));
    }
    let minimum_minutes = request
        .minimum_minutes
        .unwrap_or(inputs.rig_minimum_minutes)
        .min(MAX_MINIMUM_MINUTES);
    // No store slot is held while the sky is measured.
    let view = blocking(move || compute(&service, inputs, dates, minimum_minutes)).await??;
    Ok(Json(ApiResponse::success(view)))
}
