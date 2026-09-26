# PSF Guard Director: goal-driven acquisition

Status: phase 0 in progress. Shared-core, sidecar, and native simulated
acquisition spikes implemented. An experimental runtime-only plugin preview is
published in the theatr.us registry; no production acquisition plugin yet.
Last updated: 2026-09-26.

This is the tracking document for Director. Update the phase checklist and
record implementation PRs here as work lands. Keep durable architecture here;
move delivered user workflows into focused guides rather than retaining a
completed implementation checklist indefinitely.

## Purpose

PSF Guard Director is a new N.I.N.A. plugin that pursues PSF Guard observing
goals through N.I.N.A.'s supported sequencing and equipment APIs. Target
Scheduler (TS) is a behavioral reference and optional data source, not a base
class, required plugin, or execution dependency. Director is not a downloaded
schedule player.
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
- Own scheduling and feedback policy in the shared Rust core; use a thin
  Director adapter to execute through supported N.I.N.A. APIs.
- Use TS as inspiration and for optional import/coexistence, not as Director's
  planner, execution engine, or required database.
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

A global project can map to multiple rig-local contributions and catalogs,
including optional imported TS projects. Director does not require a TS project
to acquire. Persist mappings by stable identifiers. Do not join by project name,
telescope name, nearby coordinates, or database-local integer IDs. Existing TS GUIDs remain
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

## Rig constraints and local horizons

Meridian restrictions and the effective local horizon are required inputs to
the shared engine, not optional server scheduling hints. Director exports a
versioned constraint snapshot at check-in; server simulation uses that snapshot,
and local execution always checks the current configuration. A remote project
may tighten local limits, but cannot relax a rig's hard restrictions.

### Meridian policy

Use the local TS fork's asymmetric avoidance behavior as a reference when
implementing Director's rig constraints, without depending on the fork. The
inspected branch is
`codex/pier-west-meridian-avoidance` at
`8b549b1123520add0cb65124f469e9cb5723b13d`. Its
`MeridianAvoidanceClipper` resolves independent before/after values from project
overrides and profile defaults, then excludes
`[transit - before, transit + after)`. For example, 60 minutes before and zero
after excludes the preceding hour but permits a new exposure at transit,
subject to all other constraints. This is an acquisition restriction, not a
command to flip or a general model of mount collision geometry.

Director should model three separate concepts:

- Rig/configuration meridian exclusion, with independently specified before
  and after durations. This applies to every assigned project on that rig.
- N.I.N.A. flip/pause safety and execution behavior, including the local fork's
  existing safety margins. Do not let an assignment override them.
- Project imaging preferences, such as TS's existing positive `MeridianWindow`
  that limits imaging to a region near transit. This is not the exclusion zone.

The fork allows each project side to inherit with a negative value, explicitly
disable with zero, or override with a positive value. Import must retain that
provenance and show conflicts when promoting a profile default to a hard rig
limit; do not silently reinterpret legacy overrides. Canonical Director policy
should use explicit inheritance/override states instead of sentinel numbers.
The fork clamps requested avoidance values to 120 minutes because its transit
search extends two hours around the night. Preserve supported behavior and
validate any broader range against the transit calculation rather than copying
that limit as a universal telescope property.

Compose these constraints by intersecting allowed intervals. An exposure and
its blocking overhead must fit a remaining interval; do not test only its start.
Reevaluate after slow autofocus, settling, or a flip. Multiple intervals before
and after exclusions must remain distinct, and resumption must recheck horizon,
maximum altitude, and the remaining night. Unknown required geometry or stale
rig configuration blocks new work rather than silently disabling a restriction.

### N.I.N.A. horizon integration

Discover the active profile's `AstrometrySettings.Horizon` and `HorizonFilePath`
through supported profile interfaces. Recognize N.I.N.A. standard horizon files
and MountWizzard4 `.hpts` files through N.I.N.A.'s supported loading behavior.
Standard records use azimuth/altitude pairs; `.hpts` JSON uses altitude/azimuth
pairs. Do not confuse the two or make the server open the rig's local path.

Export a canonical horizon with azimuth/altitude units and convention, ordered
breakpoints, interpolation/wrap behavior, source format, content fingerprint,
profile identity, and configuration revision. Keep machine-local paths local
unless diagnostic disclosure is explicitly requested. Server simulation and
Director must evaluate the same effective curve. Avoid coarse sampling that
could erase a narrow obstruction; if the loaded public object cannot export
breakpoints, resolve a supported export or a parity-tested conversion before
enabling remote planning with it. Do not reflect private N.I.N.A. arrays.

Compose the curve with rig minimum-altitude restrictions and applicable project
minimum altitude/horizon offset. Retain conservative equality-at-horizon rejection
and verify effective-altitude behavior against N.I.N.A. and TS reference fixtures.
TS is not needed at runtime. Geographic site identity alone is insufficient:
two nearby rigs may have different obstructions, so each rig configuration
binds its own horizon.

Subscribe to profile, location, and `HorizonChanged` events. Also detect a file
edited in place at controlled refresh/check-in points; the path can stay the
same while its contents change. Reconcile disk contents with N.I.N.A.'s loaded
model before publishing a new snapshot, invalidate cached visibility by content
and configuration revision, and replan at a safe boundary. A configured or
previously required horizon that is missing, invalid, or unexpectedly cleared
must produce a visible blocked state, not a flat-horizon fallback. An explicitly
configured no-file/fixed-minimum mode remains valid. Persist the last declared
mode because N.I.N.A. can clear the path when loading a horizon fails.

Acceptance fixtures must cover asymmetric exclusions, independent inheritance,
flip margins, an exposure straddling an exclusion, both sides of transit,
horizon gaps after transit, both file formats, 0/360 wrap, narrow obstructions,
minimum altitude/offset, invalid files, same-path edits, and profile switches.
These are single-rig phase-0/phase-2 requirements, not deferred multi-rig work.
The shared core accepts multiple allocated intervals per goal and now calculates
conservative altitude and meridian windows from the resolved program target and
canonical rig constraints. The N.I.N.A. adapter exports native horizon curves;
the core does not read rig files. Production IPC, preference resolution, and
dispatch binding remain unfinished. Prove the combined path against the pinned
implementations before connecting this geometry-aware evaluator to acquisition.

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

Implementation direction: a standalone Rust planning crate used directly by
PSF Guard and by a bundled Director sidecar. The C# N.I.N.A. plugin communicates
with that sidecar over versioned local IPC. The process boundary is implemented
as a testable spike; plugin packaging and equipment integration remain pending.
Do not maintain parallel Rust and C# versions of the scheduling algorithm.

The core lives in this repository; the Director plugin lives in
[`theatrus/psf-guard-director-nina-plugin`](https://github.com/theatrus/psf-guard-director-nina-plugin),
separate from both PSF Guard and PSF Guard Sync. The
current `tools/director-interop` program is a console proof, not that plugin.
Follow Chatstronomy's core/plugin release separation: the plugin consumes a
pinned, verified Rust artifact and does not compile its own copy of the engine.
Adopt Chatstronomy's bundled backend executable and named-pipe pattern for the
Windows plugin. This isolates planner failures from N.I.N.A. The existing native
DLL experiment remains a test fixture, not the selected production runtime.
The sidecar must consume the same crate and golden decision fixtures, not add a
second scheduling implementation.

The plugin starts a pinned, verified sidecar artifact and negotiates protocol,
engine, and contract versions before accepting decisions. Use bounded messages
with request/session IDs over a current-user-restricted pipe. Credentials must
not appear in command-line arguments, environment variables, or generated
configuration files. Follow the existing Chatstronomy implementation patterns
for artifact identity, checksums, signatures, lifecycle, and transport security.

Sidecar exit, timeout, protocol mismatch, or malformed responses revoke pending
decisions and prevent new dispatch. N.I.N.A. remains responsible for an in-flight
operation and continuous safety handling. On restart, reconcile the durable
execution journal and resubmit current state; never replay a stale decision just
because its IPC request was retried. The phase-0 protocol described below proves
the process boundary, not durable recovery or permission to operate equipment.

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

The IPC boundary needs versioned messages, bounded allocation, error handling,
compatible architecture packaging, and a clear unsupported-version state. No
runtime failure may silently turn into permission to acquire. The retained DLL
test boundary also requires explicit memory ownership and bounded buffers.

## Execution and check-ins

```text
Global objectives and rig capabilities
  -> coordinator allocates bounded goals
  -> Director validates, caches, and acknowledges an assignment
  -> shared engine selects useful local work
  -> Director's adapter executes through supported N.I.N.A. APIs
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
restart, reconcile the Director journal, saved files, and catalog evidence before
retrying uncertain work. TS history is optional evidence for imported workflows,
not Director's required execution journal.
Do not claim exactly-once hardware execution across a crash. Surface ambiguous
outcomes and handle late events from old revisions without losing real captures.

## Director and N.I.N.A. boundary

N.I.N.A. 3.3 nightly is the integration baseline. Recheck and pin exact
source/package versions in phase 0; nightly branch heads and published releases
are not interchangeable.

Director does not need a TS provider extension or fork. The shared Rust core
owns objective selection, scheduling, equipment-operation planning, progress
accounting, retry policy, and timing-aware replanning. PSF Guard simulation and
the local sidecar use that same code; do not put a second scheduler in C# or
delegate selection to TS. N.I.N.A. is the only execution backend in the current
scope. Keep the core independent so another backend can implement the contract
later, but do not build a standalone equipment backend now.

TS is the behavioral reference for both what/when to image and the operations
needed to acquire it. The core must model operation prerequisites, ordering,
completion, interruption, and recovery: startup/shutdown, slew and centering,
rotation, filter changes, autofocus policy, guiding, dithering and settling,
meridian transitions, calibration acquisition, waiting, and check-in boundaries.
This is a requirements inventory, not a claim that these operations already
exist in the core. Actual device control and the mechanics of native actions
remain in N.I.N.A.; immediate safety never waits for a core decision.

The thin C# adapter translates approved work into supported N.I.N.A. sequence
items, mediators, and services. Reuse N.I.N.A.'s hardware, guiding, autofocus,
centering, flip, cancellation, and image-saving machinery rather than copying
TS's execution loop or calling private APIs. Prove each required public API in
phase 0; do not assume the existence of a generic execute-plan endpoint.

Use the same native sequencer-container model as TS: a Director container owns
the session and runs actions through N.I.N.A.'s sequence execution lifecycle.
Preserve the TS-style container options and their semantics, including configured
triggers, conditions, cancellation, and nested action behavior. Calling a
mediator directly is not sufficient if it bypasses those sequence hooks. Director
may extend the container and options, but must not require TS's private container
types or TS installation. This is behavioral compatibility, not reuse of TS's
runtime identity or a promise that saved TS sequences deserialize unchanged.

N.I.N.A. retains continuous local safety and operator control. Director's own
session container coordinates operation boundaries and reports actual results
and durations to the shared core. Native triggers may insert local operations,
but they must not silently authorize another exposure. After slow preparation
such as autofocus or centering, refresh the snapshot and ask the core again
before capture; recheck assignment revision, expiry, safety, and rig constraints
at dispatch. A C# adapter validates and enforces decisions; it does not select a
different goal or invent retry work when the core is unavailable.

Expose a Director session container and focused sequence actions for check-in,
progress reporting, and session completion where useful. Show current goal,
operation, assignment revision, next checkpoint, connectivity, and reasons for
waiting or switching. Avoid an indefinite generic "Working" status.

Director has a distinct plugin identity, configuration, credentials, queues,
and release flow from Sync. Detect competing acquisition controllers. Define
ownership for shared sync/upload duties so coexistence does not create duplicate
uploads or competing planning writes. Reject concurrent controller ownership
rather than letting Director and a TS sequence operate the same equipment.
Optional TS import/catalog compatibility remains a public contract,
including stable GUIDs, schema variation, grade values, and RA unit conversion.

### Chatstronomy interoperability

Retain Chatstronomy's existing TS integration and provide equivalent Director
visibility without making TS a runtime dependency. This is part of the N.I.N.A.
integration, not a later federation feature. Director must also work without
Chatstronomy installed.

The inspected Chatstronomy plugin main at `4add689` consumes N.I.N.A.
`IMessageBroker` topics `TargetScheduler-WaitStart`,
`TargetScheduler-NewTargetStart`, and `TargetScheduler-TargetStart`, projecting
them into `TS-WAITSTART`, `TS-NEWTARGETSTART`, and `TS-TARGETSTART`. It also
projects the native sequence tree and keeps TS-specific command target identity
across per-exposure plan containers. Existing TS behavior must remain intact.

Director needs an explicit, versioned public state/event contract that
Chatstronomy can consume. Prefer N.I.N.A.'s message broker for event delivery
plus an explicit snapshot contract for startup/reconnection. Include provider
identity, rig/profile/session IDs, monotonic event sequence, assignment revision,
stable project/target/objective IDs, operation state, progress, waiting reason,
and next checkpoint. Label forecast times as estimates rather than promised
end times. Publish actual state transitions and operation results, not the
planner's proposals as if execution had already happened.

Do not impersonate TS by emitting `TS-*` events or depending on its private
container types. Add a source-aware Director adapter to Chatstronomy and its
backend presentation/state model where needed. Preserve event deduplication,
bounded replay, profile/session isolation, privacy/access policy, and notification
preferences. A stale or disconnected Director session must not leave chat
reporting an old target as actively acquiring. Native image/equipment events and
Director target events must not produce duplicate notifications for one action.

Chatstronomy commands still enter through N.I.N.A.'s local permissions and safe
sequence hooks. Its TS-specific target resolver is not a Director resolver.
Expose stable Director execution context through a supported contract, invalidate
queued commands when target/assignment/profile ownership changes, and report any
injected operation and its duration to the core before it selects more work.
Neither status consumption nor a chat request bypasses allocation limits or
local safety. Unsupported command paths must reject explicitly, not fall back
to unrestricted equipment control.

Acceptance requires TS-only, Director-only, both installed with one active
controller, and Director-without-Chatstronomy cases. With Chatstronomy present,
verify target changes, waits, current operation, progress, stop/failure, restart,
notification suppression/privacy, and safe-boundary commands against the actual
plugins and backend. Preserve the existing TS fixtures while adding equivalent
Director fixtures; do not weaken their identity and safety checks.

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

Invalid flats can invalidate Director calibration coverage and authorize
replacement flats without a TS installation. Preserve the existing calibration
and optional TS flat-history sync contracts for users who also use Sync.
Distinguish suspect evidence from confirmed invalidation. Keep calibration records separate
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

- [ ] Pin current N.I.N.A. 3.3 nightly and record the supported version matrix.
- [ ] Prove Director-owned execution through supported N.I.N.A. APIs without
  TS installed; preserve ordinary TS and Sync behavior when separately installed.
- [ ] Inventory the pinned TS container options, operation policies, conditions,
  and trigger lifecycle. Map each to shared-core policy or native N.I.N.A.
  execution, with parity tests and explicit reasons for any intended difference.
- [ ] Validate asymmetric meridian constraints against local TS reference cases
  and prove N.I.N.A. horizon export/parity; include multiple safe intervals in
  the engine contract.
- [ ] Prove the shared Rust core loads and returns decisions in PSF Guard and
  a minimal C# plugin through the bundled sidecar, including packaging, version
  negotiation, restart reconciliation, and error handling.
- [x] Implement a deterministic core linked into the PSF Guard Rust library and
  replay shared vectors through a native library from a .NET 10 console host.
- [x] Represent multiple eligibility intervals and subtract asymmetric local
  meridian exclusions without bridging horizon gaps; validate transit coverage.
- [x] Choose a separate thin plugin plus bundled Rust sidecar, following the
  Chatstronomy core/plugin distribution model with versioned local IPC.
- [x] Exercise the same golden decisions through a real Windows sidecar with
  bounded framing, version negotiation, peer checks, and process failure tests.
- [ ] Finalize crate ownership, IPC framing, local state layout, and contracts.
- [ ] Define and test Director's public state/context contract with Chatstronomy,
  preserving its existing TS state and safe-boundary command integrations.

Gate: one simulated target/exposure runs through Director's N.I.N.A. adapter
without TS installed; the same recorded input yields matching server/plugin
decisions. No Sync changes are required to run existing workflows.

#### Phase 0 evidence and remaining work

Implementation review: [shared-core and native interop spike, PR #460](https://github.com/theatrus/psf-guard/pull/460).
Follow-on review: [rig meridian exclusions and multiple safe intervals, PR #461](https://github.com/theatrus/psf-guard/pull/461).
Sidecar review: [bounded IPC and process harness, PR #462](https://github.com/theatrus/psf-guard/pull/462).
Plugin review: [N.I.N.A. 3.3 runtime host and development bundle, Director PR #1](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/1).
Live host validation: [isolated N.I.N.A. nightly smoke tests, Director PR #2](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/2).
Native capture building block: [journaled capture and save lifecycle, Director PR #3](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/3).
Native simulator sequence: [ASCOM capture and FITS readback, Director PR #4](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/4).
Planner bridge: [typed evaluation and Rust-selected native capture, Director PR #5](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/5).
Execution storage: [durable attempt reservations and event outbox, PR #464](https://github.com/theatrus/psf-guard/pull/464).
Storage IPC: [versioned ledger operations and process recovery, PR #465](https://github.com/theatrus/psf-guard/pull/465).

The Director host pins N.I.N.A. `3.3.0.1058-nightly` and the tested sidecar by
commit, CI run/artifact identity, SHA-256, and wire versions. The C# build consumes
the artifact rather than compiling Rust. Its initial settings surface exposes
explicit runtime Start/Stop only; no pairing or equipment dispatch is present.
It has local process/protocol tests and WPF render tests using N.I.N.A.'s button
template. A separately extracted official 3.3 nightly also passed real plugin
discovery, runtime Start/Stop, child-failure recovery, profile-switch cleanup,
normal shutdown, and abrupt parent-exit cleanup on 2026-09-25. Those tests used
fresh redirected profile/plugin directories with no TS and no connected devices;
they did not replace the installed N.I.N.A. 3.2 application. They are not the
full-stack acquisition gate. Signed durable artifacts, native execution, and
Chatstronomy state integration remain open gates; the current CI artifact pin
is developmental.

The internal capture adapter uses N.I.N.A.'s public imaging and save interfaces.
It reserves a capture GUID before dispatch, snapshots the original profile's save
settings, writes `PGCAPID` into FITS/XISF metadata, and waits for a correlated final
save receipt. Queue admission is not save completion. Timeouts and cancellation
after admission retain uncertain evidence; they do not authorize another attempt.
The profile-scoped journal records identity, destination, and monotonic timings,
but it is not yet the sidecar event ledger or recovery engine. Alongside public
mediator and native-header tests, a test-only sequencer probe ran the actual
adapter in official nightly #58 with ASCOM OmniSim camera, mount, and filter wheel
on 2026-09-25. It connected, unparked, slewed, captured three filtered one-second
lights, reloaded their FITS pixels and `PGCAPID`, matched durable journals, parked,
disconnected, and stopped its verified sidecar. This required no TS. The probe
is excluded from the plugin ZIP.

The runtime host now exposes typed, bounded planning evaluation with immutable
snapshots, exact integer identities, strict reply correlation, and cancellation
integrated with its lifecycle. In a follow-on real nightly run, Rust selected
each fixture goal and revalidated it after filter preparation at the adapter's
dispatch callback. Each saved image incremented pending work, not accepted
credit. Seven recorded decisions comprised six acquire results (selection and
revalidation for each filter) followed by `wait: pending_assessment`. The run
passed FITS/journal checks and cleanup; 102 automated tests also passed. C# does
not implement the selection policy. These evaluations are recommendations for
their snapshots, not durable authorization tokens.

The fixture assignment and simulated-safe state are not a server allocation or
recovery ledger. Durable attempt/event accounting, actual hardware-boundary
ownership/safety, recovery, and server feedback remain prerequisites for a
production Director sequencer item and the full-stack gate. The native smoke
test does not claim autofocus, guiding, meridian/horizon enforcement, or
Sync/Chatstronomy coexistence coverage.

Baseline checked on 2026-09-25:

| Component | Inspected baseline |
| --- | --- |
| N.I.N.A. | 3.3 NIGHTLY #58; NuGet `NINA.Plugin` `3.3.0.1058-nightly`. |
| TS reference only | Upstream `release/nightly-3.3`, commit `65478b96c52b47d4860e781c0249799cac2749e1`; source assembly version `5.10.4.0`. Not a Director dependency. |
| Local .NET SDK | `10.0.302`. |

Earlier exploration considered a TS execution-provider extension because its
container constructs its planner and private plan container directly. That
direction is superseded: no TS extension or port is required for Director.
Its meridian behavior remains useful reference evidence. The next execution
slice must instead prove native N.I.N.A. sequencing, event hooks, image-save
observation, and cancellation. No TS source or installed N.I.N.A. plugins are
changed by the shared-core spike.

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

JSON contract 2 / engine 0.2.0 replaces the single goal interval with
`eligible_windows` and requires effective `state.meridian_exclusion` durations.
These local restrictions are intersected with assignment validity, not taken
from project preferences. The snapshot producer must bind these values and the
horizon to `configuration_id`; changing either requires a new configuration
revision. This remains a host-assembled snapshot, not an authenticated wire
assignment. The buffer-based C ABI remains version 1 because its calling
convention has not changed; consumers must check the JSON contract as well.

When either exclusion side is enabled, every goal needs known transit results
with complete search coverage from assignment start minus the after margin to
assignment end plus the before margin. This includes transits outside the
assignment whose exclusions overlap it. Known absence is an empty result with
adequate coverage; missing, duplicate, unsorted, or out-of-coverage transit data
is an error. The core does not claim to validate the astronomy itself. All
non-acquisition responses and errors authorize no new work.

[`windows.rs`](../../crates/director-core/src/windows.rs) normalizes overlapping
or touching eligibility intervals, intersects them with assignment validity,
and subtracts each exclusion without merging across gaps. Exposure plus overhead
must fit one resulting interval. A later fitting interval yields `wait`, while
no authorized fitting interval yields `check_in`. Empty visibility is valid but
does not authorize acquisition. Interval and transit counts are bounded and
time arithmetic is checked. There is no inherited 120-minute clamp: the adapter
must supply transit coverage wide enough for its actual effective limits.

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
dotnet run --project tools/director-interop --configuration Release -- target/director/psf_guard_director_ffi.dll crates/director-core/tests/fixtures/decisions.json crates/director-core/tests/fixtures/rig-windows.json
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

#### Sidecar protocol spike

[`crates/director-runtime`](../../crates/director-runtime/src/lib.rs) links the
same core into a separate executable. The Windows-only
[.NET process harness](../../tools/director-sidecar/Program.cs) launches it over
a random, current-user-only, first-instance named pipe. Both ends verify the
peer process ID. The launcher supplies the pipe name, host PID, and optionally
an absolute state directory. No credentials appear in process arguments. The
protocol has no network or equipment operations; local persistence is opt-in.

IPC version 7 uses a four-byte little-endian length followed by UTF-8 JSON.
Frames are limited to 266,240 bytes before body allocation. The nested planning
request retains its original JSON and the core's 262,144-byte limit, including
duplicate-field validation. Every envelope contains `protocol_version`,
`session_id`, `request_id`, and `payload`; payloads use a `type` discriminator.

- `hello` must be request 0 with a fresh 32-hex-character session ID. It binds
  the rig ID and requires exact runtime 0.6.0, engine 0.2.0, and contract 2.
  `ready` confirms all versions, the rig, and whether storage is enabled.
- `evaluate` wraps one core request and returns a `decision` with the unchanged
  core response. Valid requests for another assignment rig terminate the session.
  Invalid planning inputs return core errors; invalid IPC terminates the session.
  After opening a ledger, stateless `evaluate` terminates the session: it must
  not bypass durable pending work or attempt limits with a caller's old snapshot.
- `ping` returns `pong`; `shutdown` returns `stopped`, then waits at most two
  seconds for the host to close its pipe after reading the reply. The host
  closes before waiting for process exit. This avoids dropping an unread
  Windows pipe reply. No more commands may run after shutdown. Every message
  after hello requires the next consecutive request ID, including heartbeats.
- Pipe connection and hello each have a 15-second deadline. After hello, each
  complete incoming frame has a 30-second deadline; writes have 10 seconds.
  The test host applies a five-second request deadline and kills its owned
  process on an interrupted exchange. A production controller must keep the
  connection alive during long equipment operations without blocking N.I.N.A.
- Clean EOF ends the child normally. Truncation, timeout, incompatible versions,
  and stale/duplicate requests end the session without a replayed response.
  A new child must negotiate a new session and receive a fresh snapshot.

The published Director preview still pins runtime 0.2.1 / IPC 3 with the typed
ledger host and shutdown drain handshake together. Newer IPC 4/5/6/7 development
hosts need their matching adapter and bundle pin. A mismatched version is
refused, never silently downgraded. Publishing this runtime artifact alone does not update
installed plugins or change the existing Sync plugin.

With `--state-directory`, the launcher supplies a private existing directory for
one profile/rig. The runtime verifies the pipe peer before touching storage,
takes an exclusive OS lock on `director-runtime.lock`, and uses the fixed
`execution.sqlite` filename. It never accepts paths in IPC messages or unlinks
the lock file. Another sidecar cannot own the same directory while this owner
or its blocking database operation lives. Process death releases the lock; it
does not clear capture evidence.

Startup waits up to two seconds for lock contention, including Windows sharing
violations during handle release. It never steals a live owner's lock. Invalid
directories and other I/O errors fail immediately; this wait does not retry any
ledger operation or hardware dispatch.

`ledger` wraps a strict `operation` object with an `action` discriminator:

- `open` validates and binds the original core `request`. Its reply contains
  stable ledger, assignment, revision, rig, and configuration identities. This
  is the unbound compatibility path, not observing-program enforcement.
- `open_program` takes the complete version-1 execution `program` and current
  `state`. It returns `program_opened` with the stable ledger `info` and
  `program_version`. Reopening requires the exact original program. It cannot
  adopt an unbound ledger or downgrade a bound ledger to the older API.
- `begin_program_preparation` takes preparation/goal IDs, `local` equipment and
  remembered-pointing observations, estimates, and current state. The ledger
  resolves the target and recipe itself and owns progress projection.
- `advance_program_preparation` and `reserve_program_prepared` take the full
  current `configuration` and state, not merely its ID. They use the existing
  preparation/reservation replies and check fresh conditions at each boundary.
  Bound ledgers refuse the older unbound begin/advance/reserve commands.
- `capture_binding` reads the saved target, recipe, configuration, attempt,
  and ledger identity for a capture. `capture_binding_found.binding` is an
  explicit nullable field. This is recovery evidence, never a dispatch permit.
  Completion, read, close, and event commands work in both modes.
- Ledger `evaluate` takes only current state, projects durable pending progress
  and attempt budgets, and returns a read-only core decision. It never reserves
  an exposure or accepts caller-supplied progress. Active preparation or an
  unresolved capture blocks new selection; safety still wins. This is distinct
  from the forbidden stateless envelope-level `evaluate` after ledger opening.
- `reserve` takes a capture ID and current core state. The shared evaluator and
  ledger return a newly committed reservation, existing evidence, recovery
  required, or a non-acquisition decision. Only the first case is new work;
  none is a hardware permission or a restart dispatch token.
- `record` accepts verified saved/failed/uncertain evidence. `attempt` reads a
  capture's evidence without changing it, including after reconnect.
- `unresolved_attempt` discovers outstanding capture evidence without needing
  the ID from a possibly lost reservation reply.
- `events` pages at most 64 events after a cursor and returns the next cursor.
  There is no acknowledgement, pruning, revision activation, or grading yet.
- `begin_preparation` binds a resolved context, estimates, and current state.
  It returns a created/existing flag and evidence, never a native command.
- `advance_preparation` returns the shared reducer's `run`, `in_flight`,
  `ready_to_reserve`, or `decision` result. Only a newly committed `run` issues
  an operation. Nested results use `status` and `value`; operation ordinals
  and preparation/goal/target/recipe IDs correlate the eventual receipt.
- `complete_preparation` records the correlated outcome and measured duration.
  `preparation` reads by ID; `active_preparation` discovers durable work after
  a lost begin reply. Returned pending commands are evidence, never dispatch
  permissions. These reads do not advance the reducer or its clock.
- `close_preparation` explicitly releases unused work but refuses pending or
  uncertain operations. `reserve_prepared` rechecks the final boundary and
  atomically links a capture reservation; a retry returns existing evidence.
- `preparation_events` pages at most 32 events on its separate cursor. Its
  worst-case escaped payload stays within the unchanged frame limit. Nullable
  context/recovery fields must be present explicitly; omission is not reset.

SQLite work runs on the blocking executor, serialized within the pipe session.
Structured storage failures carry codes, not paths or raw database errors. Wrong
rig identity or malformed commands terminate the session; ordinary busy/invalid
operation errors do not. If a response is lost, the operation may have committed.
Reconnect with the same ledger/allocation and query the active preparation or
unresolved capture (or their known stable IDs); never
interpret a retry returning existing evidence as permission to capture again.
Preparation-not-selected, invalid-completion, clock-regression, and conflicting
evidence failures have distinct bounded codes. Invalid program content returns
`invalid_program`; a changed persisted program or current configuration returns
`assignment_mismatch`. These errors leave the session available
for status/recovery and never authorize a client-side retry of device work.

Portable protocol tests cover loss of the reservation reply, duplicate/unknown
fields, wrong-rig commands, bounded event pages with escaped maximum-length IDs,
contention, exclusive ownership, and refusal to bypass ledger progress. The real
Windows named-pipe harness passed 36 golden decisions and lifecycle/storage checks
across 33 owned sidecars, including forced termination after unbound and bound
preparation operations and their linked capture reservations. It discovers
recovery IDs, preserves exact 64-bit timings, and refuses redispatch. This is
process-level recovery evidence, not a N.I.N.A. or server-loop test.

Run the real Windows process tests:

```powershell
cargo test --locked -p psf-guard-director-runtime
cargo build --locked --release -p psf-guard-director-runtime
dotnet run --project tools/director-sidecar --configuration Release -- target/release/psf-guard-director-runtime.exe crates/director-core/tests/fixtures/decisions.json crates/director-core/tests/fixtures/rig-windows.json
```

CI runs portable protocol tests on all three platforms and Windows process
tests against a release executable. Release panic-abort is intentional here:
the failure stays in the child process, outside N.I.N.A. The sidecar alone is not
a plugin. The separate development host bundles this tested executable and has
passed real N.I.N.A. runtime-lifecycle smoke tests. Signed release artifacts,
integrated durable journals, restart reconciliation, and core-authorized native
N.I.N.A. dispatch remain phase-0 gates.
The existing Sync plugin is unchanged.

#### Execution program bindings

[`director-core::program`](../../crates/director-core/src/program.rs) adds a
version-1 execution-program model above the unchanged planning contract 2.
The program contains one immutable allocation, its rig/configuration snapshot,
targets, exposure recipes, and exactly one target/recipe binding per goal.
The shared Rust API and program-bound IPC 5 commands expose it to local hosts,
not as a server endpoint or acquisition permit. The older unbound preparation
commands cannot bypass a program-bound ledger.

Bindings use stable IDs, never display names or nearby coordinates. Duplicate
IDs, missing/extra goal mappings, and unused target/recipe definitions fail
validation. The recipe duration must equal the goal duration used by the
selector. Short and long exposures can share a filter and target while keeping
separate recipes and goal accounting. A validated program owns its snapshot;
mutable caller data cannot change a resolved binding afterward.

Configuration includes camera identity, fixed-filter or wheel-slot mappings,
allowed binning pairs and readout modes, exposure limits, gain/offset support,
and local slew/dither preferences. Range and discrete gain/offset capabilities
are distinct. Null gain/offset means unsupported, not "use the current value";
supported controls require an explicit allowed value. The initial model uses
nonnegative control values and explicit readout modes compatible with the current
N.I.N.A. execution baseline. Unrepresentable capabilities must block adoption,
not silently become a guessed mode or setting. Hosts must export and refresh
actual capabilities before this model is used for equipment dispatch.

Coordinates are ICRS integer milliarcseconds: RA and position angle lie in
`[0, 360 * 3_600_000)`, declination in `[-90 * 3_600_000, 90 * 3_600_000]`.
Round to the nearest milliarcsecond at the input boundary, at most 0.5 mas per
coordinate, and convert to degrees only at an astronomy/device boundary. TS RA
in hours must first be converted to degrees. These values are exact in both
Rust and C# JSON integer representations. A null position angle means no
requested rotation, not zero. The initial preparation path requires a connected
rotator and enabled centering when an angle is requested; manual rotation
attestation is not implemented.

`BoundProgram::preparation` resolves IDs, filter, readout, and dither settings
from the program and then uses the existing shared reducer. The current local
equipment snapshot must equal the bound snapshot, not merely reuse its ID.
It does not select
work itself. The owning ledger may project accepted/pending/remaining-attempt
counters, but cannot alter recipes, priorities, durations, windows, allocation
identity, or expand the original remaining attempt budget. This API does not
authenticate caller-supplied progress; the ledger remains its required owner.

Remembered framing includes the complete target and configuration identity.
Changed coordinates, rotation, or configuration require new-target preparation
even when the stable target ID is unchanged. Hosts must invalidate remembered
pointing after manual movement, failures, profile changes, or other lost
evidence; merely remembering the same name/ID is not enough.

Program payloads retain the 256 KiB bound and strict fields, including explicitly
present nullable settings. Tests cover typed/JSON roundtrips, integer fidelity,
capability rejection, complete mappings, distinct short/long recipes, immutable
snapshots, stale framing, projection restrictions, and fresh safety decisions.
The ledger persists this exact program and resolves saved capture bindings.
IPC 5 exposes the bound preparation path; native capture settings must next use
the same resolved recipe. Do not claim recipe enforcement from unbound APIs.
Pairing, meta-database authority, native container execution, effective horizon refresh,
and the full server/N.I.N.A. gate remain open.

#### Shared exposure preparation

[`director-core::preparation`](../../crates/director-core/src/preparation.rs)
models the first native-operation boundaries without device APIs or I/O. This
is an internal Rust API exposed since IPC 4 through the durable ledger. Planning
JSON contract 2 and the published plugin's behavior are unchanged.

The reducer follows the pinned TS reference's preparation order: unpark when
needed; center/rotate and the Before New Target hook on a target transition;
dither when the effective per-filter cadence requires it; switch filter; set
readout mode. Disabling automatic slew/center leaves the target hook enabled.
A recipe may inherit the target's dither cadence or explicitly disable it.
Recipes sharing a filter share its confirmed exposure counter; a target
transition starts with fresh dither history. The caller supplies resolved
recipe identifiers, equipment context, and confirmed history. This reducer
does not yet own that history or resolve device settings.

Each operation has a preparation ID and ordinal and is issued once. A matching
completion records the observed monotonic duration separately from wall time.
Identical receipts are idempotent; conflicting or unrelated receipts cannot
advance the sequence. A delayed receipt remains valid after a newer status
poll without moving the snapshot clock backwards. Hook durations include
nested native work; callers must not add child durations a second time.

Every boundary runs the shared selector with the remaining preparation and
capture overhead estimates. Actual elapsed time, rather than the original
estimate, determines whether the next operation/exposure still fits. Safety
and operator stop win immediately. Changed assignments/configurations latch a
check-in and wait for the current indivisible action's receipt; failures or
uncertain outcomes cannot silently retry. A final ReadyToReserve result is
only a fresh recommendation, never a stored dispatch permit.

The focused regression suite covers ordering/options, per-filter cadence,
remaining estimates, slow-operation reselection, delayed/conflicting receipts,
in-flight assignment changes, configuration changes, safety, expiry, stale
conditions, and final-boundary revalidation. Run it with:

```powershell
cargo test --locked -p psf-guard-director-core --test preparation
```

Program-bound plugin integration and native container execution remain required
before hardware use. A host must
not reconstruct lost reducer state and replay an operation whose outcome is
unknown. Preparation does not consume capture attempts or credit images; the
ledger and a fresh native dispatch check remain separate requirements. Session
startup/shutdown, autofocus policy, guiding, flips, and calibration are still
open parts of the operation inventory, not implied by this initial reducer.

#### Durable execution ledger

[`crates/director-ledger`](../../crates/director-ledger/src/lib.rs) owns the
first local attempt/event storage contract. It depends on the shared core and
SQLite, leaving the planner itself free of I/O. IPC exposes it through explicit
storage operations, but the native capture adapter does not use it yet. It
changes no existing catalog, Sync endpoint, or installed plugin package.

The initial ledger binds one immutable allocation, its original accepted/pending
baseline, the rig/configuration, and the exact engine/contract versions. Each
ledger has a stable UUID and a monotonically increasing event cursor. Unknown
schema/engine versions, foreign databases, and changed allocations are refused.
This deliberately does not activate assignment revisions yet: server baselines
must first identify acknowledged event cursors so local saved work is not added
twice. Never rotate the ledger file to bypass this guard or reset attempt limits.
The eventual host must keep one fixed, profile/rig-scoped ledger location.

Reservation runs the shared evaluator against current conditions and projected
progress under a short SQLite `BEGIN IMMEDIATE` transaction. It then commits the
attempt and outbox event together before returning a newly created reservation.
Competing connections cannot both reserve work. Saved images add pending credit,
not accepted credit; failed attempts consume budget without refund. A stable
image identity cannot credit two captures. Repeated identical result delivery
does not append another event. The outbox supports bounded cursor pages but no
acknowledgement or pruning until a server inbox contract exists.

Any reserved or uncertain attempt blocks new rig work after a restart. Retrying
the same capture ID returns existing evidence, never a new dispatch candidate.
Only a verified final save receipt can mark an image saved. A queue admission,
cancellation, timeout, or missing file is not proof of failure. Uncertain work
can resolve to a verified saved image; this first contract has no operator
resolution for uncertain no-image outcomes. Terminal evidence is immutable.
The native adapter remains responsible for matching the image identity to its
capture and recording measured elapsed time, not an estimate.

SQLite WAL with full synchronization retains committed evidence across process
termination; attempt/result and event writes roll back together on failure.
Only absolute filesystem paths are accepted, URI open modes are disabled, and
WAL mode must be confirmed. Temporary/in-memory databases are not durable ledgers.
Tests cover abrupt exits with committed reservations, committed saved receipts,
and unfinished transactions, competing writers, bounded lock contention,
duplicate results, baseline preservation, exhausted budgets, and late saves.
Copying a live SQLite main file without its WAL is not a supported backup. Backup,
restore/fork detection, acknowledged revisions, grading events, and recovery
resolution must be specified before production use.

A reservation is not a hardware permit. Sidecar session fencing, actual dispatch
boundary safety/ownership checks, and native-journal reconciliation are still
required. The current adapter and ASCOM probe have not exercised this ledger.
The next integration must preserve these distinctions rather than treating a
replayed reservation or an old `acquire` decision as permission to capture.

Ledger schema 2 adds a preparation journal using the same writer transactions.
It migrates an owned schema-1 ledger without changing its UUID, allocation,
attempts, or capture events. Wrong allocations/engines roll back the migration;
older binaries refuse schema 2. Capture event schema 1 remains unchanged.
IPC 4 introduced these local APIs; this is not a plugin release or server endpoint.

`begin_preparation` binds the resolved context to ledger-derived progress.
`advance_preparation` checkpoints the pure reducer and appends an issued event
before returning `Run`. Lost replies, restart, and competing callers return
in-flight evidence rather than reissuing the operation. Correlated completion
and its measured duration commit with the outbox record; duplicate delivery
does not add an event. Core checkpoints are bounded/versioned and validate
operation order, receipt identity, terminal state, and time ordering. A stored
SHA-256 digest detects damaged checkpoint bytes, including valid JSON that
would otherwise erase an in-flight operation. This is local integrity checking,
not authentication against someone who can rewrite the database.

Ordinary capture reservation is blocked while a preparation is active.
`reserve_prepared` refreshes conditions and rig/configuration at the final
boundary and atomically creates the capture reservation and preparation link.
It does not charge completed preparation estimates a second time. Repeating
the same reservation returns existing evidence; it never permits redispatch.
Explicit closure can release unused preparation, but cannot discard pending
or uncertain operations. A correlated late receipt can finish a pending action.
An explicit uncertain completion remains blocked: an operator-attested recovery
contract is still required rather than silently clearing it.

Preparation has a separate bounded, replayable outbox with its own cursor.
Events carry ledger/allocation/rig/configuration/engine identity and a capture
link when reserved. Consumers must keep preparation and capture cursors distinct;
there is no implied global order between the two feeds. No acknowledgement or
pruning is implemented. Status reads use `preparation`; `advance_preparation`
is a durable boundary mutation, not a high-frequency UI poll. The new storage
tests cover real child-process exit after issue/completion, partial transaction
rollback, concurrent issue, delayed/conflicting receipts, final readiness,
schema migration, and checkpoint corruption. They do not replace the required
native-container/server end-to-end test.

Ledger schema 3 adds immutable execution programs. `open_program` validates and
stores the allocation and complete program in one transaction. Reopening
requires the exact original program, including target coordinates, recipes,
equipment capabilities, and wheel slots. A separate required-program marker
and payload digest detect missing or damaged program data; they are integrity
checks, not authentication against database writers. Existing schema-1/2
ledgers migrate as unbound without changing their identity or evidence. Bound
and unbound ledgers cannot switch modes, even when empty; failed adoption rolls
back migration. Older binaries refuse schema 3.

`begin_program_preparation` resolves context from the persisted program and
uses only ledger-derived progress. Bound advance and reservation APIs require
the full current configuration snapshot to match, while the shared reducer
still checks fresh conditions at each boundary. Unbound begin, advance, and
reservation calls refuse program-bound ledgers. Completion, recovery reads,
and explicit closure remain available because they cannot issue new work.
An identical begin retry returns existing evidence without reselecting a goal
after progress has changed. `capture_binding` returns the saved target, recipe,
configuration, and attempt identity; it is evidence, never a dispatch permit.

Program-ledger tests cover exact reopen, mode separation, full configuration
checks, migration rollback, damaged metadata, concurrent issue/reservation,
transaction rollback, and real process exits with issued operations or reserved
captures. IPC 5 exposes these storage APIs through explicit bound commands;
older protocol versions fail negotiation before opening storage. This does not
change the released plugin or prove native equipment integration.

Run the isolated storage regressions with:

```powershell
cargo test --locked -p psf-guard-director-ledger
cargo clippy --locked -p psf-guard-director-ledger --all-targets -- -D warnings
```

#### Shared visibility geometry

`director-core::visibility` accepts the canonical full-breakpoint horizon
export developed in plugin PR #13. It preserves NINA's linear segments,
modulo-360 queries, explicit 0/360 discontinuities, and sub-sample obstructions.
Fixed-minimum mode is explicit, not a substituted zero-degree curve. Local and
project altitude minima/maxima are intersected, equality is excluded, and
project horizon offsets cannot lower the local obstruction curve. Conflicting
but individually valid limits produce no visibility rather than malformed data.

The same module transforms ICRS J2000 positions to topocentric azimuth, altitude,
and hour angle with the published, pinned `sofars` 0.6.1 crate. It uses zero
proper motion/parallax and no atmospheric refraction, matching NINA's
zero-pressure transform mode. Earth-orientation corrections and their validity
interval are explicit inputs; missing or stale evidence is not zero correction.
UTC is converted through a two-part SOFA quasi-Julian date, including leap-day
length. The wrapper explicitly restricts years to 1970-2028 because the pinned
translation drops SOFA's dubious-year warning; future model updates require
review. The bound also prevents the dependency's ephemeris unwrap from seeing
out-of-range dates. No live time, network fetch or host-local state enters the
calculation.

The independent reference fixture was generated from nightly 3.3.0.1058's
`SOFA_2023_10_11.dll`, with its hash recorded in the JSON. Forty-eight cases cover
northern/southern and near-polar sites, RA wrap, polar targets, and the 2016/2017
leap-second boundary. Rust agrees within 1e-9 degrees. Regenerate explicitly with:

```powershell
dotnet run --project tools/director-astronomy-reference --configuration Release -- <NINA-SOFA-DLL> crates/director-core/tests/fixtures/nina-sofa-positions.json
```

This is numerical parity against the native library shipped by NINA, not a
complete native sequence or server test. The existing selector/IPC are unchanged
and do not yet consume this geometry. Complete transit search, darkness,
constraint IPC and cache identity,
Earth-orientation acquisition, and production dispatch enforcement remain gates.
Point visibility alone must not authorize a shutter operation. In particular,
do not generate safe intervals with a coarse time grid that misses narrow
obstructions between samples.

`check_altitude_span` now screens an entire closed interval, including its finish
instant, against altitude and the full horizon curve. Callers must include
exposure and blocking overhead and rerun after slow preparation. `Clear` is
altitude evidence only, not assignment, meridian, darkness, safety, or hardware
authorization. A sampled violation returns `Blocked` with its time; malformed,
stale, unsupported, unresolved, or over-budget geometry returns a typed error.
None of those errors may be treated as clear or replaced by endpoint checks.

The fixed-star, zero-pressure model uses a conservative angular motion envelope
of 0.01 degrees/second. This is over twice terrestrial sidereal rotation
(less than 0.0042 degrees/second), with room for the much slower apparent-star
terms in the pinned SOFA model. Proper motion, parallax, moving objects, and
refraction are excluded by `observe`; this bound must be reviewed before adding
any of them or changing the astronomy dependency. An additional 0.001-degree
guard covers floating-point error. It is not a model of mount pointing error
or a substitute for measured local clearance margins. A span crossing a SOFA
UTC offset adjustment is refused: a single constant DUT1 value cannot model
that transition, even when the caller declares a longer validity period.
Supporting such spans requires time-varying orientation evidence, not a larger
numeric guard. Ordinary midnight crossings remain supported.

At each interval midpoint, a spherical cap bounds every direction in that
interval. Altitude extrema follow directly from the cap radius. The spherical
metric bounds the azimuth sweep using the largest absolute altitude in the cap;
a pole-touching cap cannot assume a narrow azimuth range. The maximum horizon
over that sweep includes every piecewise-linear breakpoint and both sides of
the 0/360 discontinuity. Narrow obstructions cannot disappear between time
samples. Intervals that cannot be certified split depth-first. At one millisecond
resolution they remain unresolved, never clear. Requests are bounded to 24 hours
and 8192 sky observations; memory grows with subdivision depth, not duration.
This conservative test can refuse otherwise usable time near a boundary.

Tests cover midpoint obstructions with clear endpoints, unsampled adjacent-float
horizon spikes, north discontinuities, extra overhead crossing altitude limits,
strict finish-time Earth-orientation validity, leap seconds, and dense SOFA
cross-checks across sites and polar targets. These tests exercise the shared
crate, not a native acquisition or server loop. Complete observing-window construction
and integration of this screening into the selector and dispatch contract remain
required; no existing planner result gains new hardware authority here.

`altitude_windows` builds conservative altitude-only windows over the same
bounded search span. It reuses the span check's spherical envelope and horizon
extrema. Entirely clear or blocked caps prune the search; uncertain caps split
until one-second resolution. The result separates certified `windows` from
`unresolved` intervals, so an empty list of usable windows is not necessarily
proof of total obstruction. Unknown regions never become eligible windows.
Adjacent certified regions merge, but neither an obstruction nor an unresolved
gap can be bridged. Both output lists are capped at the selector's 128-window
limit; invalid inputs or exhausted count/observation budgets return no partial
coverage. Fully obstructed regions are skipped without testing every second.

Tests feed the resulting windows into the existing selector and meridian
interval composition. Exposure plus overhead must fit a single surviving
window; a narrow horizon obstruction can force waiting for the next one.
Day-long dense SOFA checks verify the generated clear regions across sites.
This is not yet the complete availability compiler: darkness,
immutable constraint identity, IPC, and production dispatch binding
remain separate requirements. The native integration must not treat these
altitude windows alone as an observing assignment or hardware permit.

`meridian_windows` now constructs conservative upper-meridian exclusion windows
using the same SOFA positions and bounded spherical motion envelope. The search
extends before the assignment by the after-transit margin and after it by the
before-transit margin. Crossings outside the assignment can therefore still
remove overlapping time. Observed hour angle and declination are a rotation of
the same horizontal direction, so the spherical longitude bound applies there
too. The search does not assume monotonic hour angle near a celestial pole or
mistake the +/-180-degree lower culmination wrap for an upper transit.

Caps that cannot reach hour angle zero are skipped. Other intervals subdivide
to one-second resolution and become `possible_transits` bands; these may include
near misses, not just confirmed crossings. The whole band plus the independent
before/after margins is excluded. A band midpoint is **not** an exact transit
and must not be inserted into the legacy `TransitCoverage.transits_ms` contract.
Adjacent bands merge, and overlapping exclusions never create a usable gap.
The expanded search must satisfy the same 24-hour, 8192-observation, supported
time, Earth-orientation freshness, and UTC-offset-continuity requirements. Any
error returns no partial result. Each output list is limited to 128 intervals.
Explicit zero/zero exclusion skips geometry and returns the valid assignment;
it does not disable any other constraint or imply geometry was validated.

Regression tests check independent margins, outside-assignment crossings,
lower culmination, stale expanded coverage, leap transitions, overflow, bounded
work, and dense crossing searches across northern/southern sites and near-polar
declinations. These windows enforce the rig exclusion only. They are neither
flip commands nor proof that a mount can safely track or flip, and production
integration with the other constraints and fresh dispatch checks remains open.

`geometry::BoundGeometry` now connects those altitude and meridian calculations
to a validated observing program and the existing goal selector. Its versioned
`Constraints` input contains a rig/configuration identity and revision, complete
site, Earth-orientation evidence, full horizon, hard altitude limits and meridian
policy, plus explicit altitude preferences for every allocated goal. Missing,
duplicate, unknown, invalid, or wrong-scope entries fail before a usable result.
The target comes from the program's immutable binding, converting its integer
ICRS milliarcseconds to degrees; callers cannot supply an unrelated coordinate.

Compilation intersects the original allocation, shared altitude clearance and
shared meridian exclusions. Project preferences cannot lower rig restrictions;
uncertain horizon regions remain unusable. No geometry result can expand the
coordinator's original eligibility. The legacy exact-transit contract remains
required when its policy is active and can only further restrict the computed
windows. A claimed empty transit list cannot suppress the independent shared
meridian calculation. This does not turn approximate bands into exact transits.
Fragmentation beyond 128 intervals returns an error, never a bridged gap.
Goals sharing the same bound target and altitude limits reuse a calculation;
their individual allocated windows and recipe identities remain separate.

The compiled object owns its inputs and is not deserializable. Its evaluation
accepts only progress-counter changes to the original allocation and compares
the complete freshly supplied constraint snapshot before considering new work.
Unchanged revision labels cannot hide changed content. Goal preference order
does not affect identity. A mismatch refuses reuse and requires recompilation;
the caller is responsible for refreshing native profile/horizon state rather
than presenting a stale cached snapshot. Safety stops and completion of an
already in-flight indivisible native operation retain the existing selector's
precedence. Exposure plus blocking overhead must still fit one computed window.

`BoundGeometry::preparation` binds the shared native-operation reducer to these
computed windows. It resolves target, recipe and local configuration through the
original program, keeps the inner reducer private, and checks original intent
and fresh constraints at every operation boundary. Remaining setup estimates,
not the original aggregate overhead, must fit together with the exposure. Slow
centering or hooks can therefore end a preparation before another operation or
capture reservation. Readiness is checked again even after all steps finish.
A changed constraint latches a check-in; changing it back does not revive the old
preparation. An in-flight action still needs its correlated receipt, and safety
stops retain precedence. No failure, uncertainty or lost receipt authorizes replay.

An issued preparation command still needs a fresh feasibility check after native
before-hooks, since those hooks can consume its window before dispatch.
`GeometryPreparation::check_pending_dispatch` checks the exact pending command
against fresh state and constraints, retaining the pending step's full estimate
plus all later preparation and capture overhead. Completed steps are not charged
again. A refusal is sticky and survives the existing checkpoint format; safety
stops override other refusals and correlated receipts remain admissible.
Normal `next` polls retain their in-flight behavior and never reinterpret an
already running operation as new work.

This core check returns a decision, never `Run`, a new command, or a replay permit.
`Continue` is not permission to start another operation. Even an `Acquire`
feasibility result requires the original one-shot command from the current live
session and local native checks. A recovered pending command remains uncertain
evidence. The durable journal commits these checks through the entry points
below, exposed in IPC 7. The native before-hook boundary still needs to adopt
them before production dispatch; the existing preview gains no authority.

Geometry preparation has an opaque, versioned checkpoint for local persistence.
Restore requires a separately compiled binding from the original trusted program
and constraints. It compares the complete inputs, reconstructs the initial
narrowed request, and rejects changed saved windows or preparation context.
Pending commands remain pending, late receipts remain admissible, and stored
constraint stops, safety stops, failures and uncertainty survive recovery.
Readiness is never restored as a dispatch permit: the next call still requires
fresh state and constraints. Exact JSON float round-tripping preserves adjacent
horizon vertices. The complete checkpoint has a 2 MiB decode/encode cap, and the
inner operation journal keeps its existing size cap; a checkpoint error must
prevent dispatch, not fall back to unjournaled execution. The legacy preparation
loader rejects the geometry envelope. These bytes are neither authentication
nor tamper protection; the owning journal must atomically commit and verify an
integrity digest along with operation events before returning any Run command.

Ledger schema 4 adds an immutable geometry-bound mode. `open_geometry` recompiles
the supplied original program and constraints before opening a writer transaction,
then compares them with stored program and geometry metadata and their integrity
digests. Legacy/program-only ledgers cannot adopt this mode, and geometry-bound
ledgers cannot downgrade. Schema 1-3 migrations retain the original ledger ID,
pending operations, captures and outboxes without reinterpreting their evidence.

Geometry-aware selection, begin, advance and final reservation use durable
progress. Advance and reservation require fresh constraints and an exact current
equipment configuration. The journal shares the existing one-active-preparation
and one-unresolved-capture invariants; it writes checkpoints, events and capture
links atomically. Reopen restores pending commands as pending, and a failed event
write rolls back the accompanying checkpoint or reservation. Read-only recovery
and completion receipts remain available after constraints change. Tests cover
competing handles and abrupt process exit, not just orderly close/reopen.
The legacy mutation and selection entry points reject geometry-bound ledgers.

IPC 6 / runtime 0.5.0 exposes `open_geometry`, `evaluate_geometry`,
`begin_geometry_preparation`, `advance_geometry_preparation`, and
`reserve_geometry_prepared`. Open returns the program and constraint schema
versions; every selection or dispatch recommendation requires the complete fresh
constraint snapshot, not just its revision. All nested rig IDs must match the
handshake before storage is touched. Existing receipt, lookup and outbox commands
remain usable. The runtime delegates to the geometry ledger; it does not compute
a second set of windows or accept caller-provided geometry certificates.

Wire limits remain 262,144 bytes per operation and 266,240 bytes per frame.
A program plus a large horizon may exceed this transport limit even when each
is valid separately; refuse it rather than thinning the horizon or dropping
constraints. Compilation runs on the blocking storage worker. Process tests kill
and restart the Windows sidecar after issued work and capture reservation,
checking that neither can be dispatched twice. The plugin adapter and artifact
pin must be updated together before this mode can be used from N.I.N.A.

After a prepared capture is reserved, native before-exposure hooks can still
consume its observing window. `check_geometry_capture_dispatch` rechecks the
exact preparation/capture link with fresh state, full constraints and equipment
configuration. It accepts only a still-reserved attempt, never saved, failed or
uncertain evidence. The temporary progress projection adds back only that
reservation's attempt for the core's feasibility calculation; the stored attempt
budget is not refunded, and no new capture is reserved. Completed preparation
is not charged twice; exposure and remaining capture overhead must still fit.

The check atomically commits the preparation clock and any new sticky refusal.
An event-write failure rolls both back. A changed constraint or closed window
cannot be undone by reopening the ledger or restoring the old conditions; safety
stops retain precedence and verified save receipts remain admissible. Matching
program-only and legacy entry points preserve mode separation. `Continue` is not
readiness; even `Acquire` is only fresh feasibility for the original live-session
reservation, not a replay grant. The host must retain its one-shot dispatch
authority and revalidate native ownership and safety at the actual boundary.
Native adoption remains required, and no published plugin gains authority.

`check_geometry_pending_dispatch` requires the exact pending command, fresh
constraints, current equipment configuration and boundary state. In one writer
transaction it checks durable progress, runs the shared-core feasibility check,
persists its checkpoint and appends a changed halt once. A failed event write
rolls back both the halt and snapshot clock. Repeated checks issue no command,
add no capture credit and cannot bypass mode or lifecycle checks. Program-only
and legacy ledgers have corresponding mode-specific entry points. A second
handle observes the same sticky refusal, including a later safety escalation;
correlated completion still resolves the pending evidence. No result authorizes
replay of a recovered command.

IPC 7 / runtime 0.6.0 adds `check_geometry_pending_dispatch` with the exact
issued command and `check_geometry_capture_dispatch` with the linked preparation
and capture IDs. Both require full current configuration, constraints and state,
and return `dispatch_checked` with the shared-core decision. Nested rig IDs are
checked before storage access. Malformed or cross-rig messages terminate the
session; valid but stale commands, links or evidence return scoped storage
errors. A returned `Acquire` is feasibility only, never new dispatch authority.
Neither operation issues a native command, reserves a capture or refunds credit.
Plugin PRs #23 and #24 adopted the matching version, typed replies and
session-bound post-hook checks for the isolated native simulator sequence.
Captured preparation records may now contain a halt from a refused post-reserve
boundary while retaining successful preparation observations and their capture
link. Host decoders must accept that specific state without allowing pending or
unsuccessful preparation operations in a captured record. The IPC-7 plugin
decoder has regression coverage for that captured-plus-halted state.

This is a Rust ledger path exposed through IPC 7 and adopted by the native test
adapter, not a production acquisition container or hardware permit. The adapter
checks current native state around IPC and retains one-use dispatch authority.
This is sampled boundary validation, not a hard real-time lease or continuous
safety interlock; production dispatch must account for elapsed check/IPC time
and retain N.I.N.A.'s native safety handling.
Compilation is bounded by the program/geometry limits but can be expensive for
many distinct targets; schedule it off the interactive/dispatch path. Shared
darkness calculation, other observing criteria, and the full server/N.I.N.A.
acceptance gate remain required. No production or preview plugin gains new
acquisition authority from this API.

SOFA attribution and the full upstream terms live in
`crates/director-core/THIRD_PARTY_NOTICES.md` and `SOFARS-LICENSE.txt`.
The runtime CI artifact now includes these files beside the executable.
Plugin [PR #20](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/20)
adopted the merged geometry runtime and verifies and packages both notices.
Its fetch tests reject missing, corrupted, or extra artifact files and repair
missing or changed cached notices. The published `0.1.0.1-preview.1` plugin
bundles runtime 0.6.0 / IPC 7 with those notices. Its public controls still only
start/stop the runtime and report status; it cannot pair, receive server
assignments, or acquire. Its isolated N.I.N.A. nightly/ASCOM test saved three RGB
frames and exercised native hooks, restart and horizon changes using a fixture
assignment, not PSF Guard authorization. The full-stack gate remains open.

### Phase 1: meta database and global project model

The `psf-guard-director-meta` crate starts the coordination store, separately
from catalogs and the local execution ledger. It is linked into PSF Guard but
has no implicit registry migration or catalog adoption. A CLI server can opt in
with `--director-meta` and database management; omission opens no store. The
[metadata API](../DIRECTOR.md) exposes authenticated, bounded project/site/rig
listing, creation and revision-checked renames, plus immutable site snapshots
and rig setups under their explicit owners. Snapshot bodies retain the full
shared-core size bound instead of the smaller identity-form limit. It grants no rig or acquisition
authority. Other hosts must explicitly create a new file or open a recognized
existing file. Schema 1 stores the
coordinator instance UUID, global project and rig identities, originating catalog
identities, and explicit catalog/source-project-GUID links. Names and URL slugs
never establish identity; rigs outlive catalogs, and one global project may link
to several catalogs. Project and rig listings use bounded, stable-ID cursor pages;
renaming an entity does not move it across a page boundary. Catalog IDs must be
explicitly registered and retained by the adoption workflow, not
regenerated on every import or inferred from
paths. The operator preview/apply API establishes this identity explicitly;
the Catalogs view reviews and applies mappings. Native-catalog migration remains
unimplemented.

The `src/catalog_identity.rs` storage primitive supplies an opt-in identity for
a PSF Guard-managed catalog destination. A versioned, PSF Guard-owned singleton
table retains the catalog UUID and its originating coordinator UUID. It does
not change TS tables, GUIDs, grades, `application_id` or `user_version`. Ordinary
reads return an unadopted result without creating anything. Only the explicit
operator adoption API invokes it; startup, sync and discovery never adopt.

The preview/apply workflow confirms the destination, retains the
proposed IDs across retries, and revalidates preview evidence in the same SQLite
transaction as adoption. Adoption uses a savepoint inside that transaction;
only the host's outer commit makes it durable. A conflicting identity or damaged
record is refused, not repaired or reassigned. Never adopt a read-only TS sync
source. Catalog and coordinator commits are separate: retain the catalog's
committed identity and retry registration after an interrupted coordinator write.

SQLite-aware backup and file moves preserve this identity. A copied catalog is
the same lineage, not a new rig or independent catalog. Registering multiple
paths to that lineage must not duplicate contributions; an independent fork
requires an explicit future workflow. Identity alone grants no execution rights
and does not prove that every historical frame belongs to a given rig. The
historical attribution and duplicate-mount contribution accounting
remain unimplemented. The API only accepts registered catalog slugs. It holds
the coordinator writer while committing catalog identity, then commits catalog
registration and every confirmed mapping together. This prevents another new
catalog from claiming that UUID during finalization. If the coordinator commit
fails afterward, the durable catalog ID remains available for a retry.

Writes use short SQLite transactions with foreign keys, WAL and full synchronous
commits. Renames require an expected revision; retries cannot overwrite another
editor's work or silently move a source project to a different global project.
The store refuses foreign databases and unsupported schema versions. Opening a
schema-1 or schema-2 store upgrades it to schema 3 in one writer transaction, preserving its
instance and entity IDs; failed or competing migrations cannot partially commit.
Future migrations must retain those properties. Back up before upgrading and
stop older coordinator processes first; mixed-version online operation is not a
supported migration workflow.

Schema 2 adds named sites, immutable site snapshots and immutable rig setups.
Each site snapshot retains the complete location and native horizon, including
unequal 0/360-degree endpoints. Each rig setup binds one exact equipment
configuration ID to a site snapshot, rig altitude bounds and asymmetric meridian
exclusion. Its coordinator setup UUID is distinct from the native equipment
fingerprint (N.I.N.A. uses `nina-...`); retain that native ID unchanged in planning
programs. Geometry can change under a new setup UUID without inventing a new
equipment fingerprint. Changed content requires a new snapshot/setup ID;
registering the same ID again is idempotent only for unchanged content. Bounded ID
inventories support discovery, but neither their ordering nor a display name selects a
"latest" configuration. Planning must name an explicit revision.

Equipment, site, horizon and altitude validation use the shared core. Snapshot
payloads must fit its request-size bound; oversize horizons are refused, never
thinned. A configuration snapshot is not a complete planning request and does not
guarantee the combined request fits the IPC frame. Dynamic Earth orientation,
current conditions, observing windows and authorization remain separate inputs.

Backup uses SQLite's snapshot API, including committed WAL content, then publishes
a checked standalone file without overwriting a destination. Restore likewise
requires a new destination and preserves coordinator identity. Never run a
restored copy alongside the original as another coordinator. Backups do not
include catalogs, images, or Director's local execution journal. Stop the old
coordinator before switching to a restored path. Online replacement and automated
restore/configuration switching are not supported.

The opt-in Director management page lists, creates and renames global projects,
sites and rigs, independently of catalog selection. It honors read-only accounts,
keeps the selected collection in the URL, and supports revision-checked renames.
The Catalogs view discovers source projects/profiles, selects or creates global
projects and rigs, and requires preview before applying mappings. It preserves
choices across retry, discards stale reviews, and flags source-profile drift
without reassignment. A profile shares one rig choice across its projects.
Read-only users can inspect saved links. Desktop and narrow layouts expose the
source, destination project and rig together. Configuration and objectives are
not yet editable in this screen.

Identity, configuration and project-intent storage do not complete coordination.
Allocation authority, rig enrollment/permissions and configuration/objective UI remain
required. No assignment or hardware authority is created by registering a rig,
catalog or project-intent snapshot.

The shared core's `project` module defines versioned project intent separately
from an execution assignment. Each immutable snapshot identifies the global
project, objectives, and rig-specific contributions. Objectives retain explicit
bandpass and purpose IDs, priority, and ICRS target intent. Contributions retain
an exact setup revision and native configuration ID, panel framing, concrete
recipe/filter mapping, and their own required accepted-frame count. A narrow
field panel and a wide-field image, or short and long exposures in one band,
remain separate contributions. Their counts are not interchangeable and project
membership does not authorize stacking them together.

`BoundProject` owns and validates a bounded snapshot. Target and recipe checks
reuse the execution program's validators; conflicting meanings for one target
or configuration-scoped recipe ID are refused. Resolving a contribution requires
the explicitly named setup and configuration, never a display-name match or a
"latest" lookup. The host must load that immutable setup from trusted storage;
resolution validates capabilities, not the authenticity of caller-supplied
configuration data. An objective with no contribution is not yet a fully bound
intent snapshot; draft editing needs a separate UI state.

This model establishes intent only. It does not infer FOV or
sampling equivalence, judge quality, sum integration from different rigs, issue
assignments, or project progress. Frame goals are scoped to each contribution;
depth/cadence objectives and rig-optics compatibility remain required. The
coordinator must separately reserve outstanding allocation and bind intent
provenance before an executor can use it. Existing program and IPC contracts
are unchanged.

Meta schema 3 persists these project-intent snapshots and a foreign-key-backed
inventory of their rig-setup references. Registration validates the shared-core
contract, canonical project/rig/setup identities, project ownership, and every
recipe against its stored immutable setup in one short transaction. Identical
retries are idempotent; changing content or moving an existing snapshot to a
different project returns a conflict. A changed plan needs a new snapshot ID.
There is no implicit latest/active plan pointer or assignment issuance.

Reads use one SQLite snapshot and validate both the bounded payload and exact
reference inventory. Same-named projects and rigs remain distinct. Listings are
bounded, project-scoped ID pages, not revision chronology. Backup/restore retains
the plan, referenced configurations and instance identity. Upgrading schema 1
or 2 to 3 is transactional; older backups remain read-only until the restored
copy is explicitly opened for migration. HTTP editing, active-plan selection,
allocation accounting and native assignment delivery are not implemented here.

- [ ] Add opt-in meta storage, migrations, backup/restore, and stable mappings.
- [ ] Model sites, rig configurations, objectives, recipes, and contribution plans.
- [ ] Link existing catalogs without rewriting TS history or merging names.
- [ ] Add scoped project views that distinguish global and rig-local projects.

Gate: one project references two rigs/catalogs with different FOVs and distinct
short/long objectives; identities survive catalog relocation and projections
rebuild without counting mirrored captures twice.

#### Confirmed catalog mappings

The meta crate's schema 4 adds explicit source-profile-to-rig links and binds
each confirmed source project to that profile and a global project. A caller
first registers the stable catalog identity and creates or selects the global
project and rig. `link_catalog_project` then records both links in one writer
transaction. It never infers a rig from a database name, source row number or
project name, and never changes a source catalog.
`link_catalog_projects` applies up to 256 mappings in one transaction; a conflict
or storage failure rolls back the entire batch, including any earlier entries.

Profiles are scoped by catalog identity and retained as exact opaque source
IDs. Several profiles/catalogs may link to one rig, and several rig-local
projects may contribute to one global project. A profile within one catalog
cannot silently change rigs; a source project cannot silently change its
profile or global project. Identical retries succeed; changed mappings conflict.
Legacy project-only mappings remain intact but do not imply a rig. Migration
from schemas 1-3 is transactional and does not invent missing associations.

Complete mapping inventories are catalog-scoped and paged by source project
GUID, with at most 256 entries per page. They describe explicit associations,
not equipment compatibility, image attribution or acquisition permission. A
project's current profile does not prove every historical image came from that
rig. The adoption API verifies fresh source evidence, retains durable catalog
identity across relocation/copies, and previews the proposed links. The Catalogs
view uses a read-only, paged mapping inventory and requires explicit Apply.
Ambiguous historical frame ownership and contribution accounting still need a
separate workflow; these mappings alone do not resolve them.

### Phase 2: single-rig autonomous Director

- [ ] Implement versioned allocation, acknowledgements, checkpoints, and limits.
- [ ] Add durable event delivery, local recovery, offline operation, and status UI.
- [ ] Support native sequence safety/hooks and explicit Sync coexistence rules.
- [ ] Implement the shared-core operation state machine and the TS-style native
  container/options contract. Test configured trigger order and frequency,
  nested operations, cancellation, and failure propagation in real N.I.N.A.
  simulator sequences; report injected actions and durations back to the core.
- [ ] Enforce the current rig meridian/horizon snapshot at dispatch and refresh
  it on profile changes, horizon changes, and same-path file edits.

Gate: a real N.I.N.A. instance using simulated equipment handles slow autofocus,
failed centering, reprioritization, network loss, restart, and operator stop.
It never starts unauthorized work and reports ambiguous capture outcomes.
It does not start an exposure across a meridian exclusion or below the effective
local horizon, including after unexpectedly slow setup operations.

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

Maintain shared-engine fixtures across server and sidecar hosts. Protect
ordinary TS execution and existing Sync behavior with regression coverage.
Use an experimental Director release channel until these gates pass; do not
replace the stable Sync registry entry.

### Full-stack end-to-end gate

Once the Director plugin, shared crate/sidecar, native N.I.N.A. adapter, and
corresponding PSF Guard changes are available, run them together. Passing core
fixtures or the console process harness does not satisfy this gate. Run the first integrated
test as soon as those components exist, then repeat it after changes to their
contracts or execution behavior and before an experimental release.

- Build a separate local PSF Guard instance from the candidate changes. Use
  copied catalogs, isolated meta storage, a test registry supplied with
  `--registry`, and a temporary image receive directory. Do not write to live
  catalogs, production endpoints, or the user's real registry.
- Install the candidate Director plugin and its pinned runtime into the test
  N.I.N.A. setup without TS installed. Use simulated camera, mount, and
  other required devices, an isolated profile, and a real sequencer run.
- Pair with the local server, check in, obtain an assignment, select work in
  the shared engine, execute through native N.I.N.A. APIs, and record the
  resulting image and timing events. Verify progress in PSF Guard and the plugin
  UI, not just logs.
- Grade the captured work in PSF Guard and check in again. Confirm accepted
  work completes its objective, rejected work can request a bounded retry, and
  pending assessments do not cause duplicate acquisition.
- Exercise changed priorities, slow autofocus, horizon/meridian restrictions,
  operator stop, safety loss, server disconnection, sidecar failure, and restart.
  Verify recovery reconciles actual execution without replaying a stale decision
  or starting work outside the assignment or local safety constraints.
- In a separate coexistence pass, install TS and Sync and re-run their ordinary
  workflows. Check that conflicting acquisition ownership is rejected.
- Run Chatstronomy with TS-only and Director-only sessions. Verify accurate,
  source-aware state and target/wait/progress notifications, privacy and delivery
  controls, restart recovery, and permitted safe-boundary commands. Repeat
  Director acquisition without Chatstronomy installed to prove it is optional.
- Retain exact commits/package versions, sequence/profile fixtures, redacted logs,
  execution events, assertions, and UI captures with the relevant PR evidence.

Mark unavailable paths as untested and keep the gate open; do not substitute
mocked components for the missing integration. No current sidecar-spike result
claims this full-stack test has run.

Open design choices include objective depth equivalence across instruments,
coordinate/time precision policy, conservative expiry margins, duration-model
retention, and the supported N.I.N.A. execution API surface. Resolve these with
documented evidence in the relevant phase, not implicit defaults in implementation.

## References

- [Chatstronomy N.I.N.A. plugin](https://github.com/theatrus/chatstronomy-nina-plugin):
  existing TS message-broker events, sequence state projection, privacy controls,
  and safe-boundary commands must coexist with Director.
- [Data transfer and remote sync](data-transfer.md): existing directional merge
  and grading contracts; Director coordination is additive.
- [Multi-database architecture](multi-database.md): catalog scope and identity.
- [Calibration libraries](../CALIBRATION_LIBRARY.md).
- [Stack previews](../STACKING_PREVIEWS.md).
- [N.I.N.A. downloads](https://nighttime-imaging.eu/download/).
- [TS 3.3 source branch](https://github.com/tcpalmer/nina.plugin.targetscheduler/tree/release/nightly-3.3).
- [Local TS meridian fork](https://github.com/theatrus/nina.plugin.targetscheduler/tree/8b549b1123520add0cb65124f469e9cb5723b13d).
- [N.I.N.A. custom horizons](https://github.com/isbeorn/nina/blob/develop/NINA.Core/Model/CustomHorizon.cs)
  and [profile service](https://github.com/isbeorn/nina/blob/develop/NINA.Profile/ProfileService.cs):
  source inspected for file formats and profile change notifications; verify
  against the pinned nightly during adapter implementation.
- [Astro-PM N.I.N.A. integration](https://astro-pm.com/nina-sync/) and
  [project lifecycle](https://astro-pm.com/project-management/): product
  references for integrated planning and acquisition, not dependencies or
  specifications for this implementation.
