# PSF Guard Director: goal-driven acquisition

Status: phase 0 in progress. Shared-core/native interop spike implemented;
no acquisition integration or production Director plugin yet.
Last updated: 2026-09-25.

This is the tracking document for Director. Update the phase checklist and
record implementation PRs here as work lands. Keep durable architecture here;
move delivered user workflows into focused guides rather than retaining a
completed implementation checklist indefinitely.

## Purpose

PSF Guard Director is a new N.I.N.A. plugin that pursues PSF Guard observing
goals through Target Scheduler (TS). It is not a downloaded schedule player.
Autofocus, centering, weather, and equipment delays change what is feasible;
Director must make useful local decisions from actual conditions and progress.

PSF Guard owns the full project lifecycle: define objectives, allocate work,
simulate opportunities, acquire, evaluate quality and calibration, request
replacement data, process, and assess completion. A project can span different
telescopes, sites, catalogs, and eventually collaborating PSF Guard instances.

The existing PSF Guard Sync plugin remains a separate supported product.
Director must not require users to migrate away from Sync or change existing
sync semantics.

## Architectural decisions

- Introduce a PSF Guard meta database for coordination above per-rig databases.
- Make global projects independent of rigs, database files, and local TS IDs.
- Exchange goals, constraints, and bounded assignments, not fixed timetables.
- Run the same planning core in PSF Guard and Director.
- Use TS as the execution adapter in Director-controlled sessions; retain its
  existing planner for ordinary TS sessions.
- Keep equipment control, safety, and operator overrides local to N.I.N.A.
- Start with one coordinating instance per project. Federation is a later phase,
  not multi-master editing of the same plan.

## Domain model

| Entity | Responsibility |
| --- | --- |
| Global project | Desired result, targets, objectives, participants, lifecycle, and outputs. |
| Observation objective | Required coverage, bandpass, exposure purpose, quality, depth, resolution, or cadence. |
| Site | Location, horizon, availability, and site-specific constraints. |
| Rig | Stable equipment identity, capabilities, and versioned configurations. |
| Contribution plan | How a particular rig can satisfy an objective, including framing, panels, recipes, and quality requirements. |
| Assignment | Versioned, bounded authorization for an executor to pursue specified contributions. |
| Contribution | Captured data, provenance, assessment state, and its relationship to objectives. |
| Execution event | Durable evidence of an operation, capture, decision, or state transition. |

A global project can map to multiple rig-local TS projects. Persist those
mappings by stable identifiers. Do not join by project name, telescope name,
nearby coordinates, or database-local integer IDs. Existing TS GUIDs remain
intact. Database slugs remain URL scope, not federation identity.

A rig identity outlives a catalog file. Record equipment and site configuration
revisions so a changed camera, telescope, or location does not rewrite history.
Historical catalogs may contain several configurations; do not require a
destructive split to adopt Director.

Short and long exposures through the same filter are separate objectives when
they serve different purposes. Recipes include duration, filter mapping,
binning, gain, offset, and readout mode where supported. Resolve capabilities
explicitly; do not silently substitute filters or unsupported settings.

Different rigs need not produce interchangeable subs. A wide-field contribution
and a high-resolution central region can serve one project but require separate
stacks. Project membership never grants stack compatibility. Completion must
measure required coverage and quality, not just add integration hours from
different instruments and assume equal depth.

## Storage and authority

| Store | Owns |
| --- | --- |
| Meta database | Global projects and objectives, sites, rig capabilities, mappings, contribution plans, assignments, permissions, and progress projections. |
| Per-rig catalog databases | TS-compatible acquisition history, image records, grades, calibration records, and local evidence. |
| Director local state | Cached assignments, execution journal, unsent events, recovery state, and operation timing observations. |

Start with a separate coordination SQLite database, provisionally named
`psf-guard-meta.sqlite`, alongside existing registered catalogs. This is a new
domain, not a repurposing of the catalog registry or the current merged Overview.
Keep PSF-owned coordination tables out of TS-owned schema. Decide the precise
sidecar layout for Director state during the execution spike.

Meta is authoritative for intent and allocation. Originating rig records and
execution journals describe what happened. Grades and calibration assessments
retain explicit provenance and the existing documented directional sync rules;
the meta progress projection is not another independently editable grade store.

Each projection records source revisions or event cursors and freshness. It must
be rebuildable without double-counting mirrored image records. Copies of a
capture share its identity; a new copy is not a new contribution.

Do not hold TS SQLite transactions open during network calls. Use short local
transactions and durable outbox/inbox processing, not cross-database distributed
transactions. Each owning service accesses its own databases. Remote instances
exchange scoped API messages, never open another instance's SQLite file.

Adoption is opt-in. Existing catalogs and Sync endpoints continue to work
without a meta database. Define backup, restore, and schema migration behavior
before production coordination data is stored.

## Shared planning core

Preferred implementation: a standalone Rust planning crate used directly by
PSF Guard and through a small native interface by the C# Director plugin.
Validate that packaging and interop boundary early; it is not yet implemented.
Do not maintain parallel Rust and C# versions of the scheduling algorithm.

The core has no HTTP, SQLite, N.I.N.A., or TS dependencies. Hosts provide inputs
and execute outputs. Time and randomness are explicit inputs, not hidden global
state. Identical versioned inputs and engine versions must yield identical
decisions, subject to a defined cross-platform numeric policy tested in CI.

Inputs include objectives, allocation limits, rig configuration, current time,
equipment state, visibility and horizon constraints, conditions with freshness,
progress and pending assessments, duration estimates, and execution state.

Outputs include continue, acquire, switch target, request calibration, wait,
check in, or finish. Every decision includes its reason and relevant constraints
so the operator can understand what happened.

Central allocation and local execution are distinct decision levels. Share
capability matching, constraint evaluation, progress accounting, duration models,
and scoring. Do not force both levels into an identical scheduling loop.

PSF Guard simulation advances a virtual clock using estimated operation
durations. Director supplies actual events and elapsed times. Both exercise the
same decision logic. Forecasts show uncertainty rather than promising exact
clock-time playback. Persist engine and duration-model versions for replay.

The native boundary needs versioned messages, explicit memory ownership, error
handling, compatible architecture packaging, and a clear unsupported-version
state. No native failure may silently turn into permission to acquire.

## Execution and check-ins

```text
Global objectives and rig capabilities
  -> coordinator allocates bounded goals
  -> Director validates, caches, and acknowledges an assignment
  -> shared engine selects useful local work
  -> TS executes through N.I.N.A.
  -> actual results update local state and duration estimates
  -> engine reevaluates at execution boundaries
  -> check-in reconciles progress and revises allocation
```

An assignment specifies eligible objectives, quantities or other completion
limits, constraints, approved alternatives, validity, offline policy, and
checkpoint policy. It is an observing program, not a list of timestamps.

For example, unexpectedly slow autofocus can leave insufficient visibility for
a planned block. Director reevaluates after autofocus and continues, shortens
the block, or chooses another authorized objective. It does not race to catch
up with a stale schedule or create unauthorized work.

Check in at session start, target or block completion, allocation exhaustion,
safety recovery, significant condition changes, and configurable periodic
intervals. A server priority notification requests a check-in; it is not an
unbounded hardware command. Ordinary captures queue events without blocking
the image-save path on HTTP.

Apply revisions at explicit safe boundaries. Distinguish proposed, cached,
acknowledged, and active revisions. Ordinary reprioritization normally lets the
current exposure or indivisible operation finish; local safety can interrupt
immediately. A local operator can always stop acquisition.

On connection loss, continue only within the cached assignment's limits and
offline authorization. At expiry, do not start new acquisition; define safe
completion of an in-flight operation. A disconnected rig cannot acknowledge
revocation. The coordinator must not reallocate its outstanding work until
release is acknowledged or authorization has safely expired. Specify clock
skew handling, reconnect reconciliation, and limits on unavoidable overlap.

Use stable operation and capture IDs with idempotent event delivery. After a
restart, reconcile saved files and TS history before retrying uncertain work.
Do not claim exactly-once hardware execution across a crash. Surface ambiguous
outcomes and handle late events from old revisions without losing real captures.

## TS and N.I.N.A. boundary

Baseline discovery identified N.I.N.A. 3.3 nightly and TS's 3.3 branch as the
starting point. Recheck and pin exact source/package versions in phase 0;
nightly branch heads and published releases are not interchangeable.

Add a supported optional plan-provider/execution interface to TS. Preserve its
default planner unchanged. The initial source inspection found a read-oriented
REST surface, not a sufficient acquisition-control API. Do not drive this by
replacing its live database, reflecting private methods, or copying its entire
execution loop.

In Director-controlled mode, Director's shared engine owns work selection; TS
translates approved work into its established execution machinery. N.I.N.A.
retains hardware, guiding, autofocus, flips, safety, cancellation, and image
saving. Respect native sequence hooks and locally configured safety policies.

Expose a Director session container and focused sequence actions for check-in,
progress reporting, and session completion where useful. Show current goal,
operation, assignment revision, next checkpoint, connectivity, and reasons for
waiting or switching. Avoid an indefinite generic "Working" status.

Director has a distinct plugin identity, configuration, credentials, queues,
and release flow from Sync. Detect competing acquisition controllers. Define
ownership for shared sync/upload duties so coexistence does not create duplicate
uploads or competing planning writes. TS compatibility remains a public contract,
including stable GUIDs, schema variation, grade values, and RA unit conversion.

## Operation timing and observability

Record autofocus, slew, centering, filter change, dither and settle, exposure,
download, flip, and other significant sequence timings. Each observation includes:

- Operation and parent/correlation IDs, assignment revision, and outcome.
- UTC timestamps for correlation and monotonic elapsed duration for measurement.
- Rig/configuration revision and relevant context, such as filter, autofocus
  method, or slew distance.
- Retry, timeout, interruption, and failure status.

Handle nested and overlapping operations explicitly. Do not add autofocus time
twice because it also belongs to a target-start operation. Keep image transport
timing separate from acquisition overhead unless it actually blocks acquisition.

Learn distributions rather than only averages, with configurable defaults for
new equipment and uncertainty for sparse samples. Distinguish failed/censored
attempts from successful durations. Version estimates, prevent stale equipment
history from dominating, and select conservative estimates near hard windows.

Persist detailed observations locally, synchronize them incrementally, and
define retention and summary policies. Log decisions and structured transitions
with correlation IDs, without credentials. Expose queue depth, last successful
check-in, current operation duration, and recoverable errors to the operator.

## Quality, calibration, and processing loop

Track started, saved, cataloged, pending assessment, accepted, rejected, and
processing-ready states separately. Pending work reserves part of an objective
under a bounded policy so delayed grading does not cause runaway acquisition.
Rejection can reopen a deficit, but retries need limits and an operator-visible
reason to avoid repeating an impossible objective all night.

Invalid flats can invalidate calibration coverage and authorize replacement
flats through the existing calibration/TS flat-history contracts. Distinguish
suspect evidence from confirmed invalidation. Keep calibration records separate
from TS light-frame acquisition records and preserve original files.

Processing retains recipes, calibration provenance, and output relationships.
Define objective completion separately from readiness to process and finished
project output. Project-level views aggregate contributions without silently
combining incompatible instruments or exposure purposes.

Control/progress and image transport are independent. Deferred end-of-night
uploads are valid. Missing remote pixels must appear as unavailable evidence,
not proof that acquisition failed or that an image passes quality requirements.

## Federation and collaboration

Each participating instance may have its own meta database, but each shared
project initially has one authoritative coordinator. A remote participant keeps
control of its equipment and only accepts assignments within local policy.

Use explicit invitations and scoped permissions for project metadata, contribution
submission, grading, original-image access, and acquisition authority. Sharing
a project must not share stored credentials or expose unrelated catalogs.
Retain contributor attribution, assessment provenance, and audit history.

Treat remote plans as untrusted input. Validate capabilities and allowed
templates locally; do not accept arbitrary executable code or unrestricted file
paths. Pairing, token rotation, revocation, and transport must preserve existing
security guarantees. Immediate revocation cannot be promised while offline;
bounded authorization defines that risk.

## Phased delivery

Phase 0 is in progress; later phases are pending. A phase is complete only when
its acceptance gate passes and its review and validation evidence is linked here.

### Phase 0: compatibility and execution spike

- [ ] Pin current N.I.N.A. 3.3 nightly and TS 3.3 versions; audit custom TS fixes
  before porting them and record the supported version matrix.
- [ ] Prove the TS execution extension while retaining ordinary TS behavior.
- [ ] Prove the shared Rust core loads and returns decisions in PSF Guard and
  a minimal C# plugin, including native packaging and error handling.
- [x] Implement a deterministic core linked into the PSF Guard Rust library and
  replay shared vectors through a native library from a .NET 10 console host.
- [ ] Decide crate ownership, interop format, local state layout, and contracts.

Gate: one simulated target/exposure runs through the supported adapter; the
same recorded input yields matching server/plugin decisions. No Sync changes
are required to run existing workflows.

#### Phase 0 evidence and remaining work

Implementation review: [shared-core and native interop spike, PR #460](https://github.com/theatrus/psf-guard/pull/460).

Baseline checked on 2026-09-25:

| Component | Inspected baseline |
| --- | --- |
| N.I.N.A. | 3.3 NIGHTLY #58; NuGet `NINA.Plugin` `3.3.0.1058-nightly`. |
| TS | Upstream `release/nightly-3.3`, commit `65478b96c52b47d4860e781c0249799cac2749e1`; source assembly version `5.10.4.0`. |
| TS dependencies | `net10.0-windows7.0`, NINA package `3.3.0.1037-nightly`; compatibility with #58 is not yet runtime-tested. |
| Local .NET SDK | `10.0.302`. |

TS's `TargetSchedulerContainer.Execute` constructs `Planner` directly, then
creates a private `PlanContainer` and executes it. The next TS change must
introduce an optional provider at work selection while preserving event hooks,
history, image-save observation, cancellation, and the default planner. Auditing
and porting local TS modifications remains pending. No TS source or installed
N.I.N.A. plugins are changed by the shared-core spike.

The initial implementation lives in
[`crates/director-core`](../../crates/director-core/src/lib.rs) and
[`crates/director-ffi`](../../crates/director-ffi/src/lib.rs). PSF Guard re-exports
the core as `psf_guard::director`; the native library calls the same evaluator.
There is no server route, database migration, or hardware dispatch yet. The
[.NET harness](../../tools/director-interop/Program.cs) is a console interop test,
not a N.I.N.A. plugin or a substitute for the required simulator session.

The experimental input is a host-assembled decision snapshot, not the eventual
signed/authorized assignment API. It includes goal progress and estimates for
convenience; production allocation records must remain separate from mutable
observations. Hosts are responsible for authenticated allocation, durable attempt
reservation, capture deduplication, and state/assignment revision checks before
dispatch. Calling the evaluator twice does not reserve two exposures or mutate
progress. A `continue` decision permits only the existing indivisible operation
to finish; it never authorizes another exposure.

The spike chooses the highest-priority feasible goal, with stable ID tie-breaking.
It uses host-supplied windows and estimates, not a new astronomy implementation.
Window and assignment ends are exclusive start bounds; an exposure may finish
exactly at the end. Exposure plus blocking overhead must fit both windows.
Timestamps are nonnegative Unix milliseconds and durations are milliseconds;
all arithmetic is checked integer arithmetic. Safety remains continuously
enforced by the host, not only when this evaluator runs.

Both Rust and .NET consume the same golden decision vectors, including slow
autofocus, pending grades, retry limits, safety, expiry, wrong-rig state, and
invalid protocol input. The C ABI uses caller-owned UTF-8 buffers, a version
probe, bounded input, and explicit status codes. See the
[C header](../../crates/director-ffi/include/director.h).

Run from the repository root on Windows:

```powershell
cargo test --locked -p psf-guard-director-core -p psf-guard-director-ffi
cargo clippy --locked -p psf-guard-director-core -p psf-guard-director-ffi --all-targets -- -D warnings
cargo build --locked --profile director -p psf-guard-director-ffi
dotnet run --project tools/director-interop --configuration Release -- target/director/psf_guard_director_ffi.dll crates/director-core/tests/fixtures/decisions.json
```

The dedicated `director` Cargo profile unwinds panics so the native boundary can
report an internal error. Ordinary `--release` builds of the FFI crate are
rejected because PSF Guard's application release profile aborts on panic.
Invalid native pointers, allocation failure, or process termination cannot be
made recoverable by this wrapper; no plugin installation is authorized by this
spike. Packaging and host-failure safety still require review before deployment.

The Director CI workflow runs Rust checks and the .NET/native vectors on
Windows, Linux, and macOS. Only local Windows results are evidence until those
hosted jobs pass. The existing application remains the default Cargo workspace
member, so normal application builds do not package the experimental native DLL.

### Phase 1: meta database and global project model

- [ ] Add opt-in meta storage, migrations, backup/restore, and stable mappings.
- [ ] Model sites, rig configurations, objectives, recipes, and contribution plans.
- [ ] Link existing catalogs without rewriting TS history or merging names.
- [ ] Add scoped project views that distinguish global and rig-local projects.

Gate: one project references two rigs/catalogs with different FOVs and distinct
short/long objectives; identities survive catalog relocation and projections
rebuild without counting mirrored captures twice.

### Phase 2: single-rig autonomous Director

- [ ] Implement versioned allocation, acknowledgements, checkpoints, and limits.
- [ ] Add durable event delivery, local recovery, offline operation, and status UI.
- [ ] Support native sequence safety/hooks and explicit Sync coexistence rules.

Gate: a real N.I.N.A. instance using simulated equipment handles slow autofocus,
failed centering, reprioritization, network loss, restart, and operator stop.
It never starts unauthorized work and reports ambiguous capture outcomes.

### Phase 3: timing-aware shared simulation

- [ ] Instrument operations and learn contextual duration distributions.
- [ ] Add virtual-clock simulation, decision explanations, and replay fixtures.
- [ ] Display uncertainty, constraints, and predicted versus actual progress.

Gate: timing observations affect both hosts consistently; nested durations are
not double-counted and slow operations cause sensible goal reevaluation rather
than timetable catch-up. Live tests from phase 2 become replay regressions.

### Phase 4: quality and calibration feedback

- [ ] Account for pending/accepted/rejected contributions and bounded reacquisition.
- [ ] Request replacement calibration when coverage is invalidated.
- [ ] Connect project readiness and processing provenance to existing workflows.

Gate: delayed uploads and grading do not cause duplicate or unlimited work;
rejected lights and invalid flats reopen only the appropriate deficits.

### Phase 5: coordinated multi-rig acquisition

- [ ] Allocate compatible contributions across rigs and sites.
- [ ] Evaluate horizons, coverage, sampling, priorities, and separate stack groups.
- [ ] Add assignment handoff and project-level allocation/progress views.

Gate: two rigs with different FOVs and horizons advance one project; loss of
contact with one does not authorize duplicate outstanding work on the other.

### Phase 6: remote instances and collaboration

- [ ] Add coordinator/participant APIs, invitations, and scoped permissions.
- [ ] Add provenance-preserving contribution exchange and reconnect reconciliation.
- [ ] Validate expiry, revocation, compatibility, and isolation across instances.

Gate: a remote participant can contribute while retaining local safety/control;
offline recovery preserves history and unauthorized catalogs remain inaccessible.

## Review and validation policy

Each implementation phase requires code review, relevant contract and migration
tests, and UI validation for changed screens. Exercise real N.I.N.A. with
simulated devices, not only mocked plugin classes. Include cancellation,
duplicate/reordered events, stale conditions, expired assignments, clock skew,
disk/SQLite contention, missing images, and incompatible versions.

Maintain shared-engine fixtures across server and native plugin hosts. Protect
ordinary TS execution and existing Sync behavior with regression coverage.
Use an experimental Director release channel until these gates pass; do not
replace the stable Sync registry entry.

Open design choices include objective depth equivalence across instruments,
coordinate/time precision policy, conservative expiry margins, duration-model
retention, and exact TS extension API. Resolve these with documented evidence
in the relevant phase, not implicit defaults in implementation.

## References

- [Data transfer and remote sync](data-transfer.md): existing directional merge
  and grading contracts; Director coordination is additive.
- [Multi-database architecture](multi-database.md): catalog scope and identity.
- [Calibration libraries](../CALIBRATION_LIBRARY.md).
- [Stack previews](../STACKING_PREVIEWS.md).
- [N.I.N.A. downloads](https://nighttime-imaging.eu/download/).
- [TS 3.3 source branch](https://github.com/tcpalmer/nina.plugin.targetscheduler/tree/release/nightly-3.3).
- [Astro-PM N.I.N.A. integration](https://astro-pm.com/nina-sync/) and
  [project lifecycle](https://astro-pm.com/project-management/): product
  references for integrated planning and acquisition, not dependencies or
  specifications for this implementation.
