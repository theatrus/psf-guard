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
//!
//! A request is answered in place: it is about a second of work at most,
//! too short for a row in the job line. It still follows the rules for
//! heavy work. It measures in a pool of workers leased from the
//! interactive budget, so it shares the cores with a stack someone
//! started, and it holds the interactive guard, so background work steps
//! aside. Requests measure one at a time ([`Grids::turn`]): a second
//! request for the same nights waits and then finds the grids kept, and
//! the requests left behind by someone still typing cannot pile up on the
//! cores. A request whose caller has gone measures nothing more.

use super::feasibility::{combined_limits, FeasibilityError, DARK_BELOW_DEGREES, STEP_MS};
use super::least_recent::LeastRecent;
use super::*;
use crate::concurrency::{plan_workers, ComputePool, Priority, Stop};
use psf_guard_director_core::{
    night::{place_over, DarkRuns, NightDate, NightError, NightSky, MAX_NIGHTS},
    visibility::{AltitudeLimits, Horizon, IcrsPosition, Site},
};
use psf_guard_director_meta::{
    profile::RigProfile,
    site_profile::{Origin, RigSite},
};
use rayon::prelude::*;
use std::time::{SystemTime, UNIX_EPOCH};

mod contour;

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
    /// The first night, the evening's date at the rig's site, as
    /// `YYYY-MM-DD`; without it, the night under way there now. Read as
    /// text so a date that is not a night is refused with a sentence.
    #[serde(default)]
    first_night: Option<String>,
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
    kept: Mutex<LeastRecent<Key, Grid>>,
    /// Held while one request measures, so they go one at a time.
    turn: Mutex<()>,
    /// How many grids were computed.
    #[cfg(test)]
    computed: std::sync::atomic::AtomicUsize,
}

impl Grids {
    fn get(&self, key: &Key) -> Option<Grid> {
        self.kept.lock().unwrap_or_else(|e| e.into_inner()).get(key)
    }

    fn keep(&self, key: Key, grid: Grid) {
        self.kept
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keep(key, grid, MAX_KEPT);
        #[cfg(test)]
        self.computed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn computed(&self) -> usize {
        self.computed.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Why the sky was not measured.
#[derive(Debug, PartialEq)]
enum Failure {
    /// The astronomy refused a night or a place.
    Night(NightError),
    /// The request's caller has gone.
    Stopped,
}

impl From<NightError> for Failure {
    fn from(error: NightError) -> Self {
        Self::Night(error)
    }
}

/// What the sky is judged with, read from the store.
pub(super) struct Inputs {
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

/// One row of the grids of the nights `wanted`, among `runs`: for each of
/// those nights, whether each node has a run of `needed` samples. A node
/// is placed once for all the nights. Each pole is one place, measured
/// once.
fn grid_rows(
    runs: &[DarkRuns<'_>],
    wanted: &[usize],
    row: usize,
    needed: u32,
) -> Result<Vec<Vec<bool>>, NightError> {
    let cols = if row == 0 || row == ROWS - 1 { 1 } else { COLS };
    let mut rows: Vec<Vec<bool>> = wanted.iter().map(|_| Vec::with_capacity(COLS)).collect();
    for col in 0..cols {
        let at = node(col, row);
        let placed = place_over(runs, at)?;
        for (row, night) in rows.iter_mut().zip(wanted) {
            row.push(runs[*night].reaches_placed(at, needed, &placed, *night)?);
        }
    }
    if cols == 1 {
        for row in &mut rows {
            *row = vec![row[0]; COLS];
        }
    }
    Ok(rows)
}

/// The lines around the nodes `seen` marks, placed by `in_view`, and the
/// share of the sky inside them. A place `in_view` cannot judge fails the
/// region, as it fails a grid row: it is never taken for out of view.
fn region(
    seen: &[bool],
    in_view: &(dyn Fn(IcrsPosition) -> Result<bool, Failure> + Sync),
) -> Result<Region, Failure> {
    let crossing = |from: contour::Node, to: contour::Node| -> Result<f64, Failure> {
        // `from` is in view and `to` is not: halve towards the change.
        let (mut near, mut far) = (0.0_f64, 1.0_f64);
        for _ in 0..HALVINGS {
            let middle = (near + far) / 2.0;
            let across = from.0 as f64 + (to.0 as f64 - from.0 as f64) * middle;
            let up = from.1 as f64 + (to.1 as f64 - from.1 as f64) * middle;
            if in_view(place(across, up))? {
                near = middle;
            } else {
                far = middle;
            }
        }
        Ok((near + far) / 2.0)
    };
    let lines = contour::contour(seen, COLS, ROWS, &crossing)?
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
    Ok(Region {
        lines,
        sky_fraction: (inside / whole * 10_000.0).round() / 10_000.0,
    })
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

/// The view for `inputs` over `dates`, measured in this request's turn in
/// a pool of leased workers. `stop` is asked once the caller has gone.
pub(super) fn compute(
    state: &AppState,
    service: &Service,
    inputs: Inputs,
    dates: Vec<NightDate>,
    minimum_minutes: u32,
    stop: &Stop,
) -> Result<View, FeasibilityError> {
    let failed = |failure: Failure| {
        match failure {
            Failure::Night(error) => {
                tracing::warn!(?error, "visible sky could not be computed")
            }
            Failure::Stopped => tracing::debug!("visible sky left: its request has gone"),
        }
        FeasibilityError::Api(Error::Internal)
    };
    let _turn = service
        .visible_sky
        .turn
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // Requests left behind while this one waited end here.
    if stop.requested() {
        return Err(failed(Failure::Stopped));
    }
    let _interactive = state.begin_named_interactive_job("the visible sky");
    let budget = plan_workers(None, &state.worker_policy(), Priority::Interactive, None);
    let lease = state.lease_workers(Priority::Interactive, budget.workers);
    let pool = ComputePool::take("visible-sky", lease.workers).map_err(|error| {
        tracing::error!(%error, "no pool for the visible sky");
        FeasibilityError::Api(Error::Internal)
    })?;
    tracing::debug!(
        workers = lease.workers,
        nights = dates.len(),
        "measuring the visible sky"
    );
    // The skies, the grids and the crossings all run inside the pool.
    pool.install(|| measure(service, inputs, dates, minimum_minutes, stop))
        .map_err(failed)
}

/// [`compute`]'s arithmetic, on the pool of the calling thread.
fn measure(
    service: &Service,
    inputs: Inputs,
    dates: Vec<NightDate>,
    minimum_minutes: u32,
    stop: &Stop,
) -> Result<View, Failure> {
    let site = inputs.site;
    let began = std::time::Instant::now();
    let noons: Vec<u64> = dates
        .iter()
        .map(|date| date.span(site.longitude_degrees).start_ms)
        .collect();
    let skies: Vec<Arc<NightSky>> = service
        .skies
        .nights(site, &noons, STEP_MS, false)
        .into_iter()
        .collect::<Result<_, _>>()?;
    let horizon = &inputs.placed.horizon;
    let runs: Vec<DarkRuns<'_>> = skies
        .iter()
        .map(|sky| DarkRuns::new(sky, horizon, inputs.limits, DARK_BELOW_DEGREES))
        .collect::<Result<_, _>>()?;
    let skies_ms = began.elapsed().as_millis();
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
    // Every row of the nights not kept, across the pool: a row's nodes
    // are placed once for all those nights. A row not yet begun when the
    // caller goes is not begun.
    let missing: Vec<usize> = (0..grids.len())
        .filter(|night| grids[*night].is_none())
        .collect();
    let rows: Vec<usize> = if missing.is_empty() {
        Vec::new()
    } else {
        (0..ROWS).collect()
    };
    let measured: Vec<Result<Vec<Vec<bool>>, Failure>> = rows
        .par_iter()
        .map(|row| {
            if stop.requested() {
                return Err(Failure::Stopped);
            }
            Ok(grid_rows(&runs, &missing, *row, needed)?)
        })
        .collect();
    let mut fresh: Vec<Vec<bool>> = missing
        .iter()
        .map(|_| Vec::with_capacity(COLS * ROWS))
        .collect();
    for row in measured {
        for (grid, row) in fresh.iter_mut().zip(row?) {
            grid.extend(row);
        }
    }
    for (night, grid) in missing.into_iter().zip(fresh) {
        let grid = Arc::new(grid);
        service
            .visible_sky
            .keep(keys[night].clone(), Arc::clone(&grid));
        grids[night] = Some(grid);
    }
    let grids: Vec<Grid> = grids.into_iter().flatten().collect();
    let grids_ms = began.elapsed().as_millis() - skies_ms;
    let every: Vec<bool> = (0..COLS * ROWS)
        .map(|index| grids.iter().all(|grid| grid[index]))
        .collect();
    // Whether a place between nodes has its run on every night, or on any:
    // placed once for all of them. An error here fails the request, as it
    // does in a grid row.
    let on_nights = |at: IcrsPosition, every: bool| -> Result<bool, Failure> {
        if stop.requested() {
            return Err(Failure::Stopped);
        }
        let placed = place_over(&runs, at)?;
        for (night, runs) in runs.iter().enumerate() {
            if runs.reaches_placed(at, needed, &placed, night)? != every {
                return Ok(!every);
            }
        }
        Ok(every)
    };
    let every_night = region(&every, &|at| on_nights(at, true))?;
    let any_night = if grids.len() > 1 {
        let any: Vec<bool> = (0..COLS * ROWS)
            .map(|index| grids.iter().any(|grid| grid[index]))
            .collect();
        Some(region(&any, &|at| on_nights(at, false))?)
    } else {
        None
    };
    // Where the time went, for the next person to wonder.
    tracing::debug!(
        nights = dates.len(),
        needed,
        skies_ms = skies_ms as u64,
        grids_ms = grids_ms as u64,
        lines_ms = (began.elapsed().as_millis() - skies_ms - grids_ms) as u64,
        "measured the visible sky"
    );
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

/// What the rig's sky is judged with, read on one store snapshot. `422`
/// when the rig has no location, or limits that leave nothing to image.
pub(super) fn read_inputs(store: &MetaStore, rig_id: Uuid) -> Result<Inputs, FeasibilityError> {
    let rig = store.rig(rig_id)?.ok_or(Error::Missing)?;
    // A rig with no profile yet can still use its site's location and
    // horizon; its limits are the defaults until it has one.
    let profile = store.rig_profile(rig_id)?;
    let placed = store.rig_site(rig_id, profile.as_ref())?;
    let profile = profile.unwrap_or_else(|| RigProfile::empty(rig_id, 0));
    // Global, then the rig's site, then the rig: the limits its Target
    // Scheduler projects start from.
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
}

/// The nights a request covers: `nights` from `first`, or from the night
/// under way at the site at `now_ms`. `400` with the reason when `first` is
/// not a night, or the range runs past the last night the astronomy times.
pub(super) fn nights_asked(
    first: Option<&str>,
    nights: u32,
    longitude_degrees: f64,
    now_ms: u64,
) -> Result<Vec<NightDate>, FeasibilityError> {
    let first = match first {
        Some(text) => text.parse::<NightDate>().map_err(|_| {
            FeasibilityError::Rejected(
                "first_night must be a date from 1970-01-02 to 2098-12-31, written YYYY-MM-DD."
                    .into(),
            )
        })?,
        None => NightDate::at(now_ms, longitude_degrees).ok_or(Error::Internal)?,
    };
    let dates: Vec<NightDate> = std::iter::successors(Some(first), |date| date.next())
        .take(nights as usize)
        .collect();
    if dates.len() != nights as usize {
        return Err(FeasibilityError::Rejected(
            "The last night must be 2098-12-31 or earlier.".into(),
        ));
    }
    Ok(dates)
}

/// Asks the work to stop when the request that started it is dropped, as
/// it is when its caller hangs up or cancels.
struct StopWhenDropped(Stop);

impl Drop for StopWhenDropped {
    fn drop(&mut self) {
        self.0.request();
    }
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
        .with_reader(move |store| read_inputs(store, rig_id))
        .await?;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let dates = nights_asked(
        request.first_night.as_deref(),
        nights,
        inputs.site.longitude_degrees,
        now_ms,
    )?;
    let minimum_minutes = request
        .minimum_minutes
        .unwrap_or(inputs.rig_minimum_minutes)
        .min(MAX_MINIMUM_MINUTES);
    // No store slot is held while the sky is measured. The work runs on a
    // blocking thread that outlives this request, so it is told when the
    // request is dropped.
    let stop = Stop::new();
    let gone = StopWhenDropped(stop.clone());
    let view = blocking(move || compute(&state, &service, inputs, dates, minimum_minutes, &stop))
        .await??;
    drop(gone);
    Ok(Json(ApiResponse::success(view)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_that_cannot_be_judged_fails_the_region() {
        // A cap around the north pole: its edge has crossings to place.
        let cap: Vec<bool> = (0..COLS * ROWS).map(|index| index / COLS >= 70).collect();
        // The lowest row in view is at +50°; the sky is in view from +48.6°.
        let placed = region(&cap, &|at| Ok(at.dec_degrees >= 48.6)).unwrap();
        assert_eq!(placed.lines.len(), 1);
        assert!(placed.lines[0]
            .iter()
            .all(|point| (point[1] - 48.6).abs() < 0.13));
        let failed = region(&cap, &|_| Err(Failure::Night(NightError::InvalidRequest)));
        assert_eq!(
            failed.err(),
            Some(Failure::Night(NightError::InvalidRequest))
        );
        // The caller going while crossings are placed ends it the same way.
        assert_eq!(
            region(&cap, &|_| Err(Failure::Stopped)).err(),
            Some(Failure::Stopped)
        );
    }

    /// The grids and lines of `nights` from `first_noon` at a site, by the
    /// shortcut or by the plain rule: the same steps the endpoint takes.
    fn regions(
        site: Site,
        first_noon: u64,
        nights: u64,
        horizon: &Horizon,
        minimum_degrees: f64,
        needed: u32,
        plain: bool,
    ) -> (Vec<Vec<bool>>, Region, Region) {
        let limits = AltitudeLimits {
            rig_minimum_degrees: minimum_degrees,
            project_minimum_degrees: -90.0,
            horizon_offset_degrees: 0.0,
            rig_maximum_degrees: 90.0,
            project_maximum_degrees: 90.0,
        };
        let skies: Vec<NightSky> = (0..nights)
            .map(|night| NightSky::new(site, first_noon + night * 86_400_000, STEP_MS).unwrap())
            .collect();
        let runs: Vec<DarkRuns<'_>> = skies
            .iter()
            .map(|sky| {
                if plain {
                    DarkRuns::plain(sky, horizon, limits, DARK_BELOW_DEGREES).unwrap()
                } else {
                    DarkRuns::new(sky, horizon, limits, DARK_BELOW_DEGREES).unwrap()
                }
            })
            .collect();
        let wanted: Vec<usize> = (0..runs.len()).collect();
        let rows: Vec<Vec<Vec<bool>>> = (0..ROWS)
            .into_par_iter()
            .map(|row| grid_rows(&runs, &wanted, row, needed).unwrap())
            .collect();
        let mut grids: Vec<Vec<bool>> = wanted.iter().map(|_| Vec::new()).collect();
        for row in rows {
            for (grid, row) in grids.iter_mut().zip(row) {
                grid.extend(row);
            }
        }
        let on_nights = |at: IcrsPosition, every: bool| -> Result<bool, Failure> {
            let placed = place_over(&runs, at)?;
            for (night, runs) in runs.iter().enumerate() {
                if runs.reaches_placed(at, needed, &placed, night)? != every {
                    return Ok(!every);
                }
            }
            Ok(every)
        };
        let every: Vec<bool> = (0..COLS * ROWS)
            .map(|index| grids.iter().all(|grid| grid[index]))
            .collect();
        let any: Vec<bool> = (0..COLS * ROWS)
            .map(|index| grids.iter().any(|grid| grid[index]))
            .collect();
        let every = region(&every, &|at| on_nights(at, true)).unwrap();
        let any = region(&any, &|at| on_nights(at, false)).unwrap();
        (grids, every, any)
    }

    #[test]
    fn the_shortcut_draws_the_lines_the_plain_rule_draws() {
        let point = |azimuth_degrees: f64, altitude_degrees: f64| {
            psf_guard_director_core::visibility::HorizonPoint {
                azimuth_degrees,
                altitude_degrees,
            }
        };
        let wall = Horizon::Custom {
            points: vec![
                point(0.0, 10.0),
                point(119.0, 10.0),
                point(120.0, 65.0),
                point(150.0, 65.0),
                point(151.0, 10.0),
                point(360.0, 10.0),
            ],
        };
        // A mast one degree wide, due south.
        let narrow = Horizon::Custom {
            points: vec![
                point(0.0, 5.0),
                point(179.5, 5.0),
                point(180.0, 70.0),
                point(181.0, 70.0),
                point(181.5, 5.0),
                point(360.0, 5.0),
            ],
        };
        let flat = Horizon::FixedMinimum {};
        let site = |latitude_degrees: f64, longitude_degrees: f64| Site {
            latitude_degrees,
            longitude_degrees,
            elevation_meters: 100.0,
        };
        // Local noon on the first night, near enough: the instant's night.
        let noon = |date: &str, longitude: f64| {
            date.parse::<NightDate>().unwrap().span(longitude).start_ms
        };
        let cases: [(&str, Site, &str, u64, &Horizon, f64, u32); 6] = [
            (
                "the scratch site and its wall",
                site(40.0, -105.3),
                "2026-11-15",
                4,
                &wall,
                20.0,
                6,
            ),
            (
                "the same at one sample",
                site(40.0, -105.3),
                "2026-11-15",
                3,
                &wall,
                20.0,
                1,
            ),
            (
                "52° north in June, a short dark",
                site(52.0, 0.0),
                "2027-06-05",
                4,
                &flat,
                25.0,
                6,
            ),
            (
                "Tromsø as the dark returns",
                site(69.6, 18.9),
                "2027-09-10",
                5,
                &flat,
                20.0,
                2,
            ),
            (
                "Cape Town and the wall",
                site(-33.9, 18.5),
                "2027-01-10",
                3,
                &wall,
                15.0,
                12,
            ),
            (
                "a mast one degree wide",
                site(40.0, -105.3),
                "2027-03-01",
                3,
                &narrow,
                0.0,
                3,
            ),
        ];
        for (name, site, date, nights, horizon, minimum, needed) in cases {
            let first = noon(date, site.longitude_degrees);
            let (grids, every, any) = regions(site, first, nights, horizon, minimum, needed, false);
            let (plain_grids, plain_every, plain_any) =
                regions(site, first, nights, horizon, minimum, needed, true);
            // Node for node and point for point: no difference at all.
            assert_eq!(grids, plain_grids, "{name}: grids");
            assert_eq!(every.lines, plain_every.lines, "{name}: every night");
            assert_eq!(any.lines, plain_any.lines, "{name}: any night");
            assert_eq!(every.sky_fraction, plain_every.sky_fraction, "{name}");
            assert_eq!(any.sky_fraction, plain_any.sky_fraction, "{name}");
            // And there was something to compare.
            assert!(
                !any.lines.is_empty() && any.sky_fraction > 0.05,
                "{name}: {}",
                any.sky_fraction
            );
        }
    }

    #[test]
    fn the_nights_asked_stay_inside_the_years_the_astronomy_times() {
        let asked = |first, nights| nights_asked(Some(first), nights, -118.3, 0);
        let dates = asked("1970-01-02", 2).ok().unwrap();
        assert_eq!(dates[1].to_string(), "1970-01-03");
        assert_eq!(asked("2098-12-31", 1).ok().unwrap().len(), 1);
        for (first, nights) in [
            ("1970-01-01", 1),
            ("2099-01-01", 1),
            ("2098-12-31", 2),
            ("2098-12-02", 31),
            ("2026-9-24", 1),
            ("tonight", 1),
        ] {
            assert!(
                matches!(asked(first, nights), Err(FeasibilityError::Rejected(_))),
                "{first} for {nights}"
            );
        }
        assert_eq!(asked("2098-12-01", 31).ok().unwrap().len(), 31);
        // Without a first night: the night under way at the site.
        let tonight = nights_asked(None, 1, -118.3, 1_790_294_400_000)
            .ok()
            .unwrap();
        assert_eq!(tonight[0].to_string(), "2026-09-24");
    }
}
