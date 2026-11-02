# Night plans

Status: **In progress**. Phases 1 to 5 and 7 are built: summaries, manual
availability, the Target Scheduler emulation, the Director emulation,
projection (each plan's completion night by its rig's own schedule, and an
availability-aware `nights_to_complete`), and cloud forecasts. Phase 6
(overrides) is not. §5 says what each phase built and what is left. The
answers in §7 marked *provisional* are what the code does now; confirm them
or change them. Last updated: 2026-11-01. Tracks
[#1037](https://github.com/theatrus/psf-guard/issues/1037).

This record plans per-rig nightly plan summaries, a shooting schedule preview
and night availability (weather and clouds, marked by hand first). The
preview emulates the runtime that drives each rig: Target Scheduler's
planning engine on a Target Scheduler rig, and Director's selection on a
Director rig. Overrides and fixed steps are for Director rigs only.

It extends [Director](director.md), whose Phase 3 lists virtual-clock
simulation and a per-rig night preview through the same engine, and the
[two rig modes](director.md#two-rig-modes-target-scheduler-and-director-proposal)
proposal. It reuses the feasibility that the framing view's
[Visibility](../DIRECTOR.md#visibility) strip shows.

## 1. Problem

PSF Guard times one plan at a time. The Visibility strip and
`POST /projects/{id}/feasibility` answer "when can this rig shoot this
target", but no view answers the questions a person asks before a night:

- What is each rig going to shoot tonight, and in what order?
- Which plans compete for the same dark hours, and which rig sits idle?
- Will this plan finish this month, or does it need another rig?
- What changes if Tuesday is cloudy, or a rig is down next week?

The shared core (`crates/director-core`) has most of what it takes: the night
calculations, the horizon, limit and meridian geometry, per-filter Moon
rules, rig priority, and the goal selection Director's dispatch runs. Today
only the framing view and the Director plugin use them, and the plugin is new
and lightly used.

Most rigs run Target Scheduler, which picks its own work on the rig with its
own rules. A preview of such a rig is only useful if it predicts what Target
Scheduler will do, so this feature emulates Target Scheduler's planner as
well as Director's.

Nothing in PSF Guard knows about weather. `nights_to_complete` assumes every
night is clear.

Goals:

1. **Night plans for every rig, or for chosen rigs**, over a range of nights
   starting at any instant: tonight and the next week by default, up to 31
   nights (`night::MAX_NIGHTS`).
2. **A summary for each rig and night**: darkness, the Moon, each plan's hours
   inside the rig's limits, and the load (hours owed against hours available).
3. **A schedule preview for each rig and night, from the runtime that drives
   it**: a timeline of what the rig would shoot, with each block's reason,
   and the gaps with theirs.
4. **Night availability**: a night marked available, unavailable or limited to
   some hours, by site or by rig. Manual marks come first and forecast sources
   later. Summaries and both runtimes' schedules honour them.
5. **Projection across nights**: frames planned on one night count on the
   next, so each plan gets a projected completion night.
6. **Overrides and fixed steps on Director rigs**: pin work to a window,
   leave a plan out for a night, change a night's order, fix a night's blocks,
   and see the steps Director will run. A Target Scheduler rig's preview is a
   prediction only.

## 2. Current state

### 2.1 Shared core

- `night`:
  - `night_preview`, `night_summary` and `night_curve` take a
    `NightRequest`, which holds a site, a horizon, limits, up to 16
    targets, a start, up to 31 nights, a sample step, a darkness
    threshold, the meridian pause and the meridian window.
  - Each `Night` runs from local mean noon to the next and is keyed by
    the evening's date (`YYYY-MM-DD`). It reports dusk, dawn, dark
    hours, the Moon's illumination and its hours up in the dark.
  - Each `TargetNight` gives hours up inside the limits, the same with
    the Moon down, hours lost to the meridian pause, the transit, the
    peak altitude and the closest approach to the Moon.
- `geometry::BoundGeometry`:
  - Built from a `BoundProgram` and a rig's `Constraints`: site, Earth
    orientation, horizon, the rig's altitude limits and meridian pause,
    and each goal's limits.
  - For every goal it computes the altitude windows and intersects them
    with the meridian windows and with the goal's allocated windows. Where
    the goal's recipe has Moon rules, it intersects the lunar windows too.
    It keeps both sets (`windows` and `non_lunar_windows`) and, at equal
    priority, ranks Moon-sensitive goals first.
  - Because the allocated windows are intersected, a program can only
    narrow what geometry allows.
  - `evaluate` returns the decision at one instant. This is the
    selection the sidecar runs on the rig.
- `evaluate` (crate root):
  - picks the highest-priority goal whose exposure and overhead fit
    inside one of its windows now;
  - otherwise waits for a future window, or reports the work complete,
    or that nothing it may take is feasible.
- `preparation`: the reducer for the native steps before an exposure:
  `Unpark`, `Center { rotate }`, the `BeforeTarget` hook, `Dither`,
  `SwitchFilter` and `SetReadoutMode`.
- `moon::moon_windows`: lunar windows for one night and one policy.
  `moon` also holds the Target Scheduler-compatible Lorentzian avoidance.
- `priority`:
  - resolves the explainable policy of legacy weighted programs;
  - `priority::preview` ranks the candidates, with each factor's
    contribution;
  - `priority::ts_importance` maps a Target Scheduler priority.
  - Ranked programs compile the global, site or rig project order into
    `Goal.priority` instead (Director, *User-controlled observing
    priority*).
- The core has no Target Scheduler, SQLite, HTTP or N.I.N.A. dependency
  (Director, *Shared planning core*).

### 2.2 Server

- `src/server/director/feasibility.rs`:
  - For one plan and every rig with a site, it computes:
    - the applied limits: the tighter of the rig profile's limits and
      the plan's resolved scheduling limits;
    - nights sampled every five minutes, counting dark below −12°;
    - tonight's curve;
    - `hours_needed` and `nights_to_complete`.
  - Rigs and nights run on the rayon pool.
- `src/server/director/program.rs`, `assemble` (the plugin's pull):
  - It refuses a rig that has not reported its equipment (`NotReady`).
  - It reads the rig's Director activations and the Target Scheduler rows
    they wrote, read-only. That covers the exposure, desired and accepted
    counts, filter, gain, offset, binning, readout mode and the
    template's Moon rules.
  - It applies the project order (`apply_project_order`) and gives every
    goal the allocation span as its only window. The program is valid for
    24 hours.
  - It then *writes* a program issue to the meta store
    (`issue_program_preview`). The rig's geometry adds the real windows.
- `src/server/director/plan_progress.rs`: frames asked for, taken,
  accepted and rejected, per rig and objective, read from Target Scheduler.
- `src/server/scheduler.rs` and `src/ts_schema` (vendored, schema version
  23) read and write Target Scheduler's tables, including
  `ruleweight`, `filtercadenceitem`, `overrideexposureorderitem` and
  `profilepreference`.
- The meta store (`crates/director-meta`, schema 32) holds:
  - sites, each with a `SiteProfile` (location and horizon);
  - each rig's choice of planning site;
  - observing settings at global, site, rig and project scope, including
    `project_order`;
  - rig profiles, each with the equipment configuration once the plugin
    reports it.
- The rig mode (Target Scheduler or Director) is proposed, not built.

### 2.3 Target Scheduler's planner

Target Scheduler 5's
[planning engine](https://tcpalmer.github.io/nina-scheduler/concepts/planning-engine.html)
is a dispatch scheduler. Each call returns one exposure, or a wait. A call:

1. **Stays with the previous target** while it is incomplete, an exposure
   plan passes Moon avoidance, and the project's minimum time span still
   has room for the next exposure.
2. **Builds the candidates:** active projects of the N.I.N.A. profile, less
   targets that never rise and complete targets.
3. **Times visibility** over the minimum time from now. The horizon is the
   higher of the profile's custom horizon (plus its offset when enabled) and
   the project's minimum altitude, under the maximum altitude. The project's
   meridian window must allow the minimum time. The no-flip zone runs from
   transit less the pause before meridian, less 30 s, to transit plus the
   minutes after meridian, plus 30 s, and clips or rejects a target that
   meets it.
4. **Applies Moon avoidance** per exposure template: classic separation and
   width, relaxation by Moon altitude, and Moon must be down. Each surviving
   plan gets a Moon avoidance score.
5. **Applies each template's acceptable twilight:**
   - Nighttime: Sun below −18°.
   - Astronomical: below −12°.
   - Nautical: below −6°.
   - Civil: below 0°.
   - Each is shifted by the template's twilight offset in minutes.
6. **Splits ready from later:**
   - none ready but some later: a wait, found at one-minute steps;
   - one ready: that target;
   - several ready: the scoring engine.
7. **Scores** each ready target as Σ (weight ÷ 100) × rule score. Each rule
   scores 0 to 1, and its weight is set per project in `ruleweight`, where 0
   switches the rule off. The rules, with their default weights:
   - Meridian Flip Penalty, 0
   - Meridian Window Priority, 75
   - Mosaic Completion, 0
   - Percent Complete, 50
   - Priority, 50
   - Setting Soonest, 50
   - Smart Exposure Order, 0
   - Target Switch Penalty, 67
8. **Picks the exposure:**
   - Basic: the filter cadence, by filter switch frequency.
   - Repeat until done: frequency 0.
   - Override order: the target's own order.
   - Smart: the best Moon avoidance score.
   - It dithers before the N+1th use of a filter since the last dither.
9. **Emits instructions:**
   - slew, centre and rotate when the target changes;
   - filter and readout mode;
   - dither;
   - the exposure.

Its own Scheduler Preview calls the planner again from each plan's end, with
weather disconnected and no trigger overhead. It counts a frame as accepted
only when the project's grader is on. Its View Report lists each candidate,
its rule scores and why it was or was not chosen.

Reading Target Scheduler's source (5.9.6.1) for the emulation corrected
some of the above and the documentation it came from:

- Each call returns exactly one exposure; nothing fills a window with
  several. `enableSmartPlanWindow` is never read.
- Completion: with the grader on, a plan owes its desired frames less those
  accepted; with it off, the exposure throttle's share of the desired
  frames, truncated, less those acquired.
- The live run samples visibility every 10 s and searches for a wait in
  60 s steps; the Scheduler Preview uses 30 s and 180 s.
- Altitudes use the stored J2000 coordinates against apparent sidereal time,
  with no precession or refraction, and transit times count sidereal hours
  as clock hours. The emulation reproduces both.
- "Moon must be down" applies whether or not relaxation is on.
- A target set aside for twilight, or for a mix of reasons, is never waited
  for; with nothing else pending the planner ends the night.

### 2.4 Interface

- `VisibilityPanel` draws one plan's night for each rig: the altitude chart
  with horizon, Moon, darkness bands and meridian pause, plus a week of
  nights.
- `SkyPage`, `SkyTimeline` and `SkyLive` make up Live on the Sky and the
  coverage timeline.
- Nothing shows one rig across all its plans, or many rigs across nights.

## 3. Decision

1. **Compute on demand.** A night plan is a pure function of state PSF Guard
   already stores. A week for two rigs takes feasibility well under a tenth
   of a second, and a schedule adds one cheap selection per exposure. v1
   stores no plans. Section 5 keeps a saved snapshot for later, to compare
   planned and actual.
2. **Emulate both runtimes, each from what that runtime reads on the rig.**
   - **Target Scheduler rigs: a Target Scheduler emulation.**
     - It lives in a new pure crate, `crates/ts-planner`, which uses
       director-core's astronomy (`visibility`, `ephemeris`, `moon`,
       `windows`). Target Scheduler's semantics stay out of the core, as
       the core's design requires.
     - It reads only what Target Scheduler reads: the rig's database
       (projects, targets, exposure plans, templates, rule weights, filter
       cadences, override orders, profile preferences) and the N.I.N.A.
       profile's horizon and meridian flip settings, as the rig profile
       records them.
     - Director's project order and Director settings do not apply to a
       Target Scheduler rig, and the emulation ignores them.
   - **Director rigs: Director's selection.** A new pure module in the core,
     `schedule`, steps a virtual clock and asks `BoundGeometry::evaluate` at
     every step, the call the sidecar makes on the rig. This is the
     "virtual-clock simulation, decision explanations" item of Director's
     Phase 3.
   - **Choosing the runtime:** a rig's runtime is its rig mode, defaulting
     to Target Scheduler. A request may ask for the other runtime on any
     rig, to compare. Such a request is read-only, like every preview.
3. **Make availability an input of both runtimes** and of `night`. Marks are
   planning input only (§4.3).
4. **Split program assembly** into a read-only gather step, shared by the
   pull and the Director preview, and the pull's binding and issue. The
   preview's work list is then the pull's, not a second reading of Target
   Scheduler.
5. **Overrides and fixed steps are Director-only, and compile into the
   program.**
   - They become narrower goal windows and a night's order in the programs
     the rig pulls for that night. These are fields the program already
     carries, so the wire contract does not change. Geometry intersects
     the windows, so an override can only narrow what the rig may do.
   - A Target Scheduler rig picks its own work on the rig. PSF Guard could
     only change that by rewriting its Target Scheduler rows, and
     activation stays the only writer of those. Its preview stays a
     prediction: change the plan's priority, limits or rule weights, and
     preview again.
6. **Store availability marks and overrides in the meta store**, keyed by
   scope and night date, each with a compare-and-set revision. Meta backups
   carry them.
7. **Follow Target Scheduler's source, and keep its licence.**
   - The documentation leaves out or misstates behaviour a faithful
     emulation needs (§2.3), so `crates/ts-planner` follows the source
     closely, and N.I.N.A.'s rise-and-set fit for the Sun.
   - Both are MPL-2.0, so the crate is MPL-2.0 file by file, with the
     licence beside it, the way `src/ts_schema` keeps the vendored schema.
     The rest of PSF Guard stays Apache-2.0.
   - Check its choices against Target Scheduler's own Scheduler Preview
     (§4.5.1).

## 4. The model

### 4.1 Which rigs and which nights

A request names rigs, or none for every rig with a site; a start instant,
defaulting to now; and a number of nights, 1 to 31, defaulting to 7. As in
feasibility, the first night is the one under way at the start. Rigs without a
site are listed in `warnings`. A night is identified by the core's
`Night.date`, the evening's local date, which availability marks and
overrides use too.

Summaries are cheap, so every request gets them. Schedules are asked for: all
nights of the range, or the first N.

### 4.2 A rig's work

| | Target Scheduler rig | Director rig |
| --- | --- | --- |
| Plans | every active project of the rig's N.I.N.A. profile, as Target Scheduler picks them | the plans activated on the rig, as the pull reads them |
| Priority | each project's priority and rule weights | the rig's resolved project order and objective priority, compiled as the pull compiles them |
| Limits | the project's limits and the profile's horizon and flip settings, by Target Scheduler's rules (§2.3) | `applied_limits`, the rule feasibility uses |
| Moon | each template's Target Scheduler avoidance | each recipe's Moon rules, as `BoundGeometry` applies them |
| Darkness | each template's acceptable twilight and offset | the night's darkness |
| Exposure order | the project's filter switch frequency, smart exposure order, or the target's override order | `evaluate`'s choice of goal |
| Equipment | not checked: Target Scheduler runs what its rows say | the reported configuration, or a preview configuration (below) |

- **Summaries** use the same limit rules for both runtimes (§4.4), so a
  rig's summary does not change with its mode. Only schedules differ.
- **Preview configuration:**
  - A Director rig that has not reported its equipment is previewed with a
    configuration made from the filters, binning, readout modes, gains and
    offsets its recipes use.
  - It exists only so `BoundProgram` validates. It never leaves the server
    and is never issued.
  - With a reported configuration, recipes the rig cannot run drop out the
    way they would in the pull, and are listed.
- **Overheads:**
  - Both runtimes add the same estimates: a target change (slew and
    centre), a filter change, and dithering every N frames, on top of the
    pull's 15 s per exposure (`OVERHEAD_MS`). These are rig settings with
    stated defaults (§7, question 4).
  - The Target Scheduler emulation can turn them off to match Target
    Scheduler's own preview, which counts exposure time only.
  - Director's Phase 3 later replaces the estimates with learned
    durations. The local evidence already records preparation and capture
    times.
- **Acceptance:**
  - A planned frame counts at the plan's recent acceptance ratio on that
    rig, from its grades (1.0 when there are none).
  - The Target Scheduler emulation can instead count frames as Target
    Scheduler's preview does: accepted only when the project's grader is
    on.
  - The response states which rule it used.

### 4.3 Night availability

A mark covers one night in one scope:

| State | Meaning |
| --- | --- |
| `available` | Clear. At this scope it overrides any forecast. |
| `unavailable` | Nothing that night: clouds, rain, the rig down, nobody home. |
| `limited` | Clear only in the given windows, for example 22:30 to 02:00, or after 01:00. |

- **Scopes:**
  - A **site** mark is about the sky and applies to every rig whose
    planning site it is.
  - A **rig** mark is about the rig: maintenance, a camera away, an
    observatory closed.
  - A night's effective availability is the intersection of the two.
    A rig down on a clear night is out, and a cloudy site keeps every
    rig there out.
  - At each scope a manual mark wins over a forecast. With no mark and
    no fresh forecast, the night is available.
- **Windows:**
  - They are entered in local clock time and stored as UTC instants
    inside that night's noon-to-noon span, so a window can cross
    midnight.
  - A window outside the span is refused, and so is a `limited` mark
    without windows.
  - A mark may carry a short note ("dome motor", "high cirrus").
- **Ranges:** "the rig is down 3 to 9 November" writes one mark per night
  in one request and one transaction. Each mark can be cleared on its own
  later.
- **In the runtimes:**
  - `NightRequest` gains `available: Option<Vec<Interval>>`. `None`
    means the whole night, so existing callers do not change.
  - `Night` gains `available_dark_hours`, and `TargetNight` gains
    `hours_up_available`. The existing fields keep their meaning, so the
    Visibility strip changes only if it chooses to.
  - Director's `schedule` intersects every goal's windows with
    availability.
  - The Target Scheduler emulation treats time outside availability as
    a wait, the way Target Scheduler waits for a target to rise. Its
    planner calls never land there.
- **Planning only:**
  - Marks and forecasts never reach the program, the pull, the plugin,
    Target Scheduler or N.I.N.A., and never allow or block acquisition.
  - This follows Director's site rule: "A site does not grant permission
    to run equipment or turn a forecast into a safety guarantee." The
    rig's safety monitor and N.I.N.A.'s unsafe state still decide on the
    night.
  - On a Director rig, holding the rig for a night is a separate,
    explicit override (§4.6).
- **Forecasts (Phase 7):** see §4.3.1.

#### 4.3.1 Forecasts

A site can take a cloud forecast. It turns into the site's availability
wherever no manual mark says otherwise.

- **Providers.** Each site chooses one, or none (the default).

  | Provider | Clouds | Range | Coverage | Key | Terms |
  | --- | --- | --- | --- | --- | --- |
  | Open-Meteo (the default choice) | total, low, mid and high, hourly | up to 16 days | global; ECMWF, GFS and ICON blended, or one chosen | none | free for non-commercial use, 10,000 calls a day, CC BY 4.0; paid plans, and the service is open source, so it can be self-hosted |
  | MET Norway Locationforecast | total, low, medium and high, plus fog; hourly, then every 6 hours | about 9 days | global, most detailed over Scandinavia | none; requests must identify the app and a contact | free, commercial use included, CC BY 4.0; obey `Expires`, send `If-Modified-Since`, coordinates to 4 decimals |

  Each provider's base URL can be changed in the server's configuration,
  so a commercial user can point Open-Meteo at a paid plan or an instance
  of their own. Considered and left for later: the US National Weather
  Service (US only), 7Timer! ASTRO (adds seeing and transparency, but
  3-hourly, 3 days, non-commercial), keyed services (Pirate Weather,
  OpenWeatherMap, Tomorrow.io, Visual Crossing) and paid astronomy
  services (Meteoblue, Astrospheric).
- **Opt-in and privacy.** Fetching a forecast sends the site's latitude and
  longitude to the provider, and PSF Guard sends nothing about a site
  anywhere today. So forecasts are off until an editor turns them on for a
  site, which says what is sent and to whom. Coordinates go rounded to two
  decimals, about a kilometre, which is finer than any model's grid.
- **Clear or not.** An hour is clear when its total cloud cover is at or
  under the site's threshold (30 % by default). Clear hours inside a night's
  noon-to-noon span become that night's `limited` windows; a night with
  none is `unavailable`; a night clear in every dark hour is `available`.
  Weighting cloud layers (thin high cloud blocks less than low cloud) is a
  later site option.
- **Storage.** The meta store keeps each site's latest forecast: its
  provider, when it was fetched, when it expires, and the hourly cover
  (total and by layer). Windows are worked out when they are read, so a
  changed threshold applies at once.
- **Freshness.** A forecast is good for 12 hours after it was fetched. An
  expired forecast counts as no information, not as clear, and hours past
  a forecast's range are no information too. A night with no information
  is available, as before, and says so.
- **Refresh.** A background task refreshes each opted-in site's forecast
  every three hours, sooner never than the provider's `Expires`, with
  backoff after a failure. A failure keeps the last forecast with its
  expiry and never fabricates a clear night. **Fetch now** in the site's
  settings refreshes at once.
- **Precedence.** At each scope a manual mark wins over a forecast. A
  forecast is a site's: it is about the sky, so rigs take it through their
  planning site, and a rig's own mark still meets it.
- **Shown.** The Nights grid draws a forecast night like a marked one, with
  a cloud glyph; the night's panel adds an hourly cloud strip to the
  timeline, and every place a forecast shows credits its provider.
- **Observed weather** stays separate. It is acquisition evidence and
  belongs to the rig (Director, *Site and rig responsibilities*).

### 4.4 Summary per rig and night

For each rig and night:

- the date, dusk, dawn and dark hours, and the available dark hours with
  the mark that set them (scope, state, windows, source, note);
- the Moon's illumination and hours up in the dark;
- for each plan target:
  - hours up;
  - hours up while available;
  - hours its filters are Moon-blocked: hours up while available, less
    the part inside `moon::moon_windows` for each template's Moon rules
    (that call covers one night, which is all a summary needs);
  - the peak altitude and when it comes;
  - the hours the plan still owes on the rig;
- **the load:** owed exposure and overhead time that fits the night against
  the available dark hours. A night shows as overbooked, or as idle for so
  many hours.

The core's 16-target limit per `NightRequest` is met by splitting a rig's
targets into several requests over the same night.

### 4.5 Schedule preview

Both runtimes return the same shape: one schedule per night and a projection.

**Blocks** carry:
- the target (or panel), filter, exposure and frame count;
- the start and the end;
- why the block was chosen.

**Gaps** carry a reason:

| Gap reason | Target Scheduler emulation | Director |
| --- | --- | --- |
| `twilight` | no template's acceptable twilight allows imaging yet | before dusk or after dawn |
| `unavailable` | outside the night's available windows | the same |
| `moon` | every candidate's plans fail Moon avoidance | inside a goal's `non_lunar_windows` but not its `windows` |
| `meridian` | the no-flip zone, or outside a meridian window | the rig's meridian pause |
| `below_limits` | nothing is visible for its minimum time | under the horizon or outside the altitude limits |
| `complete` | every target is complete | every goal has its frames |
| `nothing_eligible` | candidates remain, but none is ready before the night ends | goals are owed, but none fits before the night ends |

**Projection:**
- Planned counts carry from one night to the next.
- Each goal and plan gets the night it completes, or a note that it does
  not complete within the range.
- Identical inputs give identical schedules, with caps on steps per night
  and on the range.

The preview shows the order of work and roughly when it happens. It does not
promise exact clock times (Director, *Shared planning core*). Uncertainty
bands follow once durations are learned.

#### 4.5.1 Target Scheduler emulation

- **Input:** `ts_planner::simulate` takes:
  - a snapshot read from the rig's database for one N.I.N.A. profile: the
    projects' limits, priority, minimum time, filter switch frequency,
    dither, smart exposure order, grader, mosaic and meridian window;
    targets and override orders; exposure plans and templates (twilight
    and offset, Moon avoidance, dither); filter cadences; rule weights;
    and the profile preferences that affect planning (slew and centre,
    park on wait, exposure throttle, smart plan window);
  - the profile's custom horizon and meridian flip settings;
  - the site, availability and overheads;
  - the range.
- **Loop:**
  - It runs the steps in §2.3 at each instant.
  - It starts at the night's first instant a template's twilight allows,
    within availability.
  - After each exposure, or wait, it calls the planner again from where
    that one ended, as Target Scheduler's Scheduler Preview does.
  - Dither state, filter cadence and the minimum-time stickiness carry
    from step to step, as Target Scheduler keeps them during a run.
- **Explanation:** a block's reason is the scoring at the step that started
  it: each ready candidate's rule scores and weighted total, and why the
  others were rejected (not visible for the minimum time, Moon, twilight,
  meridian, complete). This is the same account Target Scheduler's View
  Report gives.
- **Fidelity:**
  - Two modes: `estimate`, the default, follows a live run (10 s sampling,
    overheads, frames counted at each plan's acceptance rate); the
    `target_scheduler_preview` mode follows the Scheduler Preview (30 s
    sampling, exposure time only, frames accepted when the project grades).
  - The emulation names the Target Scheduler version it follows (5, as of
    the vendored schema version 23).
  - Golden fixtures recorded from Target Scheduler's own Scheduler Preview,
    on fixed databases and start times, pin its choices:
    - the same sequence of targets and exposures;
    - rule scores within 1e-6;
    - waits within a minute, since Target Scheduler times positions with
      N.I.N.A.'s astrometry and the core uses SOFA.
  - A Target Scheduler release that changes planning means new fixtures
    and a new version.
- **Weather:** Target Scheduler's maximum humidity needs a weather device,
  and its own preview ignores it. The emulation ignores it too, and uses
  availability marks instead.

#### 4.5.2 Director emulation

`schedule::simulate` takes:
- a bound program, from the shared gather step and a reported or preview
  configuration, with the night's overrides applied (§4.6);
- the rig's constraints;
- availability, overheads and the range.

1. **Bind** a `BoundGeometry` over the whole range, from the first night's
   noon to the last night's next noon, so windows and lunar windows are
   computed once.
2. **Step each night** from dusk, or the first available instant, to dawn.
   At each instant, ask `evaluate`, with the state safe, at a boundary, and
   the completion deadline at the end of the current available window.
   - **Acquire:**
     - start or extend a block for the goal;
     - advance the clock by the exposure and its overhead, plus the
       target-change or filter-change estimate when either changes;
     - add the frame to the goal's planned count.
   - **Wait for a future window:** jump to the earliest window start after
     the clock, among the goals still owed, and record the gap.
   - **Complete, or nothing feasible:** record the rest of the night as a
     gap.
3. **Steps:** each block expands into the steps the preparation reducer
   issues for it (unpark, centre and rotate, the `BeforeTarget` hook, filter
   and readout mode changes, dithers) and its exposures, each with its
   estimated duration.

A block's reason is the project's place in the night's order, the objective's
priority, and whether the Moon tie-break decided it. For a legacy weighted
program, it is the factor contributions from `priority::preview`.

### 4.6 Director overrides and fixed steps

These apply only to rigs in Director mode. On any other rig they are
refused.

| Override | Effect on that night's programs |
| --- | --- |
| `exclude` | a plan, target or goal gets no windows for the night, or loses the given window |
| `pin` | inside the given window, every other goal loses that window, so only the pinned goal (or target) may run there |
| `order` | a project order for the night, in place of the rig's |
| `hold` | the rig gets no work for the night, or after a given time |

- **Fixed steps:**
  - A night's fixed steps are an ordered list of pinned blocks ("M31 Ha
    from 21:00 to 23:00, then M33 L until 02:00"), and optionally a
    `hold` from a stop time.
  - In the editor a person makes them by dragging blocks on the Director
    timeline. They are saved as `pin` and `hold` overrides, so they need
    nothing further from the rig.
  - Steps the program cannot carry today, such as an autofocus run, a park
    at a given time or flats at dawn, wait on the program contract
    (§7, question 12).
- **Compilation:**
  - The pull compiles a night's overrides into the goals' allocated
    windows, subtracting from the allocation span, and into
    `Goal.priority` through the existing order compilation.
  - Geometry intersects those windows with its own, so an override never
    widens eligibility, extends an allocation, or relaxes a limit.
  - A pinned goal that geometry or the Moon rules out in its window
    leaves that window idle. The editor shows that before saving.
- **Live shifts:** an override saved while the night runs, from tonight's
  timeline on the Rigs page: hold the rig, pin the next stretch to a
  target, or move a block later. The preview re-plans the rest of the night
  from now with the rig's current counts, and the program the rig pulls
  carries the change at its next check-in (question 13).
- **Previewing:**
  - The Director emulation applies the night's overrides, so the
    timeline shows their effect.
  - An unsaved edit is previewed by sending it with the request. It is
    not stored.
- **Lifetime:**
  - An override belongs to one rig and one night, and lapses when the
    night ends.
  - Each has a revision. A change alters the program fingerprint, so
    programs issued after it carry it.
  - A session that is already running picks the change up at its next
    check-in, through the existing priority handoff: it finishes the
    exposure, parks and reconciles before taking the successor (§7,
    question 13).

### 4.7 Server

- **Gather:**
  - Split `program::assemble` into `gather`, read-only (activations,
    Target Scheduler rows, saved captures, project order, and the night's
    overrides), and the pull's binding to the reported configuration plus
    the program issue.
  - The pull's output must not change while there are no overrides: its
    program fingerprint before and after the split is a test.
- **Target Scheduler snapshot:** a read-only reader of the rig's database
  that builds `ts_planner`'s input. It shares the vendored schema's column
  checks, so an older schema missing a column takes Target Scheduler's
  default for it.
- **Night plans** (`src/server/director/night_plans.rs`):
  - `POST /api/director/v1/night-plans` takes `rigs` (optional),
    `start_ms`, `nights` and `schedule_nights`, plus `runtime` (`auto`,
    `target_scheduler` or `director`, default `auto`), `explain` (the night
    whose blocks carry every candidate and its scores), `fidelity` and
    `utc_offset_minutes`, and, with Phase 6, `override_draft` (unsaved
    Director overrides to preview). It returns,
    per rig:
    - the runtime used, and the site and applied limits per plan;
    - `nights` (summaries);
    - `schedules`, when asked for;
    - `projection` (the completion night per plan and goal);
    - `assumptions`: acceptance rule, overhead estimates, the emulated
      Target Scheduler version, the configuration used;
    - plus `warnings` (rigs without a site, recipes or rows left out and
      why).
  - `GET /api/director/v1/rigs/{rig}/night-plan`, the same for one rig, is
    not built: the POST with one rig in `rigs` does it.
  - Rigs run on the rayon pool. Within a rig, nights run in order,
    because of carry-over.
  - The handler reads the meta store under a read slot, then lets it go
    before reading the rigs' databases and computing, as feasibility
    computes in its request. A month of 30 targets takes about 1.4 s. If
    real catalogs take much longer, the schedules move to a tracked job
    that the page polls.
  - Read access is the same as feasibility's. Nothing is written.
- **Availability:**
  - `GET /api/director/v1/night-availability` takes `scope`, `id`, `from`
    and `to`, and returns marks with revisions.
  - `PUT /api/director/v1/night-availability/{scope}/{id}` takes one night
    or a range of nights, the state, windows and note, and
    `expected_revision`, and writes them in one transaction.
  - `DELETE` on a night returns it to the default.
- **Overrides:**
  - `GET`, `PUT` and `DELETE /api/director/v1/rigs/{rig}/night-overrides/{date}`
    carry the night's override list, with `expected_revision`.
  - They return `409` for a rig not in Director mode.
- **Writes:** availability and overrides need editor access, and a
  mismatched revision refuses the whole request.
- **Meta schema:** each migration only adds tables.
  - Schema 33 (Phase 2) adds `night_availability`, keyed by scope, scope
    id, night date and source, with state, note, revision and update
    time, and its windows in a child table.
  - The next schema (Phase 6) adds `night_override`, keyed by rig and
    night date, with its ordered entries and revision.

### 4.8 Interface

- **Nights:**
  - A tab of the **Rigs** page, beside **Live**. Both are tables of rigs, so
    they share one page, which the header's Rigs tab opens on Live. That
    tab carries the fleet's status the separate rigs chip used to show; the
    Sky keeps the rigs on its map. Live's **Tonight** column opens tonight
    on Nights.
  - Rigs are rows and nights are columns, for all rigs or a chosen set,
    starting at any date. Each row says which runtime it shows.
  - Each cell shows the available dark hours as a bar, the mark (✕
    unavailable, ◐ limited, ✓ marked clear), the Moon's phase, and the
    load (the colours of the plans planned that night, plus idle or
    overbooked time).
  - On a phone the grid scrolls inside its own box. The page itself
    never scrolls sideways.
- **A night:**
  - The night under way is planned from now with the rig's current counts
    (`plan_from_ms`, default now), so each refresh re-plans the rest of it.
    Its panel puts the rig's live status beside the schedule and marks now
    on the timeline. This is where Director overrides and live shifts will
    be made (§4.6).
  - Choosing a cell opens that rig's timeline from dusk to dawn: blocks
    per target and filter, drawn over the altitude chart
    `VisibilityPanel` already draws, with the gaps labelled by reason.
  - On a Target Scheduler rig:
    - the timeline is read-only;
    - a block's tooltip gives the candidates' rule scores;
    - a note says Target Scheduler decides on the rig, and that changing
      the plan's priority, limits or rule weights changes the
      prediction;
    - **Compare with Director** shows what Director would do.
  - On a Director rig:
    - blocks expand into their steps;
    - **Edit night** allows pinning a block by dragging it, excluding a
      plan or target, reordering the night's plans, and holding the rig;
    - each edit previews before it saves.
- **Marking:**
  - From a cell, or a range of cells, choose Available, Not available or
    Limited (from–to), for the site (every rig there) or for this rig
    only.
  - A marked night shows the mark and its note everywhere it is drawn.
- **Plans:**
  - The plan workspace shows the projected completion night per rig.
  - Feasibility's `nights_to_complete` takes availability and the rig's
    other plans into account once Phase 5 lands.
  - The Visibility strip's nights table shows the marks.

## 5. Phased plan

Each phase ends with its gate passing, the user docs (`docs/DIRECTOR.md`)
describing what was built, and this record updated. Target Scheduler rigs
come first, since nearly every rig is one today.

**Phase 1: summaries for all rigs or chosen rigs.** *Built.* The summary
also counts the hours each target's owed filters' Moon rules allow, which
`NightTarget.moon` carries into the core.
- In the core, add `available` to `NightRequest` and the two new fields,
  each tested with a limited window, a window across midnight, and an
  unavailable night.
- In the server, add the night-plans summary route, reusing feasibility's
  limit resolution, and chunk the targets.
- In the interface, add the Nights grid without timelines.
- *Gate:* for each plan on a rig, the rig's summary equals that plan's
  feasibility numbers under the same limits.

**Phase 2: manual availability.** *Built*, with marks of a site and of a
rig, ranges by Shift-click, and limited hours entered as from and to times.
- Meta schema 33 (`night_availability`), the availability API and marking
  in the grid. Summaries apply the marks.
- *Gate:*
  - Marks round-trip, and compare-and-set refuses a stale revision.
  - An unavailable night contributes no hours.
  - A limited window clips hours to the minute.
  - Site and rig marks intersect.

**Phase 3: the Target Scheduler emulation.** *Built, without golden
fixtures.* Behaviour tests cover twilight, the minimum-time stickiness, the
switch penalty and priority, the meridian window, Moon avoidance,
availability, the filter cadence, dithering and completion, and a preview
writes nothing to the rig's database. Fixtures recorded from Target
Scheduler's own Scheduler Preview need a N.I.N.A. machine and are still to
do.
- `crates/ts-planner`, the snapshot reader, and timelines with rule scores
  in the interface.
- *Gate:*
  - Golden fixtures from Target Scheduler's own Scheduler Preview match:
    - covering a mosaic, smart exposure order, an override order, a
      meridian window, the no-flip zone, Moon avoidance with relaxation,
      and Moon must be down;
    - and each scoring rule at a non-default weight.
  - A preview writes nothing to the rig's database.

**Phase 4: the Director emulation.** *Built.* The core's `schedule` binds
a `BoundGeometry` per night (the lunar windows cover one day at most) and
steps from dusk to dawn at −12°: Director has no darkness rule of its own
yet. A wait gap is labelled Moon or below limits; telling a meridian pause
apart needs the geometry to say why, and is left for later.
- The core's `schedule` module, the `gather` split, the preview
  configuration, and Director timelines with their steps.
- *Gate:*
  - Golden schedules for fixed programs and constraints.
  - At every simulated instant, the step's decision equals
    `BoundGeometry::evaluate` on the same state.
  - The pull's program fingerprints are unchanged.
  - Every gap carries a reason from the table in §4.5.

**Phase 5: projection.** *Built.*
- Carry-over across nights, the acceptance rules and completion nights for
  both runtimes.
- Feasibility counts availability: its nights take the rig's marks and its
  site's forecast (`rig_availability`, shared with night plans), each night
  carries its marks for the Visibility table, and `nights_to_complete`
  counts each night's hours up and available, nights past the range as
  clear.
- Contention: `POST /projects/{id}/projection` reads, for each rig the plan
  is activated on, the night its own runtime's schedule plans the plan's
  last frame, and the other plans that take the rig first. The Visibility
  panel shows it. Feasibility's arithmetic estimate stays contention-free
  and says so.
- *Gate, met:* `adoption::night_plans` projects a plan alone on a rig to the
  night its planned frames reach what it owes, then ranks a high-priority
  plan ahead of it and sees that night move later.

**Phase 6: Director overrides and fixed steps.** *Not built.*
- `night_override` in the next meta schema, the overrides API,
  compilation into the pull, and editing on the Director timeline.
- *Gate:*
  - Every override narrows the program: no goal's windows or priority
    range widen, and a fuzz test over random override lists holds this.
  - The pulled program and the preview agree on what runs in a pinned
    window.
  - A Target Scheduler rig refuses overrides.
  - A night's overrides lapse after it.

**Phase 7: forecast sources.** *Built.*
- A provider per site (Open-Meteo or MET Norway, off by default), the
  stored hourly forecast, the background refresh, expiry, manual marks
  overriding forecasts, the cloud strip and attribution (§4.3.1).
- *Gate, met:* an expired forecast counts as no information. A manual mark
  overrides the forecast at its scope. A failed request keeps the stored
  forecast until it expires and never makes up a clear night. Tests:
  `server::forecast` reads each provider's response, sends rounded
  coordinates and identifies PSF Guard; `adoption::forecasts` covers a
  failed first request, the forecast's marks, a site mark and a rig mark
  over them, a changed threshold, expiry and turning forecasts off;
  `night_plans` judges forecast nights by their dark hours; the Director
  e2e suite runs against a stand-in provider.
- Built beside it: tests that a site's `.hrz` horizon reaches both runtimes'
  schedules (`adoption::site_horizon`), checked locally against real patio
  horizons as well.
- Left for later: weighting cloud layers, and the US National Weather
  Service and 7Timer! ASTRO as further providers.

**Later:**
- learned durations and uncertainty bands (Director, Phase 3);
- a saved snapshot of a night's plan, to compare planned and actual;
- the plugin's **Tonight** showing the preview;
- a read-only MCP tool for night plans.

When Phases 4 and 5 land, tick Director's Phase 3 items for virtual-clock
simulation and for the per-rig night preview through the shared engine.

## 6. Invariants

- A night plan writes nothing. Availability and overrides write only their
  own tables.
- A Target Scheduler rig's database is never written by a preview, a mark
  or an override.
- Each runtime is emulated from what it reads on the rig. The Target
  Scheduler emulation never reads Director's settings, and the Director
  preview's selection is `BoundGeometry::evaluate`, so there is no second
  Director scheduler.
- Availability never reaches dispatch, the program, the plugin, Target
  Scheduler or N.I.N.A.
- Overrides exist only on Director rigs, and only narrow a program's windows
  or reorder its goals. They never widen eligibility, extend an allocation
  or relax a limit.
- Without overrides, the pull's output is the same before and after the
  `gather` split.
- Identical inputs, emulated versions and engine version give identical
  plans.

## 7. Open questions

1. **Runtime before the rig mode exists.** Proposal: Director for a rig
   with reported equipment and an enabled workload policy, and Target
   Scheduler otherwise. Once the rig mode is built, it decides.
   *Settled: built as proposed, until rig modes exist; overrides follow
   the same rule, and a Target Scheduler rig refuses them.*
2. **N.I.N.A. profiles.** One database can hold projects for several
   profiles, and Target Scheduler plans per profile. Proposal: plan the
   profile the rig profile records. A database with several profiles and
   none recorded is planned per profile, each as its own labelled lane.
   *Provisional: the profile that owns most projects, as activation picks
   it, with a warning naming the active projects of other profiles left
   out. Lanes per profile are not built.*
3. **Target Scheduler versions.** Proposal: emulate version 5 as of schema
   23, show the version in the response, and add fixtures when a release
   changes planning. Older schemas take Target Scheduler's defaults for the
   columns they lack.
4. **Overheads:** per-rig settings with defaults, or fixed constants until
   durations are learned? Proposal: rig settings beside the planning
   limits, defaulting to 15 s per exposure, 90 s per target change and 10 s
   per filter change, used by both runtimes. *Provisional: those values,
   plus 15 s per dither, as constants; no settings yet.*
5. **Acceptance in the Target Scheduler emulation.** Proposal: the plan's
   acceptance ratio by default, with Target Scheduler's own preview rule
   (accepted only with the grader on) as the fixture mode. *Provisional:
   built as proposed; a plan's rate counts once it has 5 frames acquired.*
6. **Limited hours:** windows, or just a number of hours? A number cannot
   place blocks. Proposal: windows, with "first N hours" and "after HH:MM"
   shortcuts that fill them in. *Provisional: one window per mark, entered
   as from and to times; an empty end means dusk or dawn.*
7. **Should an unavailable rig mark do more?** Proposal: no. Marks stay
   planning only. On a Director rig, the night's editor offers to add a
   `hold` override as a separate, explicit step.
8. **Beyond the forecast:** completion estimates for nights past any
   forecast assume clear skies. Proposal: an optional clear-night fraction
   per site and month, used only to stretch completion projections, never
   timelines.
9. **Time zones:** nights are keyed by local mean time from the site's
   longitude, and sites have no time zone yet (Director lists it as still
   required). Proposal: add an optional IANA time zone to the site profile,
   used to enter and show marks and overrides, falling back to the
   browser's zone. *Settled: as proposed, for Phase 6. Until it lands,
   marks are entered and shown in the browser's clock; the Target Scheduler
   emulation plans its days by the site's local mean time, which a request
   can override.*
10. **Darkness in summaries:** feasibility counts dark below −12°, while
    Target Scheduler uses each template's twilight level. Proposal: keep
    −12° for summaries, and let each runtime's schedule use its own rule.
    *Provisional: built as proposed.*
11. **Forecast provider:** one with hourly cloud cover, no key and fair terms
    of use. *Settled:* Open-Meteo by default and MET Norway beside it, with
    changeable base URLs (§4.3.1).
12. **Steps the program cannot carry:** an autofocus run, a park at a given
    time, flats at dawn. Proposal: leave them out of v1. Add them when the
    program contract and the plugin grow those operations.
13. **An override saved mid-night:** should it reach a running session at
    its next check-in through the priority handoff, or only the next
    session? Proposal: the next check-in, with the editor saying when it
    takes effect. *Settled: as proposed.*

## 8. Out of scope

- The plugin's wire contract. Overrides reach a Director rig only through
  the goal windows and priorities programs already carry.
- What activation writes, and any write to a Target Scheduler rig's database.
- Coordinated multi-rig allocation (Director, Phase 5). Each rig's schedule
  uses that rig's own work. A plan shot by two rigs adds up their
  projections and does not share one deficit between them.
- Safety. Marks and forecasts never allow or block acquisition.
- Moving targets.
