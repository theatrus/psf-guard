# Background jobs

Status: **Current contract** for new long or compute-heavy work. Older jobs
follow most of it; the gaps are listed at the end.
Last updated: 2026-10-18

A task that reads many frames, measures pixels or runs for more than a
moment is a **job**. A job:

1. has one runner per catalog, which merges the requests made while it
   waits or runs;
2. runs at a [`Priority`](../../src/concurrency.rs) tier (below);
3. plans and leases its workers from the shared budgets;
4. reports its progress on a status route, and shows as a row in the
   header's job line while it works.

What a job must not do:

- start threads beyond its one runner thread, or loop in
  `tokio::task::spawn_blocking`;
- do the work inside an HTTP handler. A handler queues the job and returns;
  a preview answers `202` while its work is queued;
- read the whole catalog on the shared request connection. Name the rows
  the change touched, or read on the job's own connection;
- pick a fixed worker count.

## Three tiers

| Tier | Who | Workers | Guard | Waits for |
|---|---|---|---|---|
| `Interactive` | A person started it: a stack, a preview, a scan they asked for | The interactive share | Holds `AppState::begin_named_interactive_job(name)` | Nothing |
| `Viewed` | Automatic work a page shows now, as flat measuring for the rig Calibration › Frames is open on | Borrowed from the background share, so the cores in use stay at the interactive and background shares | None | Interactive work |
| `Background` | Automatic work nobody is looking at: pre-generation, quality backfill, grading, flat measuring after an import | The background share, split between background jobs | None | Interactive work |

Only a person's work holds the guard, so nothing a page merely shows can
hold up a stack, a preview, autoimport, grading, backfill or an automatic
stack. A `Viewed` job leases again for each step and gives the workers back
between steps, so a stack started meanwhile plans with its whole share. It
does not wait out `RESUME_QUIET` after a person's work, as automatic stack
refreshes do: a page is showing what it measures.

A page lifts a job with a hint, not by polling harder: Frames sends `POST
…/calibrations/flat-measure/watch {rig}` every 20 s while one rig is chosen.
For 45 s after each, that rig goes first and runs `Viewed`. The hint takes
the gate of the other display calculations: it may queue a check, which
writes measurements, but never changes what the catalog records of frames
or grades. A rig the catalog does not record is ignored.

## The parts

| Part | Where |
|---|---|
| Tiers | `concurrency::Priority::{Interactive, Viewed, Background}` |
| Worker count for a step | `concurrency::plan_workers(None, &state.worker_policy(), priority, frame_pixels)` |
| Workers from the shared budget | `AppState::lease_workers(priority, wanted)`, returned when the `WorkerLease` drops |
| Threads Seiza runs on | `concurrency::ComputePool::take(name, lease.workers)`, then `pool.install(..)` |
| A person's work | `AppState::begin_named_interactive_job(name)`: a guard, named for what waits on it ("a stack build") |
| The yield point | `AppState::wait_for_interactive_work(nap, note)` before each step: blocks while a person's work runs, and after `WAIT_NOTE_AFTER` (1 s) tells `note` what it waits for, so the job line lists the job as waiting |
| A quiet moment | `AppState::interactive_quiet_for()`: how long no person's work has run, as automatic stack refreshes wait `RESUME_QUIET` (30 s) before they resume |
| Holding a stop until a write commits | `state.shutdown.hold(what)` |
| The job line | `GET /api/jobs` (`src/server/jobs.rs`) builds every row; `useHeaderActivity.ts` polls it and `activityItems.ts` maps each row to the header's item |

## The job line

The header's background jobs chip is the job line, kept small: a progress
ring, the overall percent and up to four icon counts, **running** (▷),
**queued** (a numbered list: waiting for a turn), **waiting**
(an hourglass: work stepped aside for other work) and **needs attention**
(a warning triangle: work stopped until a person acts). Each count names
itself for a screen reader and in its tooltip, and shows only above zero.

Its list groups the rows by tier, each heading with its own counts:

- **Started by you**: `Interactive` work. Stack builds and processing runs,
  with the re-applies that follow a person's build, scans and duplicate
  checks a person asked for, WBPP runs, a catalog refresh asked for with
  Refresh, and backups and transfers started by hand.
- **Automatic**: `Viewed` and `Background` work. Quality backfill and the
  analysis of new frames, automatic scans, flat measuring and frame sites,
  catalog refreshes the cache's expiry or the server's start began,
  automatic stack refreshes and the re-applies they bring, scheduled
  backups and transfers a connection's policy queued.

Within each, rows that need attention come first, then running work, then
queued rows, then waiting work. A row's tier comes from the job's own
status: `automatic` on a scan, a stack entry, a catalog refresh or a
transfer; `priority` on a duplicate check; `trigger` on a backup. Quality
backfill, flat measuring and frame sites are always automatic.

### The jobs route

`GET /api/jobs` lists every job the header shows as uniform rows, in the
order above. The server reads each job's in-memory status, the same state
the job's own status route reads, and builds the row's title, scope,
progress text, percent, tier, place in its line, what it waits for, the
control its Stop and move buttons call, and a structured link to the view
of its work. The server resolves a target's project for the link, so the
client needs no target list. The route never waits for a database's
shared connection or plans work:

- The project of a target is cached; an uncached one is read only when
  the shared connection is free. Until then, the row links to the
  database's images.
- The channels a waiting stack refresh would restack come from a cache
  that the refresh scheduler fills on its own thread. A waiting refresh
  that is not planned yet is listed without them.
- The project name of a waiting refresh is left out while the shared
  connection is busy.
- The duplicate check's `stale` test stays in its own route.
- The route waits at most half a second for the Director's transfer list,
  and otherwise lists the transfers of the last poll.

The route also returns each database's `busy` and `scanning` state and its
last scan's frame errors. The header uses them to read a database's views
again when its work finishes, and to keep a scan's error note. Backups and
Target Scheduler syncs are listed only for an editor on a server that
allows database management, as their own status routes are.

The header polls the route every second while it lists a job and every
5 s otherwise. Code that starts, stops or moves a job invalidates the
`['jobs']` query, and every mutation does so when it settles, so a new job
shows at once.

The stack line and the WBPP line are each one line, shared by both tiers:
an automatic refresh's build can wait ahead of a person's. So a queued row
says its place in the whole line and how many wait before it, such as
`queued · 4 · 3 ahead` or `queued · 1 · next`, whichever heading it sits
under. ↑ and ↓ move it within the whole line, past rows under the other
heading. **Move to top** on a queued task under **Started by you** moves it
straight to the front of its stack/processing or WBPP queue, past both manual
and automatic tasks. The task runs after the current one finishes. The button
is disabled when the task is already next or a queue change is in progress;
read-only users cannot change the queue.

Short color compositions started by a person use a separate queue and can run
alongside the stack worker, one composition at a time in request order.
`enqueue_color_job` selects that queue when the prepared inputs are cached, or
when it needs no deconvolution or RC-Astro and no other composition is
preparing the same inputs. Automatic compositions and those that need new
deconvolution or RC-Astro results use the stack queue. A
**Composing color** row in the separate queue has no build-queue position or
reorder controls; while queued, it shows **Waiting for the composition ahead**.
It does not count as the job holding the stack worker. Restored color jobs use
the same selection rule. See [Color previews](../STACKING_PREVIEWS.md#color-previews-from-channel-stacks).

A row adds what waits behind the step under way when the server knows it
and the detail does not already say it: `1 more rig to check · 2 rigs
queued` on flat measuring, `another pass queued` on frame sites, `3
channels to stack` on a waiting stack refresh. Flat measuring reports
`rigs_after`, the rigs the running pass checks after the current one (each
may turn out to have nothing to measure), and `next_pass`, the rigs asked
for since the pass began or whose retry is due, which the next pass takes;
a retry not yet due is left out. Frame sites reports `next_pass` as a
flag. A number the server cannot stand behind, such as frames left in a
build whose units also count calibration, registration and rejection, is
left out.

Waiting is normal: automatic work steps aside whenever a person's runs.
It is drawn muted, with `--state-waiting-*` (an hourglass and a quiet
edge), never in the warning colour. `--state-attention-*`, the warning
colour, marks the rows that need a person: a transfer held for review,
before upload or after it failed. A transfer waiting for its retry, or
paused, waits. Scan errors keep their own note above the list.

## A minimal job

A job with one runner per catalog implements `CatalogWork`
(`src/server/job_line/catalog.rs`) and stores its jobs in a
`CatalogJobs`. Modelled on `src/server/flat_measure.rs`:

```rust
pub(crate) struct MyJob;

impl CatalogWork for MyJob {
    type Queue = Queue;             // merged requests, retries, what Stop paused
    type Status = MyJobStatus;      // what the status route answers
    type Item = Scope;              // one pass
    const RUNNER: &'static str = "my-job";
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = Some("my job");

    fn next(_: &AppState, _: &str, queue: &mut Queue, now: Instant) -> Step<Scope> {
        queue.take_due(now)         // Step::Run, Step::Sleep or Step::Exit
    }

    fn run(state: &Arc<AppState>, path: &str, job: &CatalogJob<Self>, scope: Scope, stop: &Stop) {
        for frame in frames(path, &scope) {
            if stop.requested() || state.shutdown.is_stopping() {
                break;
            }
            step(state, job, &frame, Priority::Background);
        }
    }

    fn runner_failed(queue: &mut Queue, status: &mut MyJobStatus) {
        status.last_error = Some("my job stopped unexpectedly".into());
    }
}

static JOBS: CatalogJobs<MyJob> = CatalogJobs::new();

/// Called by a trigger (an import, a sync, a setting). Never does the work.
pub fn queue(state: &Arc<AppState>, path: &str, scope: Scope) {
    // Starts the catalog's runner thread unless one exists; a running one
    // reads the merged scope at its next step.
    JOBS.request(state, path, |queue| queue.merge(scope));
}

fn step(state: &AppState, job: &CatalogJob<MyJob>, frame: &Frame, priority: Priority) {
    // The one yield point: a person's work first, and the job line shows
    // this job as waiting meanwhile.
    state.wait_for_interactive_work(POLL, |waiting_for| {
        job.update(|s| s.waiting_for = waiting_for.map(str::to_string))
    });
    // Leased for this step alone.
    let budget = concurrency::plan_workers(None, &state.worker_policy(), priority, Some(pixels));
    let lease = state.lease_workers(priority, budget.workers);
    if let Ok(pool) = ComputePool::take("my-job", lease.workers) {
        pool.install(|| measure(frame));
    }
}
```

`CatalogJobs` resets the runner state when the thread panics or cannot
start, gives each item a fresh `Stop` that `CatalogJob::stop_running`
requests, and makes shutdown wait for an item only when
`SHUTDOWN_WAITS_FOR` is set.

Then:

- a status route, and Stop and Resume routes when the job can stop between
  steps. Stop pauses; it does not forget;
- work a pass could not finish (files away, a busy catalog) goes back in
  line with a growing wait, not to the next restart;
- a row builder in `src/server/jobs.rs` with a title, scope, progress,
  `automatic`, `background` for its tier, `remaining` when the status says
  what is left, and a `control` when it can be stopped, plus a matching
  stop call in the header's `ActivityChip.tsx`. A page that shows the job
  polls its status route on its own;
- a status that says whether a person started the run, and what waits
  behind the step under way (see [The job line](#the-job-line));
- a status that turns visible only once the job has found work, so a pass
  with nothing to do never flashes in the job line.

## Jobs today, and the gaps

- Flat measuring (`flat_measure.rs`) follows all of it.
- Reading frame sites (`frame_sites.rs`) follows it for one catalog-wide
  scope: queued at start, after imports and hourly when frames marked away
  come due, retried while the server runs when the volume is away or a pass
  fails, Stop and Resume (a note on Calibration while stopped), a row in
  the job line, a wait for interactive work before each chunk, and what it
  placed handed on at most every 45 s.
- The duplicate check and repair (`calibration_duplicates.rs`) follow it.
  - A check or repair a person asks for runs at `Interactive`: it holds the
    guard and leases the interactive share for its file checks.
  - The check the page starts on its own, when it has no plan or a stale
    one, runs at `Background`. It waits for a person's work and is listed
    as waiting meanwhile. A person asking for a check lifts it.
  - Each run reads and writes on connections of its own, and shows in the
    job line with Stop.
  - A run that ends frees the runner in the same step that records its end,
    so a page that sees the end can start the next run at once.
- Reject removal (`reject_removal.rs`) follows it: one runner per catalog
  for Preview, Remove, Restore and Empty trash, each a person's and so
  `Interactive`, holding the guard. It reads on a connection of its own,
  shows in the job line with Stop, and stops between frames, keeping the
  frames done.
- Backups (`backup_job.rs`) run one at a time server-wide, on one
  `CatalogJobs` runner under the fixed key `backups`; each server state has
  its own `CatalogJobs`, so it is one runner per server. A scheduled run
  is `Background` and yields between files and SQLite steps; a run a person
  starts does not wait. Stop ends the run rather than pausing it: the
  schedule tries again at its next time.
- Target Scheduler imports and syncs (`ts_sync.rs`) run on `CatalogJobs`:
  one runner per catalog for its sync, and one for imports, which run one
  at a time. An import is a person's and holds the guard; a sync waits for
  a person's work. Stop sets the job's `stopping`, which every step checks.
- Catalog upkeep (`catalog_upkeep.rs`) runs on `CatalogJobs`, one runner
  per catalog: PSF Guard's query indexes and the readout modes of older
  calibration frames, asked for at start and whenever a database is added
  or changed, and calibrated-copy pairing after each directory refresh. Its
  row appears while it builds indexes, reads readout modes that are
  missing, or pairs copies.
- A database's cache refreshes (`catalog_refresh.rs`) run on `CatalogJobs`,
  one runner per database: the file-cache refresh the header lists as the
  database's refresh, and the rebuild of a stale folder index while
  requests use the stale one. A context reaches its server through its
  `ServerLink`; a context no server works for, such as in a unit test,
  keeps a thread of its own.
- Imports (`import_job.rs`) that write run on `CatalogJobs`, one runner per
  database, with a row and Stop, which rolls the transaction back. An
  import preview is a preview, not a job: see
  [Previews](job-line.md#previews-the-newest-request-wins).
- Exports into the export folder (`export_job.rs`) run on `CatalogJobs`,
  one runner per database, as a person's work, with a row and Stop, which
  ends the export before its next file.
- Preview pre-generation (`preview_pregeneration.rs`) runs on
  `CatalogJobs`, one runner per database, at `Background`. A timer
  requests a pass for every database at start and every five minutes. A
  pass lists the images whose enabled previews are missing or past the
  cache expiry and renders them in batches on leased workers, building
  each image's cache paths again from its current row. Images not yet
  started step aside for a person's work, and the pass waits before its
  next batch; it ends when the cache volume nears its limit. A pass with previews to write has a row with Stop;
  after Stop, no pass runs for that database until the server restarts.
- A star catalog installation (`catalog_install.rs`) is one download
  server-wide. It holds the server stop's wait, ends when the server stops,
  and its row has Stop.
- The director's planning resends (`director/remote_push.rs`) run once per
  peer when a site answers. They hold the server stop's wait, and the
  header lists them as automatic work.
- Calibration grading (`calibration_quality.rs`) is a Background runner per
  catalog. Its row in the job line appears once a pass has frames to
  measure; a pass that only regrades the library is quick and gets none.
- Calibration health (`calibration_health.rs`) runs on `CatalogJobs` like
  grading. Its row appears once a pass has projects to judge.
- Quality scans and the database-wide analysis (`quality_scans.rs`) share
  one runner per catalog, since they share the database's scanner. A
  person's scans run first, at `Interactive`, in the order requested. The
  analysis then scans its targets one at a time at `Background`. A person's
  request stops the analysis's current target scan at its next frame and
  puts that target back at the front of the analysis. The runner gathers
  the frames that need computing, so the start route only checks that the
  target exists and returns at once.
- Calibration grading also waits on `CatalogJobs` for remote uploads to
  pause ([`Settling`](../../src/server/job_line/catalog.rs)) before its
  pass, and counts as busy meanwhile, so flat measuring waits for it.
- The scan of newly arrived frames (`quality_arrival.rs`) is one job per
  database: arrivals settle for two minutes, then the job queues the
  database-wide analysis for the targets with unmeasured frames. A
  database already scanning, or a catalog it cannot read, is looked at
  again after 30 s. The frames it queued stay out of the next scans for a
  day, in the job's own state.
- The Calibration page's project scope (`calibration_scope.rs`) is one
  runner per catalog. It refreshes scopes first and reads the light
  headers the health pass has not kept in chunks of 32 between them, so a
  scope grows as headers arrive. A project's read stops once no view has
  asked for its scope for a minute. Its row, **Reading light headers**,
  is a person's: a view asked for it.
- What the flat groups feed (`flat_group_use.rs`) is one runner per
  catalog, with a growing wait after a failed refresh. Its row, **Working
  out what the flat groups feed**, is automatic.
- Flat measuring, frame sites, the duplicate check, reject removal,
  calibration grading, calibration health, quality scans, arrivals, the
  calibration scope, flat group use, catalog upkeep, backups and Target
  Scheduler syncs all run on `CatalogJobs`. Their state lives in their
  job, not in statics of its own.
- Automatic import (`autoimport.rs`) is a schedule rather than a queue: a
  timer looks every 30 s at which databases are due and starts the
  ordinary import job, which has its own row. Every timer runs through
  `schedule::every`; see [Timers](job-line.md#timers). Its records and the lights
  its runs left out live on `AppState`.
- The storage budget (`cache_budget.rs`) keeps its volume limits, last
  reports and cache sizes on `AppState` (`CacheBudget`). The limits a
  person sets are a process-wide setting, applied from the registry like
  the calibration and stacking settings.
- The preview queue (`preview_queue.rs`), which renders a preview someone
  opened, is a bounded pool of short tasks and stays as it is. So does the
  director's sky-image generation, which renders on demand for display
  like an image preview.
- Stack builds and WBPP runs wait in job lines rather than per-catalog
  queues; see [One job line](job-line.md).
