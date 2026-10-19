# One job line (proposed)

Status: **Proposal**, for review before any code moves.
Last updated: 2026-10-19

This record proposes one shared shape for the server's queued work. It
covers two things that grew separately: the line that runs stack builds,
color compositions and processing runs, and the per-catalog jobs that
[Background jobs](background-jobs.md) describes. It keeps the parts of
that record that work, the tiers, worker leases, the interactive guard and
stop holds, and replaces what each job now writes for itself.

## Why

### The stack line

Stack builds, color compositions, processing runs and re-applies share one
worker and one ordered line. That line lives inside `StackPreviewManager`
(`src/server/stack_preview.rs`, about 9,800 lines). The manager has 18
fields that cover four separate concerns:

- **Job records**: mono builds, color compositions and artifact searches, each
  in its own map.
- **The line**: the `waiting` list, the worker permit, the quick-lane
  permit, the turn notifier, and the held turn for a person's re-apply.
- **The journal**: what each queued job was asked for (`origins`) and the
  snapshot a server stop takes.
- **Separate lines for other work**: processing runs, re-applies, and the
  per-catalog preparation line.

The line has no single runner. Each queued build spawns a tokio task that
waits on a semaphore, and `wait_turn` checks the line, takes the permit and
checks the line again. Each new rule since has become a case of its own:

| Rule | Where it lives now |
|---|---|
| A person's re-apply runs right after the build it follows | `turn_held_until`, a timed hold on the next turn (#932) |
| A build being prepared keeps its place but does not block the line | `first_in_line` skips `Preparing` slots; prepared builds take the slot's place (#938) |
| A color composition waits for its target's channels | `waiting_for_channels` on the preparation line, released by every build that ends (#941, open) |
| A short composition runs beside a build | a second permit, the quick lane |
| A build a person asks for interrupts automatic work | `cancel_automatic` and `interrupt_automatic` |

Each of these is a version of the same rule: this entry cannot run until
something else has happened. They do not compose. For example, a re-apply
that waits for a build that is itself waiting for preparation is handled
by a hold timer, a slot, and a release hook working together.

### The per-catalog jobs

[Background jobs](background-jobs.md) defines a job by a template that each
module copies. Six modules define the same `static JOBS:
LazyLock<Mutex<HashMap<String, Arc<Job>>>>`. Six files write their own
`RunnerClaim`, the guard that frees a runner when it panics. Each module
also has its own queue merge, status shape, and Stop and Resume handling. The header's job line has
eleven row builders in `activityItems.ts`, each fed by its own status query.
Stack builds, WBPP runs, preview pre-generation, quality scans and
calibration health predate the template and follow none of it.

## What stays

- The tiers (`concurrency::Priority`), `plan_workers` and `lease_workers`,
  `ComputePool`, `begin_named_interactive_job` and
  `wait_for_interactive_work`, and `shutdown.hold`.
- Every route and what it answers: `202` with a job being prepared,
  `replaced_by`, job IDs a page follows, queue positions, **Move to top**,
  and Stop.
- The journal file (`<registry>.jobs.json`). New entry fields keep serde
  defaults, so an older journal still reads.
- What a job does: preparing frames, stacking, composing, processing. Only
  where it waits and who runs it moves.

## The line

A **line** is an ordered list of entries with one runner per lane. The
stack line is the first; the WBPP line can be the second.

### Entries

Each entry has:

| Field | Meaning |
|---|---|
| `id` | The ID a page follows. It never changes for the life of the entry. |
| `kind` | Stack build, color composition, processing run, re-apply. |
| `tier` | `Interactive` when a person asked for it, otherwise `Viewed` or `Background`. |
| `lane` | `Worker` for the shared stacking worker, or `Quick` for a composition that runs beside it. |
| `state` | See [States](#states). |
| `waits_on` | What must happen first. See [Waiting](#waiting). |
| `stop` | Its stop flag. |
| `journal` | What it was asked for, as the journal stores it. |

### States

```text
preparing ──► queued ──► running ──► completed
    │            │          │      ├─► failed
    │            │          │      └─► cancelled
    └─► replaced (by the entries it became)
```

An entry is in its place in the line from the moment it is asked for. While
`preparing`, its catalog's preparation runner reads what it needs. When
preparation finishes, the entry becomes one or more queued entries in its
place, or is queued itself under the same ID. A failed preparation ends the
entry with the reason.

### Kinds

A kind implements a small trait. The line calls it and does nothing else
with the kind:

```rust
trait LineWork: Send + Sync {
    /// Read what the work needs, off the request and off the runner. May
    /// become other entries (one per channel), or wait on others.
    fn prepare(&self, ctx: &PrepareContext) -> Result<Prepared, String>;
    /// Do the work on the lane's runner, stopping at `stop`.
    fn run(&self, ctx: &RunContext, stop: &AtomicBool) -> RunOutcome;
    /// What the journal keeps, and what the header lists.
    fn journal(&self) -> JournaledStackJob;
    fn activity(&self) -> StackActivityEntry;
}
```

The stack code keeps `prepare_job`, `prepare_channels`, `run_job`,
`prepare_color_job` and `compose_color`. They move behind this trait rather
than being rewritten.

### Waiting

`waits_on` replaces the rules in the table under [Why](#why):

- `Entries(ids)`: the entries named have ended. A person's re-apply is queued
  with the build it follows and waits on that build. It sits right behind
  the build, so no hold timer is needed.
- `Target { database, project, target }`: no entry of that target's channels
  is preparing, queued or running. A color composition waits on this. The
  line evaluates it whenever an entry ends, so nothing needs releasing by
  hand.
- `Preparation`: the entry is still being prepared. This is the current
  `Preparing` slot, as one case of the same rule.

The runner takes the first entry in its lane that waits on nothing. Waiting
entries keep their places. The header shows what each entry waits for,
using the same text it shows now.

### Runners

Each lane has one runner thread. It takes the first ready entry, holds the
interactive guard when the entry's tier is `Interactive`, leases workers for
each step, and runs the kind's `run`. This replaces the task-per-build and
the permit races in `wait_turn`. A person's entry still interrupts automatic
work: the runner stops the running automatic entry at its checkpoint. Today
the stopped build is cancelled and its refresh queues it again later. In the
line it would go back in its place instead, which is a change of behavior
for step 2 to decide.

Preparation keeps one runner per catalog. A long preparation in one catalog
then does not hold up another, as now.

### Journal and restart

The line writes each entry's `journal()` as the journal writer does now. A
restart puts every entry back the same way a click adds it: in its place,
under its ID, as `preparing`. Retries for storage that is not mounted yet,
the two-restart limit, and the stop snapshot stay as they are, moved into
the line.

## Per-catalog jobs

The per-catalog jobs get a small runner of their own instead of the copied
template:

```rust
struct CatalogJobs<Q: Merge, S> { /* one entry per catalog */ }

impl<Q: Merge, S> CatalogJobs<Q, S> {
    /// Merge `scope` into the catalog's queue; start its runner if idle.
    fn queue(&self, state: &Arc<AppState>, catalog: &str, scope: Q, serve: ServeFn<Q, S>);
    fn status(&self, catalog: &str) -> Option<S>;
    fn stop(&self, catalog: &str);
    fn resume(&self, catalog: &str);
}
```

It holds the runner claim, the stop flag, and the "visible only once it found
work" rule. It also holds the retry with a growing wait. Flat measuring,
which follows the template exactly, moves first. Duplicates, reject
removal, frame sites, calibration quality and calibration health follow.

## The header

Later, one route lists every running and queued job as uniform rows: ID,
kind, tier, state, scope, detail, progress, place in line, what it waits
for, and the controls it offers. The eleven row builders become one. This
is the largest frontend change, so it comes last and is optional.

## Migration

Each step is its own PR. Each keeps behavior unless it says otherwise.

1. **Move the line out of `stack_preview`.** A new `src/server/job_line/`
   holds ordering, reorder, first-in-line, stop flags, the journal hooks
   and the stop snapshot. `StackPreviewManager` keeps the job records and
   calls the line. Existing tests pass unchanged.
2. **One runner per lane.** Replace the task-per-build and `wait_turn` with
   the lane runner. Gate: the full Rust suite, the stack e2e specs, and
   WebKit in CI.
3. **`waits_on`.** Fold the held turn, the preparation slot and the color
   wait into it, and remove `turn_held_until` and `release_waiting_colors`.
   A person's re-apply is queued with its build.
4. **`CatalogJobs`.** Add it, then move flat measuring onto it, then the
   other five, one PR each.
5. **WBPP as a second line.** Its queue and journal entries move onto the
   same machinery, with PixInsight as its single lane.
6. **The header route** (optional).

Steps 1 and 2 give most of the benefit. Step 3 depends on step 2.

## Not covered

- Preview pre-generation (`preview_queue.rs`) is a bounded pool of
  short tasks, not a line. It stays as it is.
- The automatic refresh scheduler decides *what* to queue and when. It
  stays separate and queues entries like any other caller.
- WBPP's module notes say a restart drops its queue, but the journal now
  keeps it. Step 5 corrects the notes.

## Open questions

- Should `Target` waits also wait for an automatic refresh that is still
  settling for that project, so a composition does not start just before
  new frames are stacked?
- Should a person's entry move ahead of automatic entries when it joins the
  line, rather than relying on the interruption alone?
- Should `Quick` stay a lane, or become a rule the worker lane applies,
  "may run beside the current entry"?
