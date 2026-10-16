# Background jobs

Status: **Current contract** for new long or compute-heavy work. Older jobs
follow most of it; the gaps are listed at the end.
Last updated: 2026-10-15

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
| The job line | `static/src/components/header/activityItems.ts`, polled by `useHeaderActivity.ts` |

## A minimal job

Modelled on `src/server/flat_measure.rs`.

```rust
#[derive(Default)]
struct Job {
    queue: Mutex<Queue>,            // merged requests, retries, what Stop paused
    status: RwLock<MyJobStatus>,    // what the status route answers
    cancelled: AtomicBool,
}
static JOBS: LazyLock<Mutex<HashMap<String, Arc<Job>>>> = LazyLock::new(Default::default);

/// Called by a trigger (an import, a sync, a setting). Never does the work.
pub fn queue(state: &Arc<AppState>, path: &str, scope: Scope) {
    let job = job(path);
    let mut queue = job.queue.lock().unwrap();
    queue.pending.merge(scope);
    if std::mem::replace(&mut queue.worker, true) {
        return; // the runner serves this catalog already; it sees the merge
    }
    let (state, path) = (Arc::clone(state), path.to_string());
    // The job's one runner. Its claim resets on a panic (`RunnerClaim`).
    std::thread::spawn(move || serve(&state, &path, &job));
}

fn step(state: &AppState, job: &Job, frame: &Frame, priority: Priority) -> anyhow::Result<()> {
    // The one yield point: a person's work first, and the job line shows
    // this job as waiting meanwhile.
    state.wait_for_interactive_work(POLL, |waiting_for| {
        job.update(|s| s.waiting_for = waiting_for.map(str::to_string))
    });
    // Leased for this step alone.
    let budget = concurrency::plan_workers(None, &state.worker_policy(), priority, Some(pixels));
    let lease = state.lease_workers(priority, budget.workers);
    let pool = ComputePool::take("my-job", lease.workers)?;
    pool.install(|| measure(frame));
    Ok(())
}
```

Then:

- a status route, and Stop and Resume routes when the job can stop between
  steps. Stop pauses; it does not forget;
- work a pass could not finish (files away, a busy catalog) goes back in
  line with a growing wait, not to the next restart;
- a query in `useHeaderActivity.ts` that polls the status (fast while it
  runs, slower while it only waits, a slow heartbeat at rest when the server
  starts the job on its own), shared with any page that shows it, and an
  item in `activityItems.ts` with a title, scope, progress, `automatic` and
  a `control` when it can be stopped;
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
- Backups (`backup_job.rs`) run one at a time server-wide rather than one per
  catalog. A scheduled run is `Background` and yields between files and
  SQLite steps; a run a person starts does not wait. Stop ends the run
  rather than pausing it: the schedule tries again at its next time.
- Calibration grading (`calibration_quality.rs`) is a Background runner per
  catalog with a status route, but it has no row in the job line yet.
- Calibration health (`calibration_health.rs`), quality backfill and scans
  (`spatial_scan.rs`), preview pre-generation (`preview_queue.rs`), stack
  builds (`stack_preview.rs`) and WBPP runs (`wbpp_queue.rs`) predate this
  record; check each before copying its shape.
