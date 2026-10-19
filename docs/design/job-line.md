# One job line

Status: **Steps 1 to 3 built** for the stack line; the rest is proposed.
Last updated: 2026-10-19

This record proposes one shared shape for the server's queued work. It
covers two things that grew separately: the line that runs stack builds,
color compositions and processing runs, and the per-catalog jobs that
[Background jobs](background-jobs.md) describes. It keeps the parts of
that record that work, the tiers, worker leases, the interactive guard and
stop holds, and replaces what each job now writes for itself.

The line must also be able to send work to another machine one day: a
stack, an RC-Astro chain or a composition run on a worker with more cores,
a GPU or a licence this server lacks. Remote work is not built here. The
shape is chosen so that adding it later changes who runs an entry, not what
an entry is. See [Remote work](#remote-work).

## What is built

`src/server/job_line/` holds the stack line. Stack builds, preview
re-renders, color compositions and processing runs all go through it:

- **The line owns the work.** An entry is its id, its lane, its `Stop`, and
  a `Submission`: the work's spec, serialized to JSON when it joins, with
  what the coordinator does once it has run (`publish`) or was stopped
  before it ran. Running it reads the spec back, as a worker on another
  machine would, so every build exercises that path.
- **One dispatcher per lane.** `JobLine::next` hands a lane's dispatcher the
  first entry that may run, with a `Lease` on the lane, or an entry stopped
  while it waited, to settle without running. The choice and the lease are
  taken under one lock. The dispatcher runs the work on this server's
  worker in a blocking task and does not wait for it, so an entry stopped
  while a long build runs is settled at once.
- **Held places.** A request still being prepared holds a place
  (`hold_place`) that runs nothing and holds up nothing behind it. The work
  it becomes takes the place, or goes just before it.
- **A last step that always runs.** A submission's `finally` runs however
  the entry ends, even when publishing or settling it panicked, and is told
  which (`Ending`). It forgets the stop flag, releases compositions waiting
  on the build's channels, leaves the quick lane and ends the server stop's
  wait, so a panic cannot leave a build in the journal or a composition
  waiting.
- **Reservations go first.** Work that takes the worker outside the line,
  such as an artifact search or removing calibration masters, is counted
  while it waits, and the dispatcher leaves the lane to it, as the old
  semaphore's first-come order did.
- **Nothing hands out a bare `Arc`.** Stop flags are `concurrency::Stop`;
  an answer to whoever awaits a piece of work is a `job_line::Reply`, staged
  while the work's last steps run and delivered after them; the line's
  wake-ups stay inside it, and callers say only what changed
  (`JobLine::changed`).

The queue a person sees is unchanged: the same order, positions, **Move to
top**, Stop, quick lane, and held turn for a person's re-apply.

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
| `needs` | What a worker must offer to run it: cores and memory, a GPU, PixInsight or an RC-Astro licence, and read access to its inputs. |
| `assigned_to` | The worker running it and its lease, while it runs. |
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

A kind implements `LineWork`:

```rust
trait LineWork {
    /// Everything the work needs, prepared by the coordinator.
    type Spec: Serialize + DeserializeOwned + Send + 'static;
    /// What the work made, for the coordinator to publish.
    type Output: Send + 'static;
    /// The worker it can run on.
    const NEEDS: Needs; // AnyWorker or ThisServer
    /// Run the work from its spec, on the worker, with its lane's lease held.
    fn execute(spec: Self::Spec, worker: &Worker<'_>) -> Self::Output;
}
```

Preparation happens before the work joins the line, as it did: the build
set's preparation runner, a color composition's preparation, or a
processing request's checks. Publishing is the `Submission`'s callback,
run on this server with the lease still held, since publishing can prune
files the next build would read.

| Kind | Spec | Needs | Why it still needs this server |
|---|---|---|---|
| Stack build | `PreparedJob`: frames, groups, method, order | `ThisServer` | It opens the calibration catalog to choose and build masters, reads the WCS and score caches, and records each channel's progress on the build |
| Preview re-render | The build's id and its stale channels | `ThisServer` | It reads the build's FITS from the stack cache and records progress on the build |
| Color composition | `PreparedColorJob`, with its plate solve as plain fields | `ThisServer` | It reads channel stacks from the stack cache, records progress, and writes the color index itself |
| Processing run | The run's identity, config, source and RC-Astro chain | `ThisServer` | It renders through the processing caches and writes the channel's selection |

A build's run now ends with its outputs; publishing the manifest, the
latest index, the re-apply, color after its channels and the cache prune
moved out of the run. The other kinds still write some of their own
results while they run.

### Requests and specs

Two things are serialized, for different reasons:

- **The journal keeps the request**: what was asked for. A restart prepares
  it again from the catalog as it is then, since frames, grades and
  calibration may have changed while the server was down.
- **The line carries the spec**: what to run now. It is a snapshot, right
  for handing to a worker and wrong as the record a restart rebuilds from.

The line could carry only the request too, and prepare it when its turn
comes, so a spec could never go stale while it waits. Build stacks would
then list its channel builds only once it starts, rather than at once.
That is left open.

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

### Coordinator and workers

The **coordinator** owns the line: order, waiting, stop flags, the journal,
and every write to the catalog and the stack cache's indexes. It assigns
each ready entry to a **worker**. A worker takes a spec, runs `execute`,
reports progress, and hands back outputs. It never reads the catalog or
changes the line.

Today there is one worker: this server, in process, with a dispatcher per
lane. It holds the interactive guard while a person's entry runs, and the
work leases cores for each step as before. This replaced the task each
build used to spawn to wait for the worker, and the permit races in
`wait_turn`. The lease has no expiry yet: a local worker cannot vanish
without the server going with it.

Assignment works the same whether the worker is local or remote:

- The coordinator offers the first ready entry whose `needs` a free worker
  meets. Entries a worker cannot run wait for one that can, and keep their
  places.
- The worker **claims** it with a lease. The lease names the entry, the
  worker and an expiry. The worker renews it with each progress report.
- An expired lease puts the entry back in its place as queued, with one
  more attempt counted, as a restart counts one now.
- Stop is a message on the lease. The worker stops at its next checkpoint
  and hands back what it has, as a stopped build does now.

A person's entry still interrupts automatic work: the coordinator stops the
automatic entry at its checkpoint, the stopped build is cancelled, and its
refresh queues it again later. Putting it back in its place instead would
change behavior; step 2 kept it as it was, and it is still open.

Preparation keeps one runner per catalog on this server. A long preparation
in one catalog then does not hold up another, as now.

### Journal and restart

The line writes each entry's `journal()` as the journal writer does now. A
restart puts every entry back the same way a click adds it: in its place,
under its ID, as `preparing`. Retries for storage that is not mounted yet,
the two-restart limit, and the stop snapshot stay as they are, moved into
the line.

## Remote work

Not built by this proposal. It shapes the steps that are.

A remote worker is a process on another machine that registers with the
coordinator, says what it offers, and asks for work. The boundary is the
`execute` part of a kind:

- **Isolation.** A worker receives a spec and short-lived, scoped access to
  the inputs that spec names, for example signed URLs for its frames and
  masters. It gets no catalog, no API token, and no access to other
  entries. Its outputs go to its own scratch directory and are uploaded by
  digest.
- **Trust.** The coordinator checks each output against its digest and
  shape before `publish`. It publishes atomically and only for the lease
  that is current. A late result from an expired lease is dropped. A worker
  can waste time but cannot change the catalog.
- **Assignment.** The coordinator matches `needs` to what workers offer:
  cores and memory, a GPU, a PixInsight or RC-Astro licence, and whether the
  worker can read the inputs where they are or must have them sent. It can
  weigh the bytes to send against waiting for this server.
- **Priority.** Tiers carry over. A remote worker has no interactive guard,
  so a person's entry preempts automatic work by stopping it at its
  checkpoint, as above.
- **Checkpoints.** A stopped build's checkpoint (`resume.rs`) is an output
  like any other. It is uploaded, so the build can resume on another worker
  or on this server.
- **Progress.** Workers report progress on their lease. The header shows a
  remote entry like a local one, with where it runs.

Questions for that work, not this one: how workers authenticate (the
collaboration auth in [AstroCollab and Starfront
interoperability](collaboration.md) may fit), how inputs move when the
worker cannot read this server's storage, and which kinds go first.
RC-Astro chains and WBPP are likely, since they need a licence or a machine
this server may not have.

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

1. **Move the line out of `stack_preview`.** *Built.*
2. **The coordinator and one local worker.** *Built:* the line owns its
   entries and a dispatcher per lane runs them.
3. **Split each kind into prepare, execute and publish.** *Built* for the
   spec, which every kind serializes and reads back on each run, and for
   the build's publishing. Still to do before any kind can leave this
   server: report progress through the worker rather than the build's
   record, move the color index and the processing selection writes into
   publishing, and, for builds, choose calibration masters while preparing
   rather than while running.
4. **`waits_on`.** Fold the held turn, the preparation slot and the color
   wait into it, and remove `turn_held_until` and `release_waiting_colors`.
   A person's re-apply is queued with its build.
5. **`CatalogJobs`.** Add it, then move flat measuring onto it, then the
   other five, one PR each.
6. **WBPP as a second line.** Its queue and journal entries move onto the
   same machinery, with PixInsight as its single lane.
7. **The header route** (optional).
8. **Remote workers** (later, its own design record): registration,
   scoped input access, upload by digest, and assignment by `needs`.

Steps 1 and 2 give most of the cleanup. Step 3 is what remote work needs,
and keeps the server's own work honest about its inputs even if remote
work never comes. Step 4 depends on step 2.

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
- Should the in-process worker go through the same lease and message path as
  a remote one, at a small cost, so the remote path is tested every day?
  This record assumes yes.
