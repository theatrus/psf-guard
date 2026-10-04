# PSF Guard Director: goal-driven acquisition

Status: phases 0, 1 and 2 are partially implemented; no phase acceptance gate is
complete. Shared-core, durable sidecar and native simulator building blocks are
merged. The published Director 0.1.0.1 preview is runtime-only, not an acquisition
scheduler. See the implementation audit below before treating a capability as
available to users.
Last updated: 2026-10-04.

### Native imaging and local observability increment

The plugin now implements Director-owned native NINA centering/rotation,
autofocus, guiding, dither and meridian-trigger integration. NINA owns device
algorithms; the shared Rust core owns target choice, preparation issuance,
cadence and fresh dispatch checks. Native filter/time/temperature autofocus and
restore-guiding triggers remain in the normal sequence ancestry. Explicit
Sequence ownership keeps custom/third-party instructions available and detects
known duplicate native actions. Connect/cool and warm/disconnect stay in the
outer NINA sequence; the Session requires connected commissioned devices.

Meridian execution uses NINA's built-in trigger and flip VM, including profile
timing/pier-side rules and before/after flip events. A thin adapter checks the
returned boolean and exposed workflow-step completion before allowing another
capture; it does not implement another flip algorithm. Runtime-installed
defaults must not leak into saved/cloned user sequences. Native recovery steps
can still be best effort (for example, recenter); workflow completion is not
pixel-derived pointing evidence. The forced-flip simulator gate is now covered
below; rotation and real-sky acceptance remain open.

Workload API capability names are `native_single_target_v1` and
`native_imaging_v1`. Old modes retain their old restrictions. Native device IDs
and operation ownership participate in the local configuration fingerprint.
Actual preparation durations/outcomes are journaled; learned duration estimates
and preparation-feed upload to central PSF Guard remain unfinished.

Target Scheduler inspiration is deliberately at the adapter/display boundary:
its TargetSchedulerContainerTemplate uses NINA AltitudeChart with the native
target DeepSkyObject and NighttimeData; TSLogger and PlanExecutionHistory retain
action details and visit timing. Director reuses the same chart, adds a bounded
local action/outcome/time history, and emits those entries to NINA's log. Neither
view adds a second scheduling engine. The Sky tab displays the selected target
and native horizon; complete per-candidate eligibility/Moon/meridian overlays
remain planned. Local history works without connectivity. Central operation
telemetry must eventually batch the durable preparation receipts, not replay UI
logs as current rig status.

This does not close phase 2: optical solver/focus performance, native rotation,
real-sky flip/recenter and guider recovery, broad third-party hooks, quality
recovery, calibration acquisition, offline cold start and uncertain-work recovery
remain acceptance work.

#### Forced native meridian validation

The packaged plugin and bundled shared-core runtime were tested against an
isolated PSF Guard server, NINA 3.3.0.1064 and ASCOM OmniSim. After two saved
exposures the native trigger enters the real NINA flip VM, waits through the
actual meridian crossing, changes the mount from West to East, completes
autofocus and settling, and resumes with a third saved exposure. The test checks
ordered before/after flip events, live device pier side, workflow completion,
capture identity and local action history.

A second run injects a false mount-command result: Director detects the failed
native `Flip` step, blocks the third exposure, parks, preserves the two earlier
saves and releases local ownership. The pinned NINA VM still raises an overall
successful result/event for this case, so the workflow-step guard is required.

The timing deadline and optical focus/solve results are controlled test inputs;
status/window presentation is headless. Recenter is disabled and Direct Guider
does not exercise PHD2 reacquisition. This is orchestration evidence, not proof
of optical quality or physical mount safety. See the plugin's
`docs/nina-smoke-test.md` for commands and retained evidence.

### Operating goal after setup

Commissioning is explicit; routine acquisition is automatic. An operator pairs
the rig, reviews its equipment/site/horizon and hard limits, permits projects
and templates, and enables a local operating policy. After that, starting a
Director Session should not require manually reporting unchanged equipment,
choosing each target, downloading plans, or admitting every allocation.
The current prepared-target/manual-admission path is a tested bootstrap, not
the intended finished workflow.

PSF Guard accepts project workloads expressed as observing goals and compatible
per-rig contributions. Director submits workload requests and check-ins with
capabilities, local conditions, available session time and durable progress.
The coordinator uses the shared Rust planning policy to allocate bounded work
from eligible active plans. Director uses that same core to choose the next
target, filter and exposure from its authorized workload, reevaluating after
real operation delays and at safe boundaries. The C# adapter must not add its
own priority, filter-selection or scheduling policy.

TS-style local execution is the primary observing loop, not an optional
fallback for remote plan playback. An admitted workload can contain multiple
targets and recipes. The on-rig shared core selects what is useful now from
local priority, visibility, horizon, meridian, equipment and progress evidence.
Online allocation/check-in supplies bounded authority, corrective revisions
and quality reinforcement; it must not authorize each ordinary target switch.
There is still no TS runtime dependency or second scheduler in C#.

Shared planning priorities resolve global defaults and site/rig/project
overrides before scoring feasible work. Site, horizon, weather, equipment
capabilities, pending captures and learned operation durations remain inputs;
a higher priority cannot override local safety or hard limits. A workload is
not a timestamp script, and the coordinator is not a remote hardware controller.

Unchanged commissioned context should refresh automatically during check-in.
Material equipment, profile, site or hard-limit changes require review under
a versioned commissioning policy; they must not be silently adopted from a
client report. Explicit report/review controls remain available for setup,
diagnostics and exceptions, not as nightly steps.

The first automatic-workload increment implements commissioned request intake,
idempotent grants, clean terminal release and carried attempt budgets for the
prepared-target executor. The local multi-target increment below extends it;
inherited policy resolution, grade feedback and crash/uncertain-work recovery
remain unfinished.
Phase 2 must finish those gates. Every grant still has a one-shot launch.

### Local multi-target execution increment

The opt-in **Local target scheduling** session mode removes the adapter's
one-target restriction. The existing Rust geometry/ledger core selects the
highest-priority feasible goal after saves and native operation boundaries;
the C# adapter only resolves that choice to its immutable target and recipe.
Its inputs include live NINA site/horizon/altitude/meridian constraints and
durable pending/attempt counts. Sequence-owned target preparation runs as the
core's `BeforeTarget` operation after native unpark/tracking, with measured
elapsed time recorded locally. Clean halted preparation can close and reselect
after slow setup changes which goal is feasible; uncertain work still stops.

Target visits use the seven TS-style slots, ordinary inherited conditions and
triggers, and native inherited-coordinate slew/center instructions. Visits to
different targets close the old target's hooks before starting the new target's
hooks. Waiting ends a visit; setup runs again on return. A saved frame is pending,
not accepted completion. The adapter verifies mount pointing before capture;
this is not pixel-derived solve evidence.

Target/periodic check-ins run in a bounded background pump. An outage cannot
gate the next local target choice within the current grant. Automatic intake
advertises `local_sequence_v1`; old sessions retain `prepared_target_v1`.
Both reject rotation and Director-owned centering/dithering until those native
policies are implemented. Unresolved intake cannot change execution modes.
Final feed delivery, park and release remain necessary for successor authority.

Implemented: local multi-target priority execution, target-context transitions,
one-shot setup receipts, clean reselect and asynchronous reporting. Not
implemented: automatic AF/guiding/center/dither/flip policy, duration-model
learning, online priority/grade corrections applied to active grants, a TS DB
refresh during a live grant, or offline cold-start/recovery admission. Priorities
currently come from the activated/imported plan objectives. New TS imports
preserve project Low/Normal/High priority (0/1/2) across their objectives,
instead of inventing a filter priority from alphabetical bandpass order.
Missing/null/unknown source priority defaults to Normal; later TS edits do not
overwrite an existing Director draft. Full inherited
policy/TS scheduling parity remains a phase-2 gate. Commissioned acquisition
and the new local mode remain explicit opt-ins. This does not complete a phase
gate or change the published runtime-only preview.

### Automatic workload exchange increment

Meta schema 18 adds an interactive, compare-and-set workload policy and retained
grant history. Commissioning binds one live client/profile to the existing
catalog/rig, an exact reviewed rig-profile revision and configuration, and a
bounded allowlist of active project IDs. The compiler must not add an
uncommissioned project. Equipment changes require review and recommissioning;
an enabled plugin cannot self-approve them.

The plugin's opt-in **Automatic workloads** mode
persists a request UUID before sending it. Paired requests return `issued`,
`released` history, or `waiting` with bounded backoff. An outstanding grant,
including an expired or offline one, prevents another grant. Lost replies reuse
the same UUID; changed snapshots cannot replace a received grant. The shared
Rust selector continues to choose goals and recipes, not the C# intake loop.

A successful session stops hooks and dispatch, verifies no unresolved capture
or active preparation, parks, and delivers the entire capture feed before
terminal release. The executor attests quiescence and parking; the server checks
the launch ledger, contiguous cursor, exact grant/engine identities and settled
reservation transitions. Gaps, uncertain outcomes, reused capture IDs, stale
scope and failed shutdown prevent release. A sealed feed accepts duplicate
delivery but no later events. A successor retains the first per-goal attempt
cap minus all reserved attempts; failures also spend authority. Per-goal budget
totals update atomically with release, avoiding a scan of old grant payloads on
each request. Saved images
remain pending, not accepted. History is bounded to 4096 grants per rig without
automatic authority-erasing pruning.

The prepared-target session can request its next workload without operator
admission, or wait for quality assessment without recapturing pending credit.
Offline completion retains the outstanding grant and local evidence; it does
not silently authorize a successor. Legacy manual allocations cannot use this
release path. General recovery, preparation-feed reconciliation, rejected-frame
feedback, additional budget approval and multi-target defaults remain open.
The commissioning/recovery UI remains a frontend handoff; extend existing rig
Setup rather than introducing new catalog or rig identities. The exact routes
and bodies are documented in [Director management](../DIRECTOR.md#automatic-workload-policy-and-exchange).
Collaboration stays deferred.

### Native equipment review handoff

Meta schema 17 stages one bounded equipment report per paired NINA client.
The plugin's explicit **Report equipment** session command uses the same native
snapshot as acquisition, including the session's constraint fingerprint, and
works without enabling acquisition. Reports are bound to the coordinator,
catalog/rig, client and native profile; credentials stay in the OS vault.
Reports cannot grant launch authority or silently replace planning inputs.

The API stages paired `POST /rigs/{rig}/equipment-reports`, lists reports for an
operator with `GET` on that path, and accepts a reviewed report with interactive
operator `POST /rigs/{rig}/equipment-reports/{client}/accept`. Acceptance uses
the exact report UUID and rig-profile compare-and-set revision in one
transaction. It preserves manual optics, site, horizon, limits, sky quality and
peer linkage. Stale observations, revoked pairings, changed reports/profiles
and outstanding allocations fail closed. Reports and acceptance are retry-safe;
revocation removes reports without erasing accepted setup.

Implemented: staging, transactional acceptance, strict native report transport
and session command. Not implemented: operator review UI, readiness summary,
allocation admission UI, successor/recovery reconciliation, automatic optics
or site commissioning. Frontend integration should extend the existing rig
Setup editor, not introduce another rig identity or database. Display the
paired profile and report age, compare current and proposed capabilities, then
submit explicit acceptance with `expected_revision`. A received or accepted
report is not an allocation or launch permit. The full wire contract and
review restrictions live in [Director management](../DIRECTOR.md#reviewing-native-equipment-evidence).

Local acceptance on 2026-10-01 used NINA 3.3.0.1064, ASCOM OmniSim and
runtime 0.7.0 / IPC 8 with a disposable schema-17 server. Reporting with
acquisition disabled left active configuration empty; explicit review preserved
manual optics. The public session then saved three frames, checked in six
events after an outage and refused replay. A separate public unsafe run
aborted exposure, parked and stayed stopped after recovery. The plugin smoke
guide records both evidence paths. This does not complete any phase gate or
change the published runtime-only preview.

### Public prepared-target acquisition increment (locally validated)

The plugin increment connects the public `Director Session` container to
the issued allocation and native capture adapters. It is deliberately a
prepared-target mode, not the complete automatic scheduler: one target, no
requested rotation, and native sequence ownership of centering, autofocus,
guiding, dithering and meridian flips. The operator must prepare the telescope;
the executor checks reported pointing within three arcminutes and tracking at
each exposure boundary. Mount coordinates are not a pixel solve. Automatic
multi-target acquisition and automatic operation policies remain unfinished.

The session requires a connected, freshly safe monitor, matching native
equipment/horizon/site context, dated Earth-orientation evidence, and an
operator-issued allocation for its exact pairing. An explicit enable toggle is
off by default. All seven native hook slots remain available; inherited native
triggers and conditions run around transient native operations/exposures.

Meta schema 16 adds `execution_start`. The paired executor posts
`POST /api/director/v1/rigs/{rig}/allocation/start` with coordinator, catalog,
allocation and ledger UUIDs. Only the selected live client/profile can launch.
The transaction consumes the allocation's single launch, even if the response
is lost. Identical retries, another ledger, expiry and process restart cannot
refund that launch. Automatic workloads now have the clean-release successor
path above, but no crash/uncertain-work resume endpoint. Do not delete
allocation or launch records to recover a rig.

First launch must be online. After acknowledgement the running session may
continue offline within the original budget and expiry, with a local owner
lock, continuous safety cancellation and monotonic/UTC clock checking. Only
that process holds launch authority: caches and serialized sequences do not.
Reservations, receipts and capture journals remain on disk for reconciliation;
check-in uses the existing idempotent batch receipt protocol. Periodic status
reports are optional. Authentication failures stop the session; transport
outages may continue only when offline continuation is enabled. Shutdown
attempts to park the original connected telescope, never a replacement profile.

This does not promise exclusion against arbitrary external hardware clients,
automatic recovery of uncertain exposures, offline cold starts, or mid-session
plan replacement. NINA must be the only equipment controller. The native
startup operation unparks and establishes verified sidereal tracking before
the shared core records it succeeded.

Local validation used NINA 3.3.0.1064, ASCOM OmniSim and the pinned runtime
0.7.0 / IPC 8 against an isolated schema-16 server. The public container saved
three FITS frames during an outage, delivered six capture events after
reconnection, preserved pending credit and refused a second launch. A separate
public run interrupted a 30-second exposure through native Unsafe evidence;
the camera aborted, the mount parked and Safe recovery did not restart work.
The plugin smoke guide retains evidence paths. These are local checks, not
hosted CI or full lifecycle acceptance. The published 0.1.0.1 runtime-only
preview is unchanged until an explicit release. Dated sections below retain
historical validation boundaries; this section and the audit describe current
prepared-target support.

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

Director covers acquisition planning and acquisition within that lifecycle; it
does not replace PSF Guard's Library or take ownership of catalog review,
grading, calibration inspection or processing. Library remains a key project
and collected-data view, including the per-rig catalogs already represented
under its projects. Both surfaces refer to the same projects.

The existing PSF Guard Sync plugin remains a separate supported product.
Director must not require users to migrate away from Sync or change existing
sync semantics.

## Architectural decisions

- Introduce a PSF Guard meta database for coordination above per-rig databases.
- Make global projects independent of rigs, database files, and local TS IDs.
- Present one PSF Guard project with rig-specific contributions, not a second
  Director project/catalog application alongside existing PSF Guard projects.
- Use each existing registered per-rig project database as the rig in Director.
  Do not require another independently maintained rig record or catalog list.
- Exchange goals, constraints, and bounded assignments, not fixed timetables.
- Run the same planning core in PSF Guard and Director.
- Own scheduling and feedback policy in the shared Rust core; use a thin
  Director adapter to execute through supported N.I.N.A. APIs.
- Use TS as inspiration and for optional import/coexistence, not as Director's
  planner, execution engine, or required database.
- Keep equipment control, safety, and operator overrides local to N.I.N.A.
- Start with one coordinating instance per project. Federation is a later phase,
  not multi-master editing of the same plan.
- Own PSF Guard's internal catalog and Director schemas. Target Scheduler is an
  import and bidirectional sync integration, not the required storage schema.
- Support ordinary N.I.N.A. Advanced Sequencer hooks and third-party extensions,
  with useful native defaults that do not require optional plugins.
- Support connected rig monitoring and intentionally offline acquisition with
  bounded cached authorization and batched check-in. Neither mode requires
  image uploads to keep control/progress evidence moving.
- Define planning behavior as global defaults with optional site, rig and
  project overrides, not a required copy of scheduling settings on every project.
- Provide a full project framing wizard, from sky coverage and mosaics through
  rig-specific contributions and coordinated planning across sites. A combined
  project does not imply that every participant's images belong in one stack.

## Implementation audit

Audited 2026-09-26 against PSF Guard main `dfd6a66` and Director plugin main
`8f6d8c2`, plus the open PR heads listed below. **Merged building block** does
not mean a production workflow or phase gate passed. Open PR work is not in
main. Update this table and the relevant checklist when a PR lands; keep
untested integration requirements unchecked.

| Area | Implemented evidence | Still missing |
| --- | --- | --- |
| Shared engine | Merged `crates/director-core`: deterministic selection, program/recipe binding, preparation reducer, conservative altitude/horizon and meridian geometry; shared Rust/.NET fixtures. | Complete observing criteria, production Earth-orientation source, full operation inventory, duration learning and server simulation. |
| Planning policy inheritance | Saved global/site/rig/project overrides, planning controls and immutable program policies drive the shared geometry scorer and durable execution selection. Explicit opt-in preserves legacy ranking. | Cross-allocation continuity, displayed candidate score explanations and adaptive timing estimates remain open. |
| Sidecar and local recovery | Merged `crates/director-ledger` and `crates/director-runtime`: schema-4 journal, capture/preparation outboxes, IPC 8/runtime 0.7.0, one-shot dispatch checks with exact latest-start deadlines, process crash/reopen tests; PSF Guard #464-487 and [#518](https://github.com/theatrus/psf-guard/pull/518). The current commissioning increment delivers capture receipts after restart and retains exact acknowledgements. | Preparation-feed delivery, pruning, grade feedback, assignment replacement and complete operator recovery. |
| NINA native execution | Public opt-in prepared-target session with one-shot server launch, exclusive local ownership, native safety/watchdog, dated NINA EOP, native unpark/tracking/filter/readout/capture, seven hooks, batch check-in and optional status. Real #64/OmniSim/server tests cover offline RGB captures, replay refusal and Unsafe interruption during exposure. | Automatic multi-target work, all third-party hook contexts, full autofocus/guiding/flip/calibration, restart/resume, duration learning and successor allocation recovery. Arbitrary external equipment clients are not excluded. |
| Published plugin | [0.1.0.1-preview.1](https://github.com/theatrus/psf-guard-director-nina-plugin/releases/tag/0.1.0.1-preview.1), runtime 0.6.0 / IPC 7; verified in the existing [registry](https://nina-plugins.psf-guard.com/plugins/manifests). | Settings expose runtime Start/Stop/status only. No pairing, assignments or usable acquisition container. Sync remains a separate unchanged plugin. |
| Meta storage | Separate schema-18 store on by default beside the registry; hashed pairing/client records; rig receipts, cursors and live status; stable previews, immutable allocations, one-shot ledger-bound launches, commissioning policies, settled workload history and transactional per-goal budgets; rig profiles, equipment reports, framing/plan drafts and activations; reviewed database/rig bindings, confirmed source project links, CAS renames, immutable sites/setups, transactional migrations and backup/restore. Unambiguous prototype links retain rig IDs. | Conflict-resolution UI for ambiguous prototype rigs, project-level permissions, active revisions, uncertain-work recovery and progress projections. |
| Project intent | [#492](https://github.com/theatrus/psf-guard/pull/492): shared objective/contribution model. [#493](https://github.com/theatrus/psf-guard/pull/493): schema-3 intent persistence with validated immutable setup references. Plan drafts with an objective editor, per-rig template binding and core exposure defaults. | Depth/FOV/sampling compatibility, per-night feasibility and authoritative allocation remain open. |
| Project framing wizard | Framing view in the project workspace: survey backgrounds from N.I.N.A.'s HiPS list, name resolution through CDS Sesame, interactive center, angle and zoom, mosaic rows/columns/overlap, per-rig footprints from rig profiles, draft editing with compare-and-set, and a visibility panel with tonight's altitude chart, custom horizon, Moon and darkness like N.I.N.A.'s framing assistant, plus a week of nights per rig. | Reference images with WCS, layer comparison, offline region cache, rotation feasibility per rig. |
| Catalog discovery | [#498](https://github.com/theatrus/psf-guard/pull/498) merged: operator-scoped read-only project/profile evidence from registered TS-compatible catalogs, with bounded results and invalid/duplicate identity reports. | Discovery does not infer rig ownership or read image history. |
| Catalog adoption | [#502](https://github.com/theatrus/psf-guard/pull/502) merged explicit durable lineage; [#503](https://github.com/theatrus/psf-guard/pull/503) merged operator preview/apply, exact identity matching, stale-review refusal and interrupted-write recovery. [#506](https://github.com/theatrus/psf-guard/pull/506) merged the reviewed mapping UI and read-only inventory, with real-server browser tests. | Independent forks, historical-image attribution, contribution accounting and acquisition authorization remain separate work. |
| Operator API and UI | The Library lists plans (`GET /plans`: links, stage, per-rig state and capture dates) as its own rows and families, with plans that have captured nothing in a section below; the header's scope (one project picker, Workspace, then a rig and target switcher for Images and Sequence) opens a plan's workspace at `/plan?plan=<TS GUID or plan id>`, which holds framing, plan and activation over the Library's existing target/exposure editor; rig setup and the template library are Settings tabs; Live is drawn on the Sky with the table under it. The Planning list page, the identity lists and the Catalogs tab are retired; old `/director` links redirect. Desktop/mobile real-server tests cover two databases contributing to one project and return navigation. | Pairing-code management UI and acquisition control. The Live table shows connectivity, last report, contact ages and assignments per rig. UI tests are not equipment tests. |
| Program delivery | #528 adds `GET /rigs/{rig}/program`, compiling activated rig rows and reported configuration into core Program plus identity links and rig context. Plugin #28 adds bounded, read-only preview intake. Admission freezes a reviewed snapshot for one paired client/profile; the plugin validates and durably caches it separately from previews. | Commissioning/admission UI, uncertain-work recovery and quality feedback. A preview remains non-executable; a stored allocation still requires exclusive local ownership, safety and ledger recovery. |
| Automatic workload exchange | Interactive CAS policy for the exact reviewed rig/catalog/client/profile/configuration and active project scope; durable paired request IDs, immutable retry results, bounded pending-assessment wait, verified capture-feed terminal release and shared-core carried attempt budgets. The prepared-target public Session requests, parks, seals and waits automatically; a real NINA #64/OmniSim/schema-18 test covers three FITS and six acknowledged events. | Multi-target operation defaults, policy inheritance, grade feedback, uncertain/preparation reconciliation, additional-budget approval, commissioning UI and collaboration. Legacy manual grants cannot silently enter this protocol. |
| Native catalog schema and TS exchange | Existing PSF Guard/Sync transfer paths are migration references, not the new implementation. Meta and execution stores already use owned schemas. | PSF Guard per-rig catalogs are still TS-shaped. Native catalog schema, import/adapters, explicit TS connections and migration/round-trip gates are planned. |
| Connected and batch check-in | Local bounded outboxes and durable pending accounting are merged foundations. #530 adds server receipt ingestion/contiguous acknowledgements and coalesced status routes; #531 shows reported rig status in Director. The current commissioning increment adds separate scoped pairing and a bounded capture sender with durable, identity-bound cursors, tested against a real local server after a native simulator run. | Preparation feed, production background/status delivery, grade feedback, offline authorization lifecycle, manual/sequence batch reconcile and replacement activation remain missing. |
| End-to-end lifecycle | PSF Guard allocation -> public NINA simulator session -> offline captures -> central batch receipts/status and replay refusal is locally tested, plus Unsafe cancellation. Automatic commissioned intake -> parked clean release -> pending-assessment wait is also locally tested. | Central grading -> reconciliation/replan, uncertain-work recovery, multi-target successors and TS/Sync/Chatstronomy coexistence gates remain open. |

## Domain model

| Entity | Responsibility |
| --- | --- |
| Global project | Desired result, targets, objectives, participants, lifecycle, and outputs. |
| Observation objective | Required coverage, bandpass, exposure purpose, quality, depth, resolution, or cadence. |
| Site | Latitude/longitude/elevation, time context, and weather inputs for planning and acquisition. |
| Rig | An existing registered per-rig project database, with site association and versioned equipment/configuration evidence. |
| Contribution plan | How a particular rig can satisfy an objective, including framing, panels, recipes, and quality requirements. |
| Assignment | Versioned, bounded authorization for an executor to pursue specified contributions. |
| Contribution | Captured data, provenance, assessment state, and its relationship to objectives. |
| Execution event | Durable evidence of an operation, capture, decision, or state transition. |

A global project can map to multiple rig-local contributions and catalogs,
including optional imported TS projects. Director does not require a TS project
to acquire. Persist mappings by stable identifiers. Do not join by project name,
telescope name, nearby coordinates, or database-local integer IDs. Existing TS GUIDs remain
intact. Database slugs remain URL scope, not federation identity.

A rig follows the durable identity of its registered project database, not its
current file path, display name or URL slug. Record equipment and site
configuration revisions so a changed camera, telescope, or location does not
rewrite history. A database may contain multiple projects and historical
configurations; neither creates another rig by itself.

### Rigs are registered project databases

The per-rig databases already visible through Library are Director's rig
inventory. This is why another "New rig" workflow and manually assigning each
TS profile to an unrelated rig identity is the wrong default: it asks users to
reconstruct information PSF Guard already has and lets the two inventories
disagree. Selecting a participating rig means selecting one of those existing
database contexts. One database can hold many downstream projects; this does
not mean one new database per target or per shared project.

PSF Guard's own tables in a rig database (`psf_guard_catalog_identity` and
the `psf_guard_director_*` side tables) are plain SQL with type CHECKs, never
`STRICT`: the file is shared with N.I.N.A. and Target Scheduler, and a keyword
their SQLite does not know would make the whole schema unreadable to them.
Sync moves Target Scheduler tables only, so identity spreads by file copy
alone; the Director treats two registered files with one identity as one
catalog, names the copy and plans with the first by slug.

Keep stable catalog lineage for moves, renames and synchronized copies. A copy
of a database does not become a second telescope or earn extra project credit.
A genuinely independent database/fork needs an explicit distinct identity. An
internal rig surrogate key may remain for references, but it must bind to the
canonical database identity rather than become another user-managed inventory.
Do not derive identity from names, paths, source-profile strings or integer IDs.

The Director plugin pairs/checks in against its selected database context and
combines its associated site information with the local N.I.N.A. setup and
current conditions. Sites supply position, time and weather; camera, optics,
filters, horizon, meridian limits and safety evidence describe the database's
current rig setup. A TS/N.I.N.A. profile is configuration provenance within that
rig, not automatically another rig. Preserve and report ambiguous historical
equipment provenance rather than pretending all frames used today's setup.

The meta database coordinates project intent and contributions across those
database-backed rigs. Each downstream project belongs to a participating rig
database and links back to the same shared project. Library still owns catalog
review and combined progress; Director plans and acquires contributions. Reuse
TS project metadata to seed that planning flow, not to create parallel catalogs.

This supersedes the independent rig-identity/profile-assignment model in the
early metadata API and mapping UI. Schema 5 now binds registered database
lineage to a rig after review and uses the registry as the normal rig inventory.
Prototype links are migration history, not a second inventory. Migration
must preview existing links, flag conflicts such as two independent catalogs
sharing one prototype rig, retain source GUIDs/history, and be transactional and
retryable. Do not silently rewrite a user's mappings or invent rigs at check-in.

### One project, multiple rigs

The user owns one PSF Guard project. Adding a rig gives that rig a contribution
plan for the same project; it does not require the user to create another
independent project and roll it up manually later. "Global project" names the
coordinator's stable identity in the implementation, not a separate product
concept or a second list of projects users must maintain.

The project owns targets, desired coverage, objectives and completion criteria.
Each participating rig owns the capture details needed to contribute: its
versioned optical setup, framing and rotation, mosaic panels, sampling, filters,
exposure lengths and acquisition recipes. A short-focal-length rig may cover a
target in one frame while a longer-focal-length rig needs a mosaic. They still
work on the same project. Shared-core compatibility and coverage checks decide
which objective each contribution can satisfy; assigning a rig does not make
its data interchangeable with every other rig's data.

Accepted, quality-assessed contributions roll up to the project's objectives
with their rig, setup, panel, bandpass and exposure-purpose provenance intact.
Pending captures remain pending, rejected or invalidated data can reopen work,
and mirrored catalog records never earn duplicate credit. Show combined project
progress with a per-rig breakdown. Combining a project does not force a single
image stack, equal raw exposure counts, or equivalent depth across instruments.

Catalogs remain PSF Guard's existing database infrastructure, not Director-owned
duplicates. Native rig-local storage holds capture history and projections of
the shared project identity. Imported TS projects retain their source GUIDs and
explicit adapter links, but those rows are not a requirement to create another
user-facing project. An existing project can gain planning and rig participation
through a reviewed adoption workflow that reuses its data and identity links.
Never silently combine same-named projects or infer rig ownership from a name.
Shared GUIDs are identity: the same project synced to two rig databases is one
plan with two rigs, taken in without an operator step.

Keep Library and Director as complementary views of the same project, not one
replacement workspace. Library retains project browsing, per-rig catalog
contributions, collected-data summaries and entry points to review, grading and
processing. Director provides framing, objectives, rig participation, planning
and acquisition status. A project can exist in Library without using Director;
planning can also begin before a project has any captured data.

Provide contextual navigation between an Library project and its Director
plan, preserving project identity and any applicable rig/database scope on the
return path. Shared progress projections can appear in both views; neither view
creates a second project or independently editable copy of acquisition history.
Adding planning to an existing project must not move or recreate its catalog.

Reuse existing database settings for source/import/sync connections and expose
source links in project context when needed. The current experimental Director
identity lists and Catalogs mapping tab are implementation scaffolding, not the
intended catalog navigation. Retire that parallel catalog workflow without
removing Library's projects or per-rig catalog access.

### Planner entry and source metadata

Extend the existing Library project/target dialog ("Plan & coordinates") into
the planner workflow. Its TS-backed project description, target coordinates and
rotation, exposure plans, template capture settings and desired counts provide
the starting data; do not ask users to enter these again in an identity form.
Reuse the existing editor and read APIs before introducing additional forms.

The next steps are framing against survey backgrounds, choosing participating
rigs and their optical setups, reviewing rig-specific downstream projects, then
activating the shared intent and bounded assignments for Director. Downstream
projects are contributions to the originating project, not independent top-level
campaigns. Library retains their collected data and the combined project rollup.

Seed a reviewed draft, not immediate acquisition authority. Preserve source GUIDs
and metadata revisions; convert RA hours at the planning boundary and handle the
declared coordinate epoch explicitly. Resolve template defaults before producing
capture recipes. Rig constraints and inherited policy still come from their
proper owners; importing TS project settings must not silently override them.
Generating or updating downstream projects requires preview/apply and must not
overwrite existing acquisition history. The shared core validates the resulting
intent before the plugin may receive an assignment.

### Planning flow implementation plan

Decisions taken on 2026-09-26 for the PSF Guard side of the plugin/backend
split. They narrow the sections above for this stage; they do not replace them.

- **Store on by default.** The meta store opens beside the database registry
  whenever database management is on, in the server and the desktop app.
  `--director-meta` only moves it. A read-only server leaves Director off.
- **Rig databases keep TS-shaped tables** as the execution projection: one
  `project` row per downstream project, one `target` row per mosaic panel, and
  `exposureplan` rows per recipe, all with stable GUIDs. Director provenance
  lives in PSF Guard-owned side tables in the same file, keyed by those GUIDs
  (see below). Import, export, Sync and other PSF Guard instances keep working
  unchanged. The native catalog schema stays planned work.
- **Own sky map.** The framing view draws on PSF Guard's existing sky
  projection code with server-fetched HiPS2FITS cutouts from the surveys
  N.I.N.A.'s framing assistant uses (DSS2 color, Finkbeiner H-alpha first).
  Cutouts are cached under the cache root. No Aladin Lite and no dependency on
  a running N.I.N.A. or its framing cache directory.
- **The plugin pulls, like Sync.** PSF Guard never pushes into a rig. The
  Director plugin fetches its program over HTTP, runs semi-offline within that
  program's validity, and checks in later with journaled receipts. Live status
  while connected is welcome but never required for execution.
- **A remote rig is a pulled copy plus a peer.** A rig at another site is
  represented here by the database pulled from that site's PSF Guard, a
  registered Sync peer. Its rig profile names the peer; activation writes the
  copy, then hands the planning tables to the peer through Sync's planning
  push, so the same GUIDs exist in both files and the remote Target Scheduler
  runs them. The remote PSF Guard stores those rows; it does not plan them.
  The plugin at that site still pulls its program from the coordinator.
- **Optics and site come from N.I.N.A.** Sensor size, pixel size, focal length,
  aperture, rotator presence and camera angle offset, plus site location and
  horizon, are defined in N.I.N.A. profiles. The plugin syncs them into a rig
  setup revision; the same fields are editable per rig database in Settings
  for rigs that have not checked in yet, with the source shown.
- **Exposure templates stay.** Plans bind to each rig database's exposure
  templates, as Target Scheduler does. The project owner enters desired
  accepted integration as hours or frames per bandpass and purpose. A rig's
  contribution converts between the two through its template exposure length.
  Default exposure lengths come from the rig (focal ratio, aperture) and the
  site's sky quality; darker sites and slower optics default longer.

#### Rig database side tables

PSF Guard owns these tables inside each rig database. Target Scheduler ignores
them, Sync copies them as opaque planning data once its adapter learns them,
and they never carry credentials or authority.

| Table | Key | Holds |
| --- | --- | --- |
| `psf_guard_director_project` | `project_guid` | global project UUID, coordinator instance UUID, activation revision, applied-at |
| `psf_guard_director_target` | `target_guid` | project GUID, panel ID, framing revision |
| `psf_guard_director_plan` | `exposureplan_guid` | target GUID, contribution ID, objective ID, bandpass ID, exposure purpose, required accepted frames, plan revision |

Preview/apply writes these in the same transaction as the TS rows they
describe. A later activation with a new intent revision updates them in place;
it never rewrites `acquiredimage` or existing grades.

#### Plugin-facing endpoints

All under `/api/director/v1`. The commissioning increment adds Director-specific
pairing for program inspection and capture/status reporting; see the exact
[pairing contract](../DIRECTOR.md#pair-a-director-client). Equipment registration
remains operator-managed. Sync keys and operator tokens are not plugin enrollment.
Every request
names the coordinator instance, catalog and rig UUIDs and is refused on a
mismatched tuple even when the slug exists.

| Endpoint | Owner | Purpose |
| --- | --- | --- |
| `GET /rigs/{rig}/program` | backend | The current preview for the rig, built from its activated plans and reported equipment: core `Assignment` (stable 24 h validity, one goal per eligible activated exposure plan with live `accepted`) and `Program`, `links` joining each goal to its project, objective, contribution, panel, source project GUID, target GUID and exposure plan GUID, `rig` context (site, horizon, limits, rotation), and `omitted` reasons. Unchanged pulls retain exact content across restart. `If-None-Match` gives `304`; `422` until equipment and executable activated work exist. Not an acquisition lease. |
| `PUT /rigs/{rig}/equipment` | backend, operator-authenticated only | Reports core `Configuration`, optional complete `filter_names` keyed by its filter IDs, optics, site, horizon and limits into the rig profile with `source: plugin`. The tuple must match the server's `catalog_rig` binding (`403` otherwise); an identical report is a no-op. Paired clients do not yet have this write permission. Activation will later freeze setup revisions from the profile and mark active plans stale when it changes. |
| `POST /rigs/{rig}/checkin` | backend merged; plugin next | One ledger's `ExecutionEvent` page (≤256, ascending, verbatim) with the held `program_revision`; the reply acknowledges `acknowledged_through` for that ledger, names duplicates and conflicts per sequence, and says `program_changed`. Saved receipts feed `pending` in the next pull. Grade application and replacement-assignment proposals are still open. |
| `POST /rigs/{rig}/framing-cache` | planned, lower priority | The plugin uploads entries of a rig's own framing cache. Whole-sky imagery no longer needs it: the server reads N.I.N.A.'s downloadable `FramingAssistantCache` sets from `<cache>/director/sky-maps` (`psf-guard sky-maps install`) and renders any view from their tiles (`src/sky_maps.rs`). What remains for the upload is a rig's plate-solved captures. |
| `POST /rigs/{rig}/status` | backend merged; plugin next | Coalesced live status per session, newest wins, late reports refused; `GET /rigs/status` is the operator view. Loss of it changes connectivity only. The Live table reads these payload fields when present: `phase` (or `state`), `target_name` (or `target`), `operation` with `operation_started_ms`, `wait_reason`, `safety`, `queue_depth`, `errors` (or `error`); send them under those names. The Sky's Live view also reads `pointing: { ra_degrees, dec_degrees }`, the mount's current position in ICRS (J2000) **degrees**: right ascension 0–360, not N.I.N.A.'s hours. Convert with the mount's reported `EquatorialSystem`: JNow coordinates go to J2000, J2000 ones pass through. Send it with every report while the mount is connected and leave it out when it is not. Without it the Sky marks the rig at the centre of the Target Scheduler *target* named in `target_name` (the target row, one per mosaic panel, not the project), matched without regard to case among that rig's plan targets, and labels the place as taken from the target. The chip and the Sky count a rig as exposing only for `phase` values `exposing`, `imaging` or `capturing`; send one of those while a light frame is being taken. |

The plugin must not read TS tables from the rig database as its planning
input and must not call an endpoint before its row above says it exists.

#### Delivery order

1. Store on by default; this plan. Done in the PR that added this section.
2. Rig optics, site, horizon, sky quality and limits in a mutable rig profile
   (meta schema 6), edited per database with header-derived defaults and
   accepted from `PUT /rigs/{rig}/equipment`. Done. Activation freezes a setup
   revision from it later.
3. Survey cutout service with cache and provider allowlist: `GET /sky/surveys`
   and `GET /sky/cutout`, N.I.N.A.'s HiPS list, tangent-plane JPEGs from
   HiPS2FITS fetched off the request path. Done.
4. Framing view. Done: `director-core::framing` (gnomonic plane, mosaic
   layout, view-relative corners), framing drafts in meta schema 7 with
   compare-and-set, `POST /framing/preview`, `GET/PUT /projects/{id}/framing`,
   `GET /rigs/profiles`, and the browser view in the plan workspace: survey
   image with pan and zoom, panel rig or typed panel size, mosaic grid and
   overlap, rig overlays, and draft save. Still open from the wizard section:
   reference images with WCS, blink or opacity comparison of layers, a
   prepared offline cache for a region, and per-rig rotation feasibility.
5. Objectives and contributions. Done: plan drafts in meta schema 8
   (objectives per bandpass and purpose with hours or frames goals; per-rig
   contributions bound to a Target Scheduler template with an exposure
   length), `GET /catalogs/{slug}/templates` with core bandpass resolution,
   `GET/PUT /projects/{id}/plan`, core default exposures from focal ratio,
   sky quality and band kind, and the Plan editor in the project workspace.
   Per-night feasibility done: core Sun/Moon ephemeris and `night_preview`
   and `night_curve`, `POST /projects/{id}/feasibility`, and the Visibility
   panel under the framing view with the altitude chart, custom horizon,
   Moon and darkness bands, a visible-tonight verdict and nights-to-complete
   per rig, with the rig's meridian pause taken out of the hours and drawn on
   the chart with the transit. Per-rig panel ownership is done (see "Mosaic
   projects across rigs").
6. Activation. Done for local rigs: `POST /projects/{id}/activation/preview`
   and `/apply` write the TS project, per-panel targets and per-objective
   exposure plans with the side tables above, record the activation in meta
   schema 9, and link new projects. `GET /rigs/{rig}/program` serves the
   core program from those rows and the plugin's equipment report. Remote
   rigs done: a rig profile names a registered Sync peer (`peer_id`), Apply
   pushes the rig database's planning tables to that peer with Sync's
   planning push after the local commit, `POST
   /projects/{id}/activation/push` resends the last activation, and every
   report row says whether the peer took it.
7. Check-in and live status. Backend done: meta schema 10 inbox
   (`rig_event`, `rig_feed`, `rig_status`), `POST /rigs/{rig}/checkin`,
   `POST /rigs/{rig}/status`, `GET /rigs/status`. Operator dashboard done:
   schema 12 notes every program pull, check-in and status report as a
   contact by server receipt time; `GET /rigs/status` lists every bound rig
   with connectivity (`online` within 3 min, `stale` within 30, `offline`,
   `never`), the newest report flagged stale past 10 min, contact ages,
   assignments and pending receipts; the Live table now sits under the Sky's
   coverage map, where each rig is also drawn where it points. The plugin's
   sender for status and check-in is the other side.

The Planning page changed with these (it has since been folded into the
Library, the header and the Sky; see "One list, two groups, Live on the
Sky" below): the identity lists gave way to a plan
list across databases (`GET /plans`) over a rig list with live status. Listing
adopts automatically: every registered database becomes a rig and every
project row with a GUID becomes a plan, same-GUID rows across databases one
plan (Sync keeps GUIDs), same-name rows separate plans; the reviewed adoption
endpoints stay for tools. A plan row opens the project workspace (its
databases with their target and exposure editors, framing, plan with rig
checkboxes, activation); a project with no database yet starts its framing
from a resolved name or typed coordinates (`GET /sky/resolve`). Rig setup
(optics, site, limits) expands inline from the rig list; the settings panel
no longer carries planning links; sites are edited inside the rig profile.

### Site and rig responsibilities

Sites are shared planning/acquisition context, not project ownership, catalog
identity, or an acquisition executor. They supply latitude, longitude and
elevation for visibility, darkness and transit calculations; a time zone for
local-night boundaries and display; and weather sources/conditions relevant to
the location. Keep event timestamps and scheduling instants in UTC. Weather
forecasts help planning, while observed weather carries its source, observation
time and expiry for acquisition decisions. A site does not grant permission to
run equipment or turn a forecast into a safety guarantee.

Rigs at one site may have different horizons, altitude limits, meridian limits,
equipment capabilities and local safety devices. Those belong to the rig's
versioned setup, not the site. N.I.N.A.'s local unsafe state and operator override
always take precedence over favorable site weather or server plans. Missing or
stale required weather/safety evidence blocks new acquisition; offline behavior
must use explicitly configured local sources and freshness rules, not the last
server report indefinitely. Multiple rigs can reference one site revision
without sharing a horizon or rewriting historical configurations.

The current schema-2 meta prototype stores the horizon inside `SiteSnapshot`.
That does not yet implement this ownership boundary. Move it into `RigSetup`
with a versioned migration that preserves every referenced effective horizon;
do not silently replace it with a common site curve. Time-zone and weather
configuration, freshness policy and their planning/acquisition integration are
also still required.

### Inherited planning policy

Smart filter selection, soft avoidance rules and other scheduling preferences
start as global defaults. Sites, rigs and projects may override only the
settings they need. Project precedence is the ordered global list described
below, with site/rig replacement lists, not project-specific weights. Projects
must not require a duplicate TS-style settings form to get normal scheduling behavior.
Resolve each field in this order: global default -> site override -> rig
override -> project override. A multi-rig project therefore has an effective
policy for each participating rig/site configuration, not one flattened policy
that accidentally erases those differences.

An unset override means inherit. Explicit false/off, zero where valid, and an
explicit strategy selection remain actual overrides. Use typed per-field
resolution, not sentinel numbers or wholesale replacement of a partially filled
settings object. Keep the original overrides and their provenance; do not copy
inherited values into every project when saving. The UI should show the
effective value and its source, with an explicit override control and reset to
inherit action. Changing a parent default then reaches its inheriting children.

The shared Rust core resolves and validates this policy for both server
simulation and local Director execution. A resolved snapshot includes the
policy/schema version, contributing scope IDs/revisions and per-field source.
Cache keys and check-in data retain that snapshot so online simulation and
offline acquisition use the same rules. A policy update takes effect at an
authorized safe boundary and triggers reevaluation; it does not rewrite an
in-flight operation or implicitly expand a cached assignment.

Separate preferences from hard constraints. Soft filter/Moon avoidance,
observing preferences and relative priority may inherit and be overridden.
Local unsafe state, equipment limits, required fresh evidence, rig horizon and
hard meridian/altitude limits still intersect the resulting policy and cannot
be relaxed by a project override. Hard-limit commissioning remains explicit and
local. Site-level planning overrides do not make sites own rig hardware or
horizons. The policy resolver selects behavior; N.I.N.A. remains the equipment
and safety execution boundary.

The project planning workspace exposes a global project order and optional
site/rig replacement orders. A rig explicitly selects an existing planning
site for inheritance. This association does not replace its native location,
horizon or safety limits. Existing programs and installations without a saved
order retain their previous scheduling policy.

#### User-controlled observing priority

Rank projects, rather than requiring operators to tune scores. The global
ordered list is the default for every rig. An optional site order replaces it;
an optional rig order replaces the site/global order. There is no project-scope
order or project weights editor. Null inherits; an explicit list replaces its
parent. Projects not yet in the saved list follow it in deterministic name/ID
order. Project merges replace a retired identity at its position, or remove it
if the survivor already has a position, and increment the settings revision.

Meta schema 20 adds optional `project_order` UUID lists to the existing
revision-checked settings APIs. Orders are limited to 256 distinct existing
projects; unknown/nil IDs and project-scope lists are rejected. Effective reads
include `project_order` and `order_source`. No new NINA wire contract is needed:
program assembly compiles project precedence and within-project objective
priority into existing shared-core `Goal.priority` values. Every objective in
a higher-ranked project outranks every objective in a lower-ranked project.
New ranked programs omit `observing_preferences`; the normal local selector
still applies hard eligibility, Moon rules, safety and completed-work checks.

An order save changes the program fingerprint and applies to newly issued
programs. It does not rewrite active grants or interrupt an exposure. Cached
allocations execute the frozen order offline. Live automatic Director sessions
can detect changed ranked priorities for the same goal set at check-in, then
finish the current preparation/exposure, run cleanup hooks, park and reconcile
the complete ledger before releasing it and requesting a successor. This is
an early clean release, not an in-place grant mutation. Manual/deferred sessions
keep the held order. Broader goal/grade/configuration replacement remains backlog.
Legacy weighted programs remain readable and executable, and saved weights
remain in storage for compatibility, but are not used for new programs once
a global/site/rig order resolves. The planner no longer exposes importance,
presets, weights, dwell or switching-margin fields.

The priority handoff reuses `GET /rigs/{rig}/program`, capture check-in and the
existing workload release/request APIs. No new wire or runtime contract is
needed. `program_changed` includes progress revisions and is not itself a reason
to yield; the client compares priority intent for matching goals/configuration.
An unready preview retains the current grant. A fully delivered, quiescent
ledger must be sealed before the next request; pending capture credit and
spent attempts carry forward and the old allocation remains unlaunchable.
If release loses connectivity, the rig stays parked and batch check-in finishes
that historical release. It cannot resume or replay old work. Continuous
tracking through a handoff and cross-allocation continuity remain future work.

Native priority-handoff validation (2026-10-03): NINA 3.3.0.1064, ASCOM OmniSim,
the packaged Director plugin and runtime 0.10.0 / IPC 10 ran against isolated
local PSF Guard. Global/site/rig replacement and inheritance executed offline.
A live order change during exposure produced A, then B, then the remaining A
capture across two sealed ledgers, preserving pending credit and spent attempts.
The plugin's `docs/nina-smoke-test.md` records commands and retained evidence.
The meta allocation regression also covers partial release, duplicate receipts,
reopen and refusal to relaunch the old allocation.

2026-10-03 validation: all five Director crate suites, 79 server Director tests,
704 frontend tests, Rust Clippy and frontend lint/build passed. The real-server
Chromium test saved and reloaded global order, overrode and reset a rig order,
and checked desktop/mobile layout. Shared-core selection tests verify strict
project precedence, within-project ordering, blocked/completed fallthrough and
unsafe stop. No NINA adapter or wire changes were needed; a new native NINA
hardware run was not performed for this UI/server change.

#### Legacy weighted-program compatibility

The previous preview used continuous **importance** (0-100) as a weighted preference.
Zero importance does not disable a project; activation and eligibility do that.
When explicitly importing legacy priorities, map Low/Normal/High to 25/50/75;
missing or unknown values map to 50. Do not reinterpret existing Director
priorities or rewrite active grants during migration.

The shared-core scorer implements seven independently adjustable factors.
Weights are relative (0-1000); zero disables a factor and at least one must be
nonzero. Normalize scores to 0-10000 and compute the weighted mean with integer
arithmetic. Each result exposes the inputs, weights, weighted contributions and
missing-evidence flags, so a future UI can explain why one objective won.

| Factor | Current scoring input |
|---|---|
| Importance | Explicit user value, scaled from 0-100. |
| Window urgency | Complete operation cost divided by time remaining in the current hard eligibility window; includes exposure and estimated overhead. This is local window urgency, not a seasonal deadline prediction. |
| Altitude | Sine of current observed altitude, clamped to 0-1; geometry preference, not measured image quality. |
| Moon opportunity | Declared recipe Moon sensitivity while its hard lunar window permits acquisition. It favors sensitive filters when eligible, not an invented prediction of sky brightness. |
| Completion | Accepted fraction of this objective. Pending images reserve work but are not accepted completion; this is not combined-project completion. |
| Efficiency | Exposure time divided by exposure plus estimated overhead. Timing learning must later provide the same estimates on server and rig. |
| Continuity | Prefer the current target, including another recipe for that target. |

The retained library presets use these defaults in the factor order above:

- **Balanced:** 30 / 25 / 15 / 15 / 5 / 5 / 5.
- **Finish objectives:** 25 / 15 / 10 / 10 / 25 / 5 / 10.
- **Best conditions:** 20 / 15 / 30 / 25 / 0 / 5 / 5.

An importance-only weighting remains available for people who prefer strict
priority. Explicit zero dwell/margin permits immediate switching at a safe
boundary. Default dwell is ten minutes and default switching margin is five
score points out of 100. Retain the active goal during dwell or unless a
challenger improves by more than the margin, but only while the active goal
still fits and has authorized work. Completion, exhausted attempts and all hard
constraints override continuity. Stable goal IDs break otherwise equal scores.
Durations use supplied monotonic-validated state, not wall-clock reads in the
core. A new allocation must explicitly reconcile its active-goal context.

The resolver implements global -> optional site -> optional rig -> optional
project overrides per field, including zero values, and retains immutable
effective values, original overrides, source IDs and revisions. A project can
therefore have different resolved policies on different rigs without owning
their safety limits. Missing optional scoring evidence is explicitly neutral,
not perfect; the geometry-bound entry point supplies altitude and Moon inputs
itself. Reject malformed weights, hierarchy, goal bindings and timestamps.

Meta schema 19 stores revision-checked overrides. Operator-only
`PUT /preferences/{scope}/{id}` saves them; the matching GET reads them.
`GET /preferences` returns global identity, presets and sites;
`GET /rigs/{rig}/preferences?project_id=...` returns effective policy and sources.
All paths are under `/api/director/v1`. These APIs never write rig databases.

Legacy-policy program snapshots carry optional `observing_preferences` schema 1: a
deduplicated policy map and exact goal-to-policy bindings. The program fingerprint
includes policy and source revisions. Existing grants remain immutable after
settings change. Runtime 0.10.0 / IPC 10 uses these policies in geometry evaluation
and native preparation; automatic clients advertise `prepared_target_v3` or
`local_sequence_v3`. Older modes refuse weighted programs rather than executing
them with legacy priorities. The engine remains 0.3.0 for old program replay.

Ledger schema 5 stores selected-goal time with an integrity digest in the same
transaction as preparation. Reopening retains dwell; damaged or missing state
fails closed. A preparation freezes its selection for one exposure but rechecks
hard feasibility before each operation. A slow autofocus cannot make setup
oscillate between targets, and no weight can override unsafe or expired work.
Selection context currently lasts for one allocation, including process restarts.
Successor allocations start without continuity after terminal release and park.
Cross-allocation continuity, candidate explanations in the UI/status and learned
timing estimates remain backlog. A preview is never a capture reservation.

2026-10-02 execution validation: 493 Director crate tests, 77 server Director
tests, 702 frontend tests and 775 plugin tests passed. The real-server Chromium
planning test saved and reloaded overrides at desktop and mobile sizes. Native
NINA 3.3.0.1064 with ASCOM OmniSim and packaged runtime 0.10.0 reversed legacy
target order using saved importance, captured three frames across two targets
during server outage, and reconciled all six events. The automatic workload
variant completed and parked offline, then released through batch check-in.
The unsafe-monitor regression aborted, parked and did not resume automatically.
Review fixes cover corrupt/deleted dwell state, parent edits that zero a child,
project attachment cleanup, and deterministic managed policy serialization.

Later factors may include seasonal opportunity, project-level completion,
fair-share/starvation control, measured quality and multi-rig opportunity cost.
They require explicit evidence and shared-core tests; collaboration/fair-share
between different owners remains future work, not part of this increment.

2026-10-02 validation: all 485 tests across core, ledger, runtime, meta and FFI
passed, including fourteen new preference/geometry regressions. Strict Clippy,
workspace formatting and diff checks passed. Review rejected duplicate weight
keys, retained the global source even when completely overridden and verified
that pending credit is not accepted completion. No UI, server endpoint or
packaged plugin behavior changed; this is not native acquisition validation.

Short and long exposures through the same filter are separate objectives when
they serve different purposes. Recipes include duration, filter mapping,
binning, gain, offset, and readout mode where supported. Resolve capabilities
explicitly; do not silently substitute filters or unsupported settings.

Different rigs need not produce interchangeable subs. A wide-field contribution
and a high-resolution central region can serve one project but require separate
stacks. Project membership never grants stack compatibility. Completion must
measure required coverage and quality, not just add integration hours from
different instruments and assume equal depth.

### Project framing wizard

The planner must support a complete framing workflow for one global project
using one or more rigs at one or more sites. This is planned work, not a feature
of the current identity screen. The first implementation uses one PSF Guard
coordinator and its enrolled rigs; another person's projects or independent
coordinators are not required to use it.

1. Select or create a project and its targets. Find a target by name or explicit
   coordinates, or start from a reference image with verified coordinate
   metadata. Display the sky/reference layer, its source and coordinate quality;
   a catalog overlay or embedded WCS is not fresh pixel-derived pointing evidence.
2. Frame the desired result on an interactive sky view. Set the center, angular
   coverage and position angle; pan, zoom and rotate with numeric equivalents.
   Add, remove and adjust mosaic panels, rows/columns and overlap. Show the
   complete footprint and panel IDs, including uncovered regions and overlap.
   Keep canonical ICRS coordinates and explicit angle conventions at the boundary.
3. Select participating rigs and exact setup/site revisions. Overlay each rig's
   field of view and sampling using sensor geometry, pixel size, effective focal
   length, binning and orientation capability. Missing optical geometry requires
   confirmed input; never infer it from equipment names. Support fixed/manual
   camera angles as well as rotators, and identify adjustments that need an
   operator before acquisition. Planning can use cached setup snapshots, but
   must show their freshness and revalidate them before activation.
4. Build rig-specific contribution plans for each objective. A wide-field rig
   may cover the whole target while a narrow-field rig needs several panels or
   provides a high-resolution region. Choose panel coverage, bandpass, exposure
   purpose, short/long recipes and accepted-data goals without duplicating the
   global project. Validate native filter/readout mappings and keep incompatible
   sampling, coverage, exposure purposes and processing groups distinct.
5. Preview the combined plan by rig, site and local night. Use each site's
   location/time/weather context, each rig's horizon and hard limits, effective
   inherited policy, existing accepted/pending coverage, and operation-duration
   estimates. Show visibility, darkness, Moon restrictions, meridian/flip gaps,
   filter opportunities, estimated work, uncertainty and unmet objectives.
   Unsupported or stale required inputs must be visible, not treated as feasible.
6. Review the proposed objectives, panels, recipes, contributions and allocation
   changes together, then explicitly save or activate a revision. Saving a draft
   never starts equipment. Changed setup, policy or project revisions require a
   fresh review; activation separately requires current rig authorization and
   validates outstanding allocations. Allow cancellation, back navigation and
   resuming a saved draft without losing choices.
7. Return to the same project coverage view as captures are graded. Distinguish
   planned, allocated, captured-pending, accepted and rejected coverage per panel
   and contribution. Revise deficits or framing without rewriting historical
   capture provenance. Replanning can reassign compatible outstanding demand at
   safe check-in boundaries, but cannot double-allocate an offline rig's work.

Persist draft state separately from immutable validated intent. Retain stable
objective, panel and contribution identities, revision provenance, reference
sources and exact equipment/site snapshots; names and tile order are not IDs.
Keep project and wizard scope in URL state. The shared Rust core owns footprint,
panel geometry and feasibility calculations used by server preview and Director;
the browser renders results and edits intent, not a separate scheduling engine.
Use the same plan engine for preview and goal-driven execution. Estimated slots
are not a replay script: slow autofocus, weather and changing priorities trigger
bounded local reevaluation and check-in within the authorized contributions.

Multi-site coordination serves one combined project's objectives. Sites affect
when and where contributions are feasible; they neither own the project nor
turn unlike data into interchangeable credit. Cross-rig coverage and completion
must use explicit compatibility rules and capture identity, not summed hours or
overlapping rectangles alone. Calibration, quality and processing provenance
remain attached to the originating rig/setup and contribution.

### Mosaic projects across rigs

A mosaic is one project whose framing has more than one panel. Two things
follow from the rig split above and from how rigs differ in field size:

- **Panels are owned per rig.** A contribution names the panels its rig
  shoots (`panel_ids` in the plan draft; empty means every panel). A wide
  rig may cover the whole target as one panel while a long-focus rig takes
  a subset, or two similar rigs split the grid. The plan editor gives each
  participating rig its own section: which panels it takes, its template
  and exposure per objective, and its own frames-and-hours totals. Coverage
  rolls up per panel and objective across rigs; a panel nobody shoots is a
  visible gap, never silently assigned. Activation writes only the targets a
  rig owns into that rig's database.
- **Panels stack apart and are joined afterwards.** Each panel target has
  its own captures, calibration and stack previews in its rig's catalog, as
  today. A later stage registers the finished per-panel stacks by the plan's
  panel geometry into one mosaic preview for the project: the framing's
  tangent plane gives each panel's expected placement, a fresh solve of each
  stack corrects it, and overlap regions show seams and depth differences.
  Panels from different rigs keep their provenance and are never blended
  into one stack; the mosaic preview is a review aid for coverage and a
  starting point for processing, not a processed image.

Done: per-rig panel selection in the plan editor (a panel chooser per rig,
every panel by default), activation writing only the targets and plans a rig
owns and naming uncovered panels, and per-rig totals scaled by panels. The
mosaic preview is done as a framing-view layer: `GET /projects/{id}/mosaic`
finds each activated panel's latest stack preview in its rig's cache through
the Sky page's placement code (reference-frame solve composed with the
stack's orientation), and the browser lays solved stacks on the view plane by
their TAN solutions, next to the plan's rectangles, with per-panel accepted
and desired frame counts. Unsolved stacks are listed, not drawn. The plan
list carries per-target accepted and desired frames from every linked
database (`targets` on each link, `progress` on the row), which for a mosaic
is per-panel progress.

### Survey backgrounds for framing

The framing wizard requires an image-backed sky map, not only catalog markers
or camera rectangles on an empty field. Start with the normal/broadband and
narrowband survey choices available in N.I.N.A., and allow additional providers.
Use the underlying survey services directly so PSF Guard's browser planner does
not require a running N.I.N.A. instance or a copy of its internal database.

The inspected N.I.N.A. source includes DSS2 Color (`CDS/P/DSS2/color`) and the
Finkbeiner H-alpha composite (`CDS/P/Finkbeiner`) in its HiPS choices. These are
initial presets, not the complete provider list. Keep broadband color, individual
bands and emission-line maps distinguishable. H-alpha is not interchangeable
with O III or S II; offer other narrowband layers only where a real survey exists
and clearly expose its coverage and resolution limits.

Use a maintained astronomical map renderer with HiPS support, evaluating Aladin
Lite for the browser, rather than implementing a new tile projection engine.
HiPS tiles support interactive pan/zoom; HiPS2FITS-style cutouts can support
fixed reference views and cached exports. A provider adapter owns fetching and
display metadata, outside the pure Rust planning core. Core-generated rig and
panel footprints remain authoritative; renderer coordinate transforms must keep
them registered to the imagery across projections, RA wrap and the poles.

The workflow must:

- Offer named survey/band choices and comparison by opacity or blinking without
  changing the saved target, position angle, panel IDs or any rig's footprint.
- Show attribution, survey identifier, bandpass, resolution and available
  coverage. Record survey identity/version and display choices with the draft,
  separately from acquisition recipes; changing a background cannot change a
  capture filter or create a new allocation.
- Preserve center, zoom, rotation and overlays while a new layer loads. Cancel
  obsolete requests and prevent late responses from replacing the selected map.
  Distinguish loading, missing coverage, service failure and cached/offline data;
  do not silently substitute a broadband layer for unavailable narrowband data.
- Cache bounded tiles/cutouts using provider, survey/version, coordinate frame,
  projection, resolution and relevant display parameters. Honor provider usage,
  attribution and redistribution terms before enabling persistent caches or
  redistributing images; N.I.N.A.'s code license is not an image-data license.
- Let the operator prepare an offline cache for the project's framing region.
  Offline editing can use cached surveys or imported reference images, with
  unavailable regions marked. Missing map pixels must never block an already
  authorized rig's offline acquisition or become a scheduling dependency.
- Support local reference images with known WCS or a verified solve alongside
  surveys. Label embedded coordinates versus fresh pixel-derived solves.
  Background pixels are composition aids, not evidence of current pointing,
  transparency, grade, accepted exposure depth or project completion.

Remote provider URLs and imported metadata are untrusted. A server-side image
fetcher must use approved endpoints, bounded transfers and timeouts, and enforce
the same destination restrictions across redirects; it must not become an
arbitrary URL proxy. Browser providers need verified CORS/Tauri compatibility.

References: N.I.N.A.'s [framing documentation](https://nighttime-imaging.eu/docs/master/site/tabs/framing/),
its inspected [survey presets](https://github.com/isbeorn/nina/blob/fdf546fc2bea0de1eeff36f227ffaf0dd408ab70/NINA/Database/Migration/16.sql)
and [HiPS2FITS adapter](https://github.com/isbeorn/nina/blob/fdf546fc2bea0de1eeff36f227ffaf0dd408ab70/NINA.WPF.Base/SkySurvey/Hips2FitsSurvey.cs),
and the CDS [HiPS survey registry](https://aladin.cds.unistra.fr/hips/list).
Verify provider availability and terms again when implementing; these references
do not promise service uptime or uniform all-sky coverage.

Implemented 2026-09-27: the server fetches HiPS2FITS tangent-plane cutouts for
N.I.N.A.'s survey list through `GET /api/director/v1/sky/cutout`, off the
request path with a `202` poll contract, and caches them under the cache root.
Aladin Lite was not adopted; the browser draws footprints over the cutout with
PSF Guard's own projection code. The view follows N.I.N.A.'s framing assistant
in its handling: the rectangle is dragged to move the target, its handle turns
it, quarter turns and the rig's fixed camera angle are one click, the sky pans
separately, and a readout gives coordinates, angle and extent. N.I.N.A.'s
framing cache (`%LOCALAPPDATA%\NINA\FramingAssistantCache`, `CacheInfo.xml`
with `RA`, `Dec`, `FoVW`, `FoVH`, `Rotation`, `Source`, `FileName`) is only
readable on the rig machine; the plugin can offer its entries through a
`POST /rigs/{rig}/framing-cache` upload so an offline site's imagery appears
here without a second download. That endpoint is planned, not built. Provider terms: CDS asks for attribution,
which the survey list carries; persistent caching for the operator's own
framing is within ordinary use.

## Rig constraints and local horizons

Meridian restrictions and the effective local horizon are required inputs to
the shared engine, not optional server scheduling hints. Director exports a
versioned constraint snapshot at check-in; server simulation uses that snapshot,
and local execution always checks the current configuration. A remote project
may tighten local limits, but cannot relax a rig's hard restrictions.

The NINA executor also monitors the admitted native constraints while an
operation or sequence hook runs. A read-only check once per second compares
site and meridian settings, the loaded horizon, and the horizon file's content
hash. A same-path edit cannot hide behind an unchanged size or timestamp.
Missing or stalled evidence stops acquisition; a two-second read deadline is
independent of the safety watchdog. This monitor supplements fresh shared-core
feasibility checks at dispatch, rather than replacing them.

Detected changes cancel work and use the configured enclosure-aware abort
policy. They do not modify the active grant, refund attempts or auto-resume when
old settings return. The operator must review/report the current configuration
and reconcile interrupted work before obtaining new authority. This needs no
new API or IPC message: existing immutable configuration fingerprints, one-shot
launches and operation outcomes retain the boundary. Automatic context renewal
and commissioning remain separate phase-2 work.

2026-10-03 native validation: NINA 3.3.0.1064 with ASCOM OmniSim and isolated
PSF Guard stopped a 45-second setup hook after offline horizon, site and
meridian edits, saved no exposure, parked and stayed stopped after restoration.
The live ranked-priority handoff still completed across two sealed ledgers.
The plugin's `docs/nina-smoke-test.md` records commands and retained evidence.

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
the core does not read rig files. IPC and internal post-hook dispatch bindings
are merged. Complete preference resolution, production time/orientation inputs,
and the production session owner remain unverified. The public editor and
server-program simulator path are tested without production arming.

## Storage and authority

| Store | Owns |
| --- | --- |
| Meta database | Global projects and objectives, sites, rig capabilities, mappings, contribution plans, assignments, permissions, and progress projections. |
| Per-rig catalog databases | PSF Guard-owned acquisition history, image records, grades, calibration records and local evidence; TS compatibility is provided at the import/sync boundary. |
| Director local state | Cached assignments, execution journal, unsent events, recovery state, and operation timing observations. |

Start with a separate coordination SQLite database, provisionally named
`psf-guard-meta.sqlite`, alongside existing registered catalogs. This is a new
domain, not a repurposing of the catalog registry or the current merged Library.
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

### Native catalogs and Target Scheduler exchange

Decision, 2026-09-26: neither Director nor future PSF Guard catalogs must use the
TS schema internally. The meta/per-rig split remains: global coordination in the
meta store, local acquisition and image evidence in PSF Guard-owned per-rig
catalogs, and offline execution evidence in Director's local journal. This is
the intended architecture, not the current catalog implementation. Existing
catalog code still reads/writes TS-shaped tables; no migration has shipped.

The future Settings/UI workflow is **Import Target Scheduler database** into a
native catalog, then optionally retain a named TS sync connection. Import alone
does not grant ongoing writeback. That connection identifies source provenance,
destination catalog, supported schema version, selected data classes and
direction, conflict policy, last successful sync and pending differences.
Preview before Apply for operator transfers. A one-time import, recurring sync,
and opening a legacy catalog are distinct operations, not aliases.

Build explicit adapters for transferable projects, targets, recipes/templates,
exposure plans, capture metadata, grades/reject reasons, and supported flat
history/coverage. Maintain a versioned capability/mapping matrix in both
directions. Probe optional fields and schema variants. Unrepresentable native
objectives, cross-rig allocations, execution state or processing provenance stay
native; the preview reports omissions and conflicts rather than silently
flattening them or claiming a lossless round trip. Never export Director hardware
authority or credentials into TS. Importing a TS project does not activate it.

Preserve external GUIDs with an explicit origin and native-ID mapping. Repeated
imports/syncs are idempotent; a mirrored capture remains one capture. Names,
paths, nearby coordinates and TS-local integers are not cross-system identity.
Convert RA hours at the adapter boundary. Grades, reject reasons, calibration
coverage invalidation and TS progress/summary updates retain their current
documented semantics for fields that are transferred. Do not put calibration
frames into TS light-frame rows or replace a whole running TS database to apply
a narrow update. Use consistent source snapshots and short writes; no database
transaction waits on HTTP or image transfer.

Migrate incrementally behind catalog access interfaces, with versioned owned
schemas, explicit database type detection, backups and a rollback path. Never
convert an external TS source file in place. Keep original files and provenance,
prove native/legacy query and grading parity on copied real catalogs, and
preserve existing registered-catalog URLs or provide explicit remapping.
New native catalogs must not depend on TS installation or TS-shaped tables.
Existing directly opened TS catalogs remain supported during transition; their
eventual migration/deprecation needs a separate reviewed release decision.

PSF Guard Sync remains a supported external integration. Its current published
API and transfer-bundle contracts must keep working through adapters or a
negotiated upgrade. Schema independence is not permission to break installed
Sync plugins or drop data that previously transferred. The phase-1 gate below
tracks this migration independently from Director's runtime preview.

## Shared planning core

Implementation direction: a standalone Rust planning crate used directly by
PSF Guard and by a bundled Director sidecar. The C# N.I.N.A. plugin communicates
with that sidecar over versioned local IPC. The process boundary is implemented
with a published runtime-only plugin and tested internal native adapters. The
production acquisition container and server integration remain pending.
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
because its IPC request was retried. The protocol and local ledger prove bounded
process recovery, not coordinator reconciliation or permission to operate
equipment. A recovered command is evidence, never a fresh dispatch grant.

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
Operator-approved projects and commissioned rig policy
  -> Director submits workload request / reconciled check-in
  -> coordinator uses shared planning priorities to allocate bounded goals
  -> Director validates, caches, and acknowledges an assignment
  -> shared engine selects useful local work
  -> Director's adapter executes through supported N.I.N.A. APIs
  -> actual results update local state and duration estimates
  -> engine reevaluates at execution boundaries
  -> check-in reconciles progress and revises allocation
```

Workload requests are scoped to the commissioned coordinator/catalog/rig/
profile/client and use stable request identities with idempotent responses.
They carry versioned configuration/policy evidence and receipt cursors, not
arbitrary instructions or a client claim to new capture credit. The coordinator
reserves work atomically before returning a grant; a dropped reply must not
reserve the same deficit again. Repeated check-ins cannot refresh spent budgets.
An unchanged current grant may be returned; a successor requires reconciliation
of the old grant and an explicit transition acknowledged at a safe boundary.
The v1 commissioned request/clean-release exchange is implemented (see
[Director management](../DIRECTOR.md#automatic-workload-policy-and-exchange)).
Receipt cursors travel through the separate capture checkpoint endpoint.
General successor recovery, active-grant revisions and duration/quality
reinforcement still need versioned contracts; capture receipts alone do not
establish those capabilities.
Requesting work must not let a paired client admit its own grants or bypass
operator-approved project scope. The coordinator issues grants under the saved
commissioning policy; local enablement, safety and exclusive ownership still
govern whether Director may act on them.

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

The TS target container **and its UI** are the compatibility baseline, not just
inspiration for a smaller set of callbacks. Preserve familiar slot names,
ordering, enablement, container options, nested instructions, and the native
trigger/condition editing experience wherever the behavior applies. The seven
instruction slots are **Before Wait**, **After Wait**, **Before New Target**,
**After Each Exposure**, **After New Target**, **After Each Target**, and
**After Target Complete**. These are distinct from ordinary N.I.N.A. Triggers
and Conditions; supporting one does not establish compatibility with the other.
Document any intentional difference next to the compatibility matrix before
changing the public UI. Director defaults extend this surface with explicit
policy ownership, not duplicate automatic actions or a separate hook scheduler.

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

### Advanced Sequencer hooks and native defaults

The production Director container must participate in the full supported N.I.N.A.
Advanced Sequencer item, trigger and condition lifecycle, not just a private
before/after-exposure callback. Inventory the pinned N.I.N.A. interfaces and TS
behavior, then publish a compatibility matrix with supported, tested and
unsupported states for each operation/context. Include third-party safety
actions such as "When Becomes Unsafe" where supplied by plugins; do not assume
their names, semantics or presence without checking the installed extension.

Required attachment boundaries are session start/end, target entry/exit,
before/after native operations and exposures, waits, condition changes, unsafe
transition/recovery, cancellation and failure cleanup. Map these to native
lifecycles; do not invent a second trigger scheduler. Nested containers retain
target/profile context, trigger cadence, condition evaluation, cancellation,
failure propagation and native validation/serialization behavior. Hooks that
consume time or change equipment must report their actual outcome and duration,
invalidate stale pointing/capability evidence, and cause a fresh core decision
before the next acquisition operation. Preserve one-shot dispatch authority.

Ordinary native and compatible plugin actions should work in these slots. An
action that itself acquires science frames must use an explicit Director
reservation/progress adapter, or be refused with a visible reason. It must not
bypass budgets because it is nested inside a hook. Unsupported or missing
plugins produce a validation error, not silently skipped actions. Local safety
and operator stop preempt the current operation even if the sidecar/server is
unavailable; canceling the main sequence must not cancel required safe cleanup.
Specify how safety transitions interrupt hooks, nested waits and cleanup, and
test repeated unsafe/safe transitions without duplicate park/shutdown work.

Director must also offer a useful default session with no TS, Chatstronomy or
optional acquisition/safety plugin installed. Use N.I.N.A.'s native equipment
and sequencing services for connection/cooling, unpark, slew/center, filter and
readout setup, autofocus, guiding, dither/settle, flip handling, capture/save,
safe wait/resume, park and configured shutdown. The Rust core owns when these
operations are needed and evaluates their measured costs; C# executes native
items and returns observations. Capability-dependent steps are explicit: a
fixed-filter rig needs no wheel, for example, while a required disconnected
guider or missing configured safety monitor is not silently ignored.

Resolve each policy as Director default, explicit user configuration, or a
named native/plugin hook. Show the effective owner and prevent double execution
(two autofocus, dither or safety policies for one boundary). Defaults use actual
profile/capability settings and conservative timing estimates, not universal
hidden constants or a fabricated safe state. With no safety source configured,
require an explicit local operating policy during commissioning; loss of a
previously required source blocks new acquisition. Remote plans can tighten
limits but cannot override native safety, flip handling or operator settings.

This is a phase-2 requirement. Plugin
[#31](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/31)
exports a **Director Session
configuration preview** with the seven named instruction editors, native
triggers/conditions, grouped local-policy fields and display-only session status.
The later prepared-target increment connects this editor to a public executor;
automatic scheduling remains gated. Exporting the editor alone was not admission.

Session options schema 1 records duration and hook/save deadlines; required
monitor versus attended policy; native horizon versus fixed minimum; altitude
and meridian bounds; park-on-wait; Director versus sequence ownership for
startup, slew/center, autofocus, guiding, dither, flip and shutdown; and offline,
batch check-in and live-status preferences. These are requested policy, not
equipment authority. Apart from the tested hook deadline, the production owner
must still connect these fields. Attended policy requires fresh local consent;
never deserialize it as permission to invent a Safe state.

The internal hook owner snapshots settings, tracks target visits and confirmed
capture IDs, records monotonic hook durations, and stops after failed/canceled
boundaries. Waiting for grade assessment does not run target-complete hooks.
Native exposure instructions inside hooks/trigger runners require an explicit
reserved-capture adapter and are refused without one. Third-party instructions
that hide hardware calls still need compatibility testing.

The prepared-target owner now binds pairing, allocation launch, native safety,
orientation, ownership and capture check-in/status. Remaining interface work
before full automatic operation:

- Extend the existing pairing/safety/orientation owner to automatic operations
  without permissive fallbacks; define conditions validity for long operations.
- Reconcile retained pending and unresolved attempts across restart/replacement.
  A program preview is never an execution allocation.
- Implement automatic ownership policies and capabilities beyond the current
  explicit prepared-target policy; reject unsupported or duplicate ownership.
- Feed measured hook/preparation outcomes through a separately identified
  preparation receipt feed. Capture `/checkin` is not a substitute.
- Add complete per-operation observations, duration learning and grade feedback
  without making server availability a safety dependency.

Director plugin [#26](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/26)
adds internal seven-slot configuration and one-use native invocations. Cloning,
JSON persistence, nested target context, inherited triggers/conditions,
cancellation and failed/skipped child reporting have native tests. See the
[TS compatibility matrix](https://github.com/theatrus/psf-guard-director-nina-plugin/blob/main/docs/native-capture.md#target-scheduler-compatible-instruction-slots).
The new configuration preview exposes those slots; the production coordinator still
owns slot cadence, policy ownership, exposure-in-hook admission and safe cleanup.
In particular, TS's internal `AfterAllTargetsContainer` is labeled **After Each
Target Instructions**; its name must not be interpreted as a session-end hook.

Director has a distinct plugin identity, configuration, credentials, queues,
and release flow from Sync. Detect competing acquisition controllers. Define
ownership for shared sync/upload duties so coexistence does not create duplicate
uploads or competing planning writes. Reject concurrent controller ownership
rather than letting Director and a TS sequence operate the same equipment.
Optional TS import/catalog compatibility remains a public contract,
including stable GUIDs, schema variation, grade values, and RA unit conversion.

### Filter-specific Moon avoidance in exposure settings

Required phase-2 work. Moon avoidance belongs in the existing exposure
template/recipe settings and must participate in local smart-filter selection.
The first implementation carries explicit template policies into immutable
recipes and enforces conservative lunar windows in the shared geometry engine.
It includes TS import/activation, template editing, executor capability gates
and a lunar-only wait reason. See [current behavior](../DIRECTOR.md#exposure-moon-rules).
This does not complete the inheritance, configurable preference weights or
detailed diagnostics below. A Moon chart alone is not acquisition enforcement.

Add a **Moon avoidance** group alongside filter, exposure and gain. Support
inherit, explicit off, and a custom policy, displaying the effective values and
their source. The custom policy includes minimum target-Moon separation,
Moon-down-only eligibility and the supported phase/altitude relaxation controls.
Separate hard eligibility limits from a configurable soft preference for better
lunar conditions. Show degrees and meaningful units; validate ranges and
combinations. Preserve unset, explicit off and valid zero as distinct values.
Use global/site/rig/project planning defaults, then the selected exposure
template's explicit override and any explicit contribution recipe override.
Persist references and overrides, not copies of every inherited value. A rig's
commissioned hard limits remain authoritative.

Templates for different filters can have different tolerances. Broadband,
OIII and Ha must not receive one flattened project-wide threshold, nor should
Director assume all narrowband filters tolerate the same conditions. Named
presets may seed editable values, but filter-name guessing cannot silently
replace a saved policy. Distinct short/long recipes may also override their
requirements while retaining exact filter/configuration identity.

The shared core first excludes recipes violating their effective hard lunar
limits, then scores eligible work using the resolved priority and smart-filter
policy, including its explicit lunar preference weight. High target priority
cannot bypass a Moon-down or separation restriction. If a preferred filter is
blocked, consider eligible filters on that target and other targets. When all
authorized work is blocked, wait until the next feasible window and explain
why; do not repeatedly slew, change filters or count this as equipment failure.
Avoid unnecessary switching with the shared switching-cost policy, without
overriding eligibility. The UI and telemetry must distinguish **Moon blocked**
from a lower-priority eligible recipe and show separation, required limit,
Moon altitude/illumination and the next candidate window.

Resolve the policy into a versioned immutable program snapshot for both server
simulation and the local sidecar. Compute conditions from the site's location,
time and the shared lunar ephemeris, including the declared horizon convention
for Moon-down and sufficient accuracy/margin near thresholds. Reevaluate after
slow setup and before capture over the required operation/exposure interval;
an old night-preview sample cannot authorize dispatch. Offline operation uses
the same pinned policy and ephemeris. Missing required policy/context blocks
that recipe with a reason. Executors that cannot enforce the policy must
reject the program capability rather than silently ignoring it.

Preserve TS template fields through import, activation and supported export:
`moonavoidanceenabled`, `moonavoidanceseparation`, `moonavoidancewidth`,
`moonrelaxscale`, `moonrelaxminaltitude`, `moonrelaxmaxaltitude` and
`moondownenabled`. Verify their formula, units and precedence against the pinned
TS implementation before mapping them. Optional legacy columns use documented
compatibility defaults; an unsupported nondefault rule needs a visible review
result instead of being discarded. Existing operator-edited Director templates
must not be overwritten by re-import.

Deliver template storage/API/editor and TS mapping together with shared-core
evaluation, program capability negotiation and native execution integration.
Keep the feature marked pending until that complete path is tested. Cover
mixed-filter priorities, independent Moon-down/separation settings, relaxation
boundaries, inheritance/off/zero, Moon rise during slow setup, all-work-blocked
waits and disconnected execution. Prove identical server/sidecar selection and
use deterministic simulated time to verify real NINA switches to an eligible
filter without capturing a blocked recipe.

### Quality holds, equipment failures and session stop

Partially implemented phase-2 work: the Rust policy and separate durable recovery
store are exposed through opt-in IPC 9 and the managed plugin client, but are not
wired into native N.I.N.A. acquisition.
They do not yet protect real acquisition or enable automatic recovery. The current
native safety owner cancels acquisition, parks only with independent enclosure
clearance, and stays stopped after Safe/Open returns. Extend it with this session
policy using local evidence while disconnected from PSF Guard.

#### Implemented recovery foundation

`director_core::recovery` supplies a pure, versioned session state machine;
`director_ledger::recovery::SessionStore` persists it in an owned SQLite database
outside allocation-specific run directories. IPC 9 exposes these internal APIs
as described below; there are no new server endpoints. The existing capture
ledger schema, allocation authority, public plugin and Sync behavior are unchanged.

- An explicit rig/configuration/night identity and frozen policy bind each
  session. States are acquiring, holding, recovering, stopping and stopped.
  The current implementation conservatively holds the whole session; target-only
  bypass and an evidence classifier remain future work.
- Typed, preclassified observations carry capture/context/reference identity and
  timestamps. Only consecutive compatible corroborated-poor samples can pause;
  recovery needs independent confirmed-good probes against the same reference.
  Unknown is neither poor nor good. The caller must classify actual evidence;
  neither a rejected grade nor low star count alone is a corroborated verdict.
- Cooldown, maximum cumulative hold time, cumulative probe count, operation
  deadlines and latest resume time bound recovery. Verified recovery resets the
  matching operation/device consecutive-failure counter, not nightly totals.
  Changing targets or workloads cannot reset these budgets. Slew, capture and
  unknown-completion failures stop rather than retry.
- Unsafe/unknown safety and prohibited enclosure motion preempt recovery.
  Motion clearance is independent of generic safety. A configured park gets
  at most one persisted attempt and a deadline; blocked, failed and uncertain
  shutdowns remain distinct from a verified parked result. No timer, new work,
  reconnect or Safe event clears a stop latch.
- Immediate SQLite transactions use revision compare-and-swap, bounded event
  records and durable evidence/attempt identities. Exact request retries return
  current state with `newly_applied: false`; new request IDs cannot count the
  same capture/failure twice or reissue a native attempt. Snapshot integrity,
  schema, scope and structural validation fail closed.
- Reopening the same night restores its immutable policy and latch, even after
  its scheduled end. A different night requires the previous one to be stopped
  and nonoverlapping. `begin_night` is an explicit admission primitive, never an
  automatic response to a changed allocation or date. Bounded `events` pages
  retain evidence for later batch delivery; IPC provides local pages, but no
  central delivery/acknowledgement API exists for this stream yet.

Host integration must use one fixed per-rig store, load it before any equipment
operation and stop dispatch on storage, clock or validation errors. Persist
before acting. `newly_applied: true` means committed input, **not** a dispatch
permit: safety/deadline preemption may have selected a different transition.
Readback, duplicate receipts and surviving in-flight attempts never issue work.
IPC now exposes exact newly issued operation identity; the native integration
must fence it through the existing owner and capture/preparation ledger. Before
recovery or park, cancel/reconcile active work and recheck fresh enclosure,
safety, geometry, allocation and local ownership. Unknown motion prevents
recovery/parking; `Acquiring` alone never grants ordinary equipment permission.
No policy defaults or Session editor controls are commissioned by this increment.

Deterministic core/store tests cover hysteresis, changing context/reference,
unknown/stale evidence, bounded failures, cumulative budgets, clock reversal,
restart during recovery/park, duplicate input, competing writers, wrong scope,
corrupt state, journal paging and nonoverlapping new-night admission.

#### Recovery IPC and native handoff

Runtime **0.9.0 / IPC 9** adds recovery contract **1**. The planning engine stays
0.3.0 / contract 2. Old IPC/runtime handshakes fail before opening a database.
Plugin [#40](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/40)
adopts the verified 0.9.0 artifact and IPC 9 handshake with a typed recovery
client. Do not replace the executable alone. The public N.I.N.A. Session still
launches with recovery disabled; native integration remains a separate gate.

The verified launcher may pass `--recovery-directory <absolute-per-rig-directory>`
after `--state-directory <absolute-allocation-directory>`. Both must already exist
and be different directories. Recovery owns `director-recovery.lock` and
`recovery.sqlite`; the per-rig lease excludes another allocation process and is
held through blocking SQLite work. Paths never arrive through IPC. `ready`
reports explicit `recovery_enabled` and `recovery_version`. Recovery remains
opt-in so existing preview/test flows do not silently adopt a policy. The native
integration must always supply the same persistent per-rig directory once
commissioned, fail if it is missing, and prohibit a launch-mode downgrade.

The new command is `{"type":"recovery","operation":{"recovery_version":1,
"operation":{...}}}`. Replies use `type: recovery` and a typed `response`:

| Operation | Reply and boundary |
|---|---|
| `open`: identity, policy, now_ms | `opened`: created, record. Explicit night admission; identical reopen restores state. Never issues equipment work. |
| `current` | `current`: required nullable record. Readback only, including surviving in-flight attempts. |
| `apply`: durable request | `applied`: newly_applied, record, required nullable issued. The request includes night/configuration/event identity, expected revision, time, current safety/motion conditions and a typed event. |
| `events`: night_id, after, limit | `events`: ordered evidence and next_cursor. Limit 1-16; return a complete prefix fitting one frame, never skip a large record. No remote acknowledgement or dispatch authority. |

`issued` is a `probe` or `park` with exact attempt ID and deadline. It is present
only for the newly committed corresponding transition, never for replay,
readback, timeout or safety-preempted input. Even this receipt is not permission
to move hardware: the native owner must validate the full record identity and
policy revision, reconcile active work, and enforce current safety/enclosure,
allocation and geometry checks. Lost issuance replies require reconciliation,
not resending an action from persisted state. Errors have typed scope, conflict,
phase, clock, evidence and storage codes; malformed protocol closes the pipe.

When recovery is enabled, the sidecar gates every ledger evaluation, preparation
advance, reservation and dispatch check. An admitted acquiring state and fresh
recorded Safe/motion-permitted conditions are required. It rejects mismatched
configurations, stale/backwards time and hold/stop states, and narrows the
planner's validity to the observing-night end so the complete operation must
fit. Opening/readback/completion stay available to settle interrupted work.
Stateless evaluation is disabled in this mode. Recovery probes do not bypass
this science gate; their separate native preparation/authority path is pending.
The native owner must submit current safety/enclosure observations before
acquisition checks and on changes during active work, not only update the
ordinary ledger state. Its independent watchdog remains responsible for prompt
cancellation when the sidecar is stalled, disconnected or unavailable.

Portable tests cover version/scope refusal, replay, gate freshness, journal frame
bounds and successor-allocation blocking. The Windows process harness tests real
pipes, exclusive recovery ownership, process death after probe/park issuance and
restart without redispatch. These are not N.I.N.A. equipment tests. Browser,
server-loop and real N.I.N.A. recovery simulator gates remain unimplemented.

The managed client requires explicit open/readback before event submission and
checks immutable identity/policy, monotonic revisions/time, exact wire fields
and enums, bounded contiguous pages, and one-shot attempt/deadline correlation.
Interrupted submission faults the connection; callers must reconnect and read
evidence, never reconstruct a permit from an in-flight snapshot. Real-sidecar
tests cover quality hold/probe completion and persisted stop/park replay after
restart. All 712 plugin tests passed locally on 2026-10-02.

Existing acquisition was also retested with packaged runtime 0.9.0, native NINA
3.3.0.1064, ASCOM OmniSim and an isolated PSF Guard server: offline multi-target
Moon avoidance (three saved frames, six batch receipts, replay refusal) and a
separate unsafe abort/park/no-restart run passed. These tests kept recovery
disabled and do not prove native recovery dispatch. Exact evidence lives in the
plugin's `docs/nina-smoke-test.md`.

Plugin [#41](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/41)
supplies independent local motion
clearance for ordinary acquisition and shutdown. The Session requires an explicit
operator choice: open air (no configured or connected dome), or a fully-open
NINA enclosure. Old sequences default to unconfigured and cannot acquire until
reviewed. A cached registration callback is not fresh evidence. Closed/closing,
opening, stale, unknown/error, disconnected or changed-device evidence cancels
the owner; reopening cannot revive it. Without clearance, cleanup requests
slew-stop and tracking-off instead of park. Stop failures propagate and no
replacement mount/profile receives cleanup commands. This is sampled evidence,
not a physical observatory interlock, and Director does not operate the shutter.

Real NINA #64/ASCOM OmniSim/local-server tests cover enclosure closure during an
offline exposure while weather remains safe: exposure abort, mount stop,
blocked park and no restart after reopening. A separate Unsafe test retains park
for a commissioned open-air setup. Native editor screenshots cover 640/1000 px;
742 plugin tests cover freshness, clock/profile/device changes, cancellation,
failed stop and clearance loss during park. Evidence is in the plugin smoke guide.

Plugin [#42](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/42)
adds a saved Session **On abort** policy: park with independent enclosure
clearance (the legacy default), or stop slewing/tracking without parking.
It applies to operator cancellation, unsafe conditions, timeout and acquisition
failure after launch. Normal completion still parks; planned waits retain their
separate setting. A blocked or failed park falls back to stop commands; failed
stops remain errors. NINA's tracking setter returns the resulting tracking state,
so `false` confirms tracking-off rather than failure. Native offline tests cover
both abort choices and enclosure closure; 751 plugin tests and 640/1000 px native
editor checks passed. The plugin smoke guide records exact evidence. No server
API or planning contract changes are required for this local adapter policy.
This does not yet commission the persistent observing-night recovery policy or
authorize automatic resume, repeated guide/slew recovery, or shutter movement.

Plugin [PR #43](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/43)
adds live/deferred capture delivery, a settings
button, a native **Director Check In** instruction and prior-run delivery at
session start. New runs persist their original allocation, geometry, initial
state and ledger identity before launch. Check-in reopens the exact ledger
through the shared runtime for reporting only, including after expiry or a
change to current equipment. It never requests a launch/reservation or operates
equipment. The acquisition lease excludes concurrent acquisition; use the
instruction before/after the Session, not inside its target hooks. All paths
reuse bounded capture pages and the existing durable acknowledgement cursor.

Automatic workloads persist clean completion only after native parking and
verification of no unresolved capture/preparation. A delayed check-in can then
confirm the old terminal release idempotently without advancing a newer local
request. Normal intake reconciles that released history before seeking fresh
work. Aborted/uncertain runs cannot use check-in as restart authority. Live
status is independent: backfilled captures or release confirmation must not be
displayed as current mount state or as a fresh historical operation. The local
session duration also bounds its initial backlog delivery.

No new server endpoint is needed for this increment. Existing scoped capture
check-in and workload release endpoints accept the original ledger/cursor;
release confirms the workload's recorded terminal parked/quiescent boundary,
not a new live rig observation. Scope remains exact origin, coordinator,
catalog, rig, profile and paired client. Re-pairing does not silently transfer
another client's history. Old prototype runs without the new archive record
need explicit reconciliation. Image uploads, grade/revision pull, timing and
recovery journals, persistent observing-night admission and automatic operation
defaults are not delivered by this capture-only increment.

Validation used the packaged plugin and runtime 0.9.0 / IPC 9 with native NINA
3.3.0.1064, ASCOM OmniSim and an isolated PSF Guard server. Deferred multi-target
capture, offline automatic completion followed by batch release, and the normal
live/offline Moon-avoidance path passed. Repeated check-in sends no duplicate
events; the original allocation still cannot launch again. All 771 plugin tests,
packaging, format and manifest checks passed. Native Session/settings screenshots
were reviewed. Exact run evidence is in the plugin's `docs/nina-smoke-test.md`.

The next native recovery increment must keep one persistent per-rig recovery
directory across allocations, admit an explicit
observing night, prevent downgrade after commissioning, classify local quality
evidence, and bind original suggestions to one-shot native checks. UI work must
not offer an enabled recovery policy before that acquisition path is tested.

#### Remaining acquisition integration

The core chooses among acquiring, holding, probing recovery, stopping and
stopped. Record a typed cause, evidence, affected scope, first/last occurrence,
policy revision, retry budget, earliest retry and terminal stop reason. Safety
preempts quality recovery. A hold blocks new science reservations in its scope
and does not mark goals complete, reject images or release unresolved capture authority.
Operator stop and **Park and stop** are available from every nonterminal state.
**Stop for the night** persists across reconnect, plugin restart and workload
renewal; identify the observing session/night explicitly, not by UTC midnight.
Restart must restore the latch before any equipment operation. A new session
requires fresh admission and safety checks; reconnecting or receiving work
cannot clear it.

| Cause | Local response | Recovery |
|---|---|---|
| Corroborated cloud/transparency loss or sustained poor frames | Finish the current exposure by default, retain its evidence, then hold acquisition; configurable immediate abort for severe loss | Wait for the configured cooldown, then perform bounded recovery probes; require sustained good evidence before resuming |
| Guide loss, failed settling, autofocus or centering | Cancel dependent work and record whether the native operation stopped cleanly | Allow only configured, bounded recovery for a known outcome; repeated failures hold or park and stop |
| Slew failure, unknown mount state or uncertain operation completion | Stop dispatch and reconcile the actual device state | No blind repeated slew or implicit retry; park only when the commissioned mount/enclosure policy permits it |
| Required safety monitor Unsafe, disconnected or stale; enclosure closing/closed | Immediately block dispatch and cancel exposure, guiding and other active work through native APIs | Execute the commissioned shutdown policy and latch stopped; Safe/Open alone never restarts the interrupted owner |
| Hold deadline, recovery budget or latest useful observing time reached | End recovery attempts and execute configured shutdown | Stop for the session/night, or explicit operator recovery |

Quality observations must identify the rig/configuration, target, filter,
exposure/binning, capture ID, observation time, source and algorithm revision.
Reuse PSF Guard's [screening evidence](../SCREENING.md) and
[statistical grading](../STATISTICAL_GRADING.md) concepts: transparency, spatial
obstruction, star counts and tracking/focus evidence. Compare compatible frames
and retain a known-good reference. A target/filter/exposure change must not
look like a sudden cloud; a long bad run must not become recovery merely
because a rolling baseline adapted. Missing metrics are unknown, not bad or
good. A single low star count, no-solve or final rejection is insufficient to
infer clouds. Separate sky-wide evidence from target-local obstruction or
focus/tracking problems so the scheduler does not cycle every target through
the same failure. Only independently feasible work may continue when a hold
is explicitly scoped to one target.

Local lightweight evidence supplies the immediate loop. Delayed central
analysis can reinforce or correct its interpretation, with age and identity
checks; an old grade cannot pause or resume unrelated current work. Server
loss alone is not cloud evidence. When required quality evidence is missing
or stale, use an explicit commissioned policy (continue with unknown quality
or hold); never wait indefinitely for an online grader. Image grading and
accepted/pending goal accounting remain separate from session control.

Expose grouped **Quality**, **Failure recovery** and **Shutdown** policy in the
existing Session editor, with effective inherited values and local overrides.
Quality supports disabled/monitor-only, timed pause, and park-and-stop modes;
show bad/good evidence thresholds, minimum samples, cooldown, maximum total
hold, probe count and latest resume time. Initial values to validate in
simulators are a 10-minute cooldown, at most three recovery probes, and a
45-minute total hold before park-and-stop. Good-evidence hysteresis must be
stricter than merely waiting out the timer. These are proposed defaults, not
universal cloud thresholds or permission to weaken hardware safety.

Recovery probes need explicit local authority, operation deadlines and separate
bounded accounting. They may require unpark, slew/center and guiding; refresh
constraints before each step. Probe frames retain provenance and do not count
as accepted science automatically. If the mount is parked or the roof is closed,
do not take a probe until the enclosure/safety contract permits the complete
setup. Timer expiry only permits evaluation of recovery. Resume needs fresh
safe evidence, a valid allocation and local ownership, reconciled operations,
and a new core decision after setup. No exposure may outlive its geometry,
meridian or session deadline.

Count consecutive and total failures by operation/device as well as by target.
Cap nested native/plugin retries and total elapsed recovery time so changing
targets or creating a successor workload cannot reset the budget. A known
recoverable guide failure may permit one configured restart/settle attempt;
unknown results require reconciliation. Preserve uncertainty and spent attempts
across restart. Clear consecutive failure counters only after verified recovery;
retain session totals until an explicitly admitted new session. An HTTP check-in
or new command ID resets neither budget.

Commission enclosure interlocks per rig: closing/closed/motion-permitted signals,
who owns closure, whether park-before-close is required, and which shutdown
actions remain permitted if closure has already begun. Check these interlocks
before startup/unpark as well as throughout acquisition. Never infer clearance
from a generic Unsafe flag. A closing roof must not trigger repeated slews,
unparks or guide restarts. Where mount motion is prohibited or its clearance is
unknown, abort/stop through the supported device path and surface the blocked
park explicitly. Where park is permitted, issue it once with a deadline and
verify the result. A failed park remains a visible terminal failure; do not
claim the rig is safe or repeatedly move it. Hardware enclosure interlocks
remain independent of Director.

Use the existing native lifecycle and one shutdown owner for Director defaults
and compatible unsafe hooks. Deduplicate repeated Unsafe/roof/failure events,
preempt recovery waits, and keep bounded cleanup alive after sequence
cancellation. A local watchdog enforces safety even if the core or server is
unavailable. Status must show the cause, evidence age, cooldown remaining,
probe/retry budget and actual mount/enclosure state. Journal transitions and
operation outcomes for batch check-in; central live status shows freshness and
cannot remotely clear a local safety/stop latch.

Implement in this order: shared-core states and persisted budgets/latches;
versioned observation/decision contracts; native stop/park/enclosure enforcement
and Session controls; local quality evidence and bounded probes; then central
reinforcement and monitoring. Add deterministic tests for false cloud signals,
baseline drift, repeated failures, stale/out-of-order evidence, timer/clock and
restart behavior. Real NINA simulator gates must inject cloud recovery and
persistent cloud, guide/slew failures, Unsafe during exposure/setup/wait/cleanup,
roof closure with motion allowed/prohibited, park timeout, flapping safety and
server outage. Assert bounded attempts, no duplicate shutdown, no automatic
restart after a terminal stop, and no capture outside current authorization.

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

### Central rig telemetry

Phase 2 includes a PSF Guard rig-monitoring UI, not just local logs or a future
Chatstronomy bridge. A connected Director sends scoped status and durable
execution events to the coordinating instance so operators can watch rigs,
current project/target/objective, active operation and elapsed time, safety,
assignment/revision, accepted versus pending progress, next check-in, local
errors and queue depth. Label forecasts as estimates and show the age of every
snapshot. A disconnected or stale rig becomes unknown/stale, never falsely
"still exposing" or successfully stopped. The monitoring page should not require
image bytes, thumbnails, or a completed catalog reconcile.

Separate bounded, coalescible status snapshots from durable operation/capture
events. Network telemetry must not block the image-save path or N.I.N.A. safety
thread. Define versioned ingestion, backpressure, reconnect and browser updates,
rig-scoped authentication and operator read permissions. Capture/configuration/
assignment identity must accompany relevant state; suppress credentials and
machine-local paths. A reported state or a dashboard button is not an allocation
or unrestricted device command. Any allowed request enters local authorization
and a safe boundary; immediate remote stop cannot be promised while disconnected.

Persist authoritative transitions before transmission and derive monitoring
projections from acknowledged events plus timestamped live status. Keep server
receipt time distinct from rig event time and clock uncertainty. Old events
backfilled after an outage must not replace fresher live status. Retention may
compact disposable samples, but not unacknowledged capture/operation evidence.
Expose storage pressure before exhaustion and stop new work if required durable
evidence cannot be recorded. The server side of this exists: status and
check-in intake, contact records by receipt time, and the Live table with
connectivity and stale labelling. The plugin's sender and the browser push
channel are still to come; the sidecar outbox remains a local building block.

### Offline and batch check-in

Offline operation is a supported mode, not just an accidental lost connection.
After commissioning/pairing and a successful assignment check-in, a rig may
start or continue authorized work from a cached, validated program without
central PSF Guard running. It still needs valid local conditions, geometry/time
evidence and safety. Authentication, assignment validity, attempt budgets and
pending-grade limits are separate; a cached login is not indefinite authority.
The first pairing/new allocation cannot occur offline. Assignment expiry,
missing required evidence or exhausted local storage prevents new work while
preserving safe completion/cleanup and unsent evidence.

Provide explicit **Check in** / **Reconcile Director** actions in plugin settings
and as sequencer steps, plus automatic start/end, target/block, periodic and
significant-condition checkpoints. Support connected, periodic and deliberate
end-of-session batch delivery without requiring a permanently reachable server.
These share one resumable reconciliation path, not different merge rules. Batch
check-in transfers bounded event pages, timings, progress/assessment revisions
and requested intent/configuration changes; it is not streaming whole SQLite
files back and forth. Image transport remains separate and may be deferred until
after the batch check-in, with missing pixels represented explicitly.

Use stable rig/ledger identity and independent cursors for each existing event
feed. A server inbox commits deduplication, event application and acknowledgements
atomically. The client retains unsent/unacknowledged evidence across crashes and
lost replies, retries idempotently, and only compacts acknowledged history under
the retention policy. Show phase, counts, cursor progress, last success and
recoverable failures, not "Working". Define page/byte limits and cancellation
between committed pages, so large backlogs resume without resending everything.

On reconnect, account for old-revision captures before activating replacement
intent and allocation. An acknowledgement must name the exact accepted event
cursors and grading/configuration revisions used in the next baseline; retries
cannot add local pending credit twice. Record unsupported/conflicting events
without discarding them or falsely acknowledging them as applied. Do not silently
shrink an offline rig's authorization and assign the same outstanding budget to
another rig. Release acknowledgement or conservative expiry/clock-skew rules
must fence that handoff. This protocol must also work when the central instance
restarts during a batch. Telemetry reconnect and manual batch reconcile use the
same durable evidence and admission rules.

### Executor and coordinator interface handoff

This is the required interface for the plugin/backend work split, not a claim
that the endpoints or production container exist. Frontend/setup work must not
invent a second rig or project identity to satisfy the plugin. The plugin uses
the registered database's reviewed `catalog_rig` binding and the shared Rust
program/geometry contracts. It does not read arbitrary remote SQLite files or
treat TS tables as the planner's internal schema.

| Boundary | Required information and behavior | Current state |
| --- | --- | --- |
| Commissioning | Coordinator instance UUID, durable catalog UUID, bound rig UUID, local N.I.N.A. profile binding, permitted projects and scoped credentials. The database slug is a locator, not identity. | Exact coordinator/catalog/rig/profile/client pairing with a separate vault. The operator-selected client can fetch and launch only its immutable allocation once, not admit projects or renew work. |
| Assignment check-in | Versioned immutable program, assignment ID/revision/validity, project/contribution/source-project identities, configuration/site/rig revisions, effective policy provenance, and offline authorization/budgets. Validate engine and contract compatibility before activation. | Stable previews, immutable first allocation, strict intake and one-shot launch are implemented. Refresh/reconciliation, successors and policy provenance remain missing. |
| Native execution | Rust selects and issues work. N.I.N.A. runs native items in target context with inherited triggers/conditions and the TS-compatible instruction slots. Each newly issued command/reservation has one native invocation. | Public prepared-target container runs the internal adapters with local safety, ownership and cleanup. Automatic policies, full recovery and third-party parity remain unfinished. |
| Local status | Session/ledger, database/rig/project/target/goal IDs, assignment revision, operation and monotonic elapsed time, wait reason, local safety, connectivity, last successful check-in and queue depth. | Public session reports phase/rig/target/goal/operation, safety, connectivity and checkpoint state. Full per-operation timings and project provenance display remain missing. |
| Central reporting | Coalesced live status is separate from durable capture/preparation receipts. Network errors update connectivity only; they do not fail an otherwise authorized local exposure or erase evidence. `POST /rigs/{rig}/status` and `GET /rigs/status` exist, with contact records and the Live table. | Optional bounded session sender is implemented. Authorization or malformed acknowledgements stop work; transport outages may continue under the explicit offline policy. |
| Batch reconciliation | Bounded independent event pages, exact feed cursors, idempotent acknowledgements, grading/configuration revisions and replacement assignment proposal. Manual, sequencer and automatic check-ins use the same implementation. `POST /rigs/{rig}/checkin` stores pages once and acknowledges contiguous cursors. | Current increment delivers capture pages and persists scoped cursors without deleting journal evidence. Preparation-feed delivery, production entry points, grade application and replacement assignment proposal remain open. |

The planning-flow endpoint table names the intended `/api/director/v1`
interfaces and records which are still planned. Finalize their wire contracts
with implementation; the plugin must not silently call an unimplemented endpoint or
reuse the operator-only metadata API for rig credentials. Keep browser/operator
management and rig execution authority separate. Enrollment binds the caller to
one coordinator/catalog/rig tuple; every check-in must reject a mismatched tuple
even when the supplied slug exists. A database rename or move preserves that
tuple. A cloned independent rig requires explicit new lineage and enrollment.

The assignment envelope must carry stable links from shared project/objective
to rig contribution, source project GUID, target and recipe. Multiple projects
within the selected rig database remain candidates; the local core decides
what to acquire. A local integer row ID, display name or profile ID cannot
substitute for source identity. Downstream projects with different framing or
exposure purpose retain their own contribution identity and are not equivalent
accepted-progress credit merely because they roll up to the same project.

Commissioning selects the database-backed rig and local profile once; saved
sequence configuration retains those identities and hook/default options, never
credentials, live callbacks or a serialized dispatch permit. Session startup
validates the cached assignment, current profile/equipment, local horizon,
site/time evidence, safety policy and exclusive acquisition ownership. An
offline cold start is not supported by prepared-target mode; its running
process may continue offline after acknowledged online launch. Future resume
must prove authorization and evidence validity. No connection, startup hook or status subscriber may fabricate a safe
state, extend expiry, reset attempt budgets or revive an uncertain operation.

Preparation/capture receipts are journaled before delivery. A page acknowledgement
must name its coordinator, catalog, rig, ledger, feed and highest contiguous
committed cursor. Never acknowledge beyond the page, across feeds or across
ledgers. A lost reply is retried with identical event IDs; late old-session
status cannot overwrite current live state. The local queue retains evidence
until exact acknowledgement; no image bytes or thumbnails are required for
status or execution reconciliation. Deferred image upload is a separate policy.

The public container needs one production coordinator that owns target entry/
exit, wait transitions, preparation and capture boundaries, safe stop/cleanup,
and checkpoint scheduling. Slot helpers alone do not decide when **After Each
Target** or **After Target Complete** is due. Distinguish completed objective,
temporary target switch, failed operation and interrupted session in receipts.
Slow hooks invalidate the previous decision and require fresh core dispatch
validation. Do not turn a native container's apparently successful return into
success when a child failed, was skipped, or never completed.

Core dispatch deadline transport and the internal TS-style slot primitives are
implemented. Next, define the database-bound assignment and acknowledgement
APIs with backend work, complete the native option contracts, and replace
the simulator-only loop with the guarded session coordinator; then expose the
familiar public container/UI and live/batch status. Each increment must state
its missing gates. The full gate remains a real N.I.N.A. simulator sequence
against an isolated PSF Guard instance, including an outage, restart, slow hook,
unsafe transition and idempotent batch reconnect.

### Program-intake audit after #528

#### September 30 handoff correction

Tracked in [PSF Guard #606](https://github.com/theatrus/psf-guard/pull/606) and
[Director plugin #30](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/30).

The server now persists preview issuance in meta schema 14. Identical inputs
return the same assignment ID, ETag, issuance time, validity and goal windows
across unconditional pulls and server restarts. A changed input, expired span,
or clock rollback creates a new preview identity. These are still **previews,
not execution leases**: automatic replacement and fresh attempt budgets are
not authorization to open another ledger.

Program validity is 24 hours, matching the geometry core's maximum interval.
The earlier 36-hour server interval passed program validation but failed the
sidecar's geometry-open boundary. A regression test now sends actual compiler
output through `BoundGeometry`, in addition to HTTP and immutable-retry tests.

Equipment reports accept optional `filter_names`, a complete map from exact
configuration filter IDs to native labels. Legacy reports without it continue
to use IDs as labels. The compiler requires a unique exact-label or bandpass
match; it never changes the ID sent back for native dispatch. Incomplete maps,
unknown IDs and invalid labels are rejected. This does not yet define a saved
operator-selected mapping between two filters of the same bandpass.

Explicit unsupported camera values are omitted with a reason, never clamped,
wrapped to a smaller integer, or replaced by the first supported mode. An
unspecified readout mode is accepted only when there is exactly one mode.
Candidates must retain their activated catalog/target/project ancestry, active
target and project state, and valid J2000 coordinates. Editing a plan draft
after activation suppresses that project's candidates until reviewed activation
is applied again; it cannot silently change running priorities. Freezing the
activated metadata would allow the old program to remain available while a
new draft is edited, and is still follow-on work.

The plugin's opt-in `run-server-plan-smoke.ps1` harness creates a disposable
loopback server/catalog and isolated NINA nightly #64 profile. Its operator-only
setup reports equipment and activates three one-frame filter goals; the paired
client pulls and caches that actual program. The harness then stops the server,
checks that it is unreachable, and exercises native captures, local sidecar
restart, server restart, receipt delivery and duplicate replay. Its safety input
now comes through NINA's built-in safety simulator and its orientation input
comes from NINA's local IERS cache. No production container or permission to
acquire is added by this test.

This combined test passed on September 30 with NINA 3.3.0.1064, runtime 0.7.0 /
IPC 8: three correlated FITS captures, seven native preparation operations,
six inherited exposure hooks, offline sidecar restart, unchanged preview after
server restart, six acknowledged capture events, duplicate replay without
duplicate credit, and paired live-status readback. The plugin's focused smoke
guide records the evidence path. The 69 server Director tests, metadata suite,
575 plugin tests and focused lint/build checks passed. This is a test-harness
integration gate, not the public acquisition acceptance gate.

The next user-facing session must still supply:

- explicit local start/stop and ownership, with a TS-style container and seven
  native instruction slots;
- production binding of the tested safety/orientation adapters, safe interruption
  and recovery;
- reviewed equipment-report permission for paired clients (currently operator
  only), without granting them general database-management access;
- capture/preparation accounting before any successor assignment, including
  uncertain work, rejects and images awaiting grading;
- bounded background check-in and coalesced live status, neither of which may
  block native safety or discard offline receipts.

#### Local evidence increment (September 30)

Plugin [#32](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/32)
adds two internal native evidence adapters, exercised in the
isolated nightly host, not yet connected to public acquisition admission:

- The safety interlock binds one profile and monitor. Only device broadcasts
  renew evidence. A 250 ms local watchdog checks connection, identity and age;
  unsafe/disconnected/stale evidence cancels the owner. Freshness is three
  polling intervals, at least five seconds; polling above ten seconds is refused.
  Cancellation remains latched after Safe returns. Profile changes invalidate
  even unarmed owners; resumed broadcasts after sleep and clock regression
  cannot hide a stale interval. Cancellation callbacks cannot block the monitor
  lock or throw through NINA's broadcast thread.
- The orientation reader opens NINA's IERS SQLite cache read-only. It requires
  three consecutive dated rows bracketing an assignment no longer than 24 hours,
  checks MJD and physical ranges, and converts pole arcseconds to radians.
  Missing data fails without network access, migration, or zero defaults.
  IPC 8 has one fixed EOP sample: use the middle row only when observed adjacent
  drift is at most 2 ms UT1 and 0.01 arcseconds per pole component. This is a daily
  approximation, not a proof of intra-day error. Retain sample provenance and
  immutable ledger constraints; time-series interpolation needs a future contract.

The updated server-plan probe uses the built-in NINA safety simulator and actual
local IERS rows. It retains all three-capture/outage/restart/check-in checks and
interrupts a running native Wait instruction when the monitor becomes unsafe.
Safe recovery cannot revive the interrupted owner. All 612 plugin tests passed.
The smoke guide records the final native evidence path. No new Rust, server or
frontend behavior is claimed by this documentation update.

Before production use, define the shared core's conditions horizon separately
from monitor freshness. A fresh Safe observation does not forecast an entire
long exposure. Keep continuous native cancellation while an operation runs, and
preserve uncertain captures for recovery. Successor allocation accounting,
equipment-report permission, exclusive local ownership, automatic shutdown and
explicit restart admission remain open. The public container stays blocked.

#### Immutable first-allocation admission

Meta schema 15 adds a single retained execution allocation per rig. An
interactive editor (or the explicitly trusted local operator) calls
`POST /rigs/{rig}/allocation` with `coordinator_instance_id`, `catalog_id`,
`allocation_id` (caller UUID for retry), `client_id`, and the reviewed
`preview_revision`. The handler recompiles/checks that revision, verifies the
live catalog/rig/client/profile binding, and freezes the full envelope. The
shared core validates the program. The new assignment is named
`allocation-{allocation_id}`, distinct from the preview ledger identity.

The response has `schema_version: 1`, those scope IDs plus `profile_id`,
`admitted_at_ms`, `preview_revision`, and `snapshot` (the complete frozen program
envelope with a new revision). `GET /rigs/{rig}/allocation` takes the same
coordinator/catalog query parameters as program preview. Both responses are
`no-store`. Existing pairing credentials receive no blanket allocation grant:
only the client explicitly selected by the operator can fetch it, using its
normal bearer token and profile header. Other clients, even a second pairing
for the same profile, are refused. Paired clients cannot POST admission.

Check-in/status `program_revision` continues to name the source preview revision
for change hints, not the new allocation snapshot revision. Reports can include
allocation identity/revision in their status payload, and capture events retain
the allocated assignment identity. A plan-change hint never replaces a grant.

Admission is transactional and idempotent. An exact retry returns the original
snapshot even after catalog/preview changes. Any different allocation or client
conflicts. Expiry, token revocation, re-pairing, restart, backup/restore and grade
delivery do not remove the record, extend validity, or replenish counters.
Pre-existing legacy receipt feeds also prevent first admission because their
outstanding work has not been reconciled. Manual admission still has no delete
or replacement path. Commissioned automatic workloads use the separate clean
terminal-release protocol above. Do not clear this table to start another night.

The plugin has separate strict allocation intake and an atomic origin/binding/
client-scoped cache. It checks all identities, configuration, ancestry links,
size and validity. Cache storage rejects replacement even when the old grant
expired. Configuration changes do not select a fresh empty cache. HTTP failures,
especially revocation, never silently fall back to disk. The caller must choose
offline continuation explicitly under an admitted session's policy; revocation
cannot retroactively stop a disconnected rig, so validity and local stop rules
still apply.

This is allocation delivery, not the full ownership lifecycle. Prepared-target
mode adds online single-launch admission, local ownership and clock continuity;
lost or copied state cannot launch that grant a second time. Remaining gates
include operator admission UI, paired equipment reporting, restart/resume,
native automatic operation policies and uncertain-work reconciliation before a
successor grant. The simulator acquires from this issued snapshot, verifies
its offline cache, and checks that server restart and receipt delivery leave it
unchanged while the ordinary preview updates its pending counts.

The September 30 native gate passed with NINA 3.3.0.1064, runtime 0.7.0 / IPC 8,
the schema-15 local server and ASCOM OmniSim: three correlated FITS captures from
`allocation-4638609a-6b14-4bee-85b9-0ffdd9ec9ba3`, offline ledger restart, six
acknowledged reservation/save events, unchanged allocation after server restart
and delivery, correct preview-change hints and latched native safety cancellation.
The plugin smoke guide holds the evidence path. These are local results, not
hosted CI or public unattended-acquisition acceptance.

The historical findings below explain those remaining boundaries; the immutable
preview, filter fidelity and catalog eligibility defects are corrected above.

Reviewed against PSF Guard `5258e4b` and Director plugin `5715e16` on
2026-09-27. The planning and database-activation path is now present. The
remaining gap is not a missing URL: its response must become a stable,
accounted-for allocation before it can feed the durable native executor.
Keep automatic scheduling and restart/resume gated while these contracts are
incomplete; the prepared-target mode has the narrower contract documented above.

- **Immutable issuance and expiry.** `src/server/director/program.rs` hashes
  content without time, then rebuilds assignment validity and goal windows
  from each pull's clock. Unconditional pulls can change the program under the
  same assignment ID and ETag; conditional pulls can keep returning `304`
  after its cached validity expires. The ledger correctly rejects changed
  allocation/program bytes. Persist the exact issued snapshot, including its
  original validity. Expiry needs an explicit successor-allocation decision,
  never a new span under the same identity or an unconditional-pull workaround.
- **Refresh must retain progress.** The compiler currently sets `pending` to
  zero and derives a fresh attempt budget from accepted counts. Saved/ungraded
  captures and outstanding attempts are not accounted for. A changed progress
  baseline changes the assignment identity, but there is no ledger replacement
  or acknowledged handoff yet. Never open another ledger to bypass a mismatch.
  Check-in must reconcile stable capture IDs and outstanding allocations before
  installing a successor; until then, do not replace active work automatically.
- **Activated content, not drafts.** Program compilation reads the current
  plan draft for objective priority and link metadata, not an immutable snapshot
  matching the activation's plan revision. Editing an unactivated draft can
  therefore change pulled scheduling behavior. Freeze that metadata at activation
  and keep draft edits inert until reviewed Apply.
- **Filter identity and recipe fidelity.** The native exporter sends opaque
  filter IDs and slots; labels remain local. The server currently guesses a
  bandpass from those IDs, which cannot resolve the real probe's `filter-0`
  against a `Red` template. Define an explicit mapping from template GUID and
  bandpass to the reported stable filter ID, with display names as evidence,
  and refuse ambiguous matches. Unsupported requested gain, offset, binning or
  readout must require review, not silent clamping or first-mode substitution.
- **Catalog eligibility and ancestry.** The pull currently checks only the
  exposure plan's enabled flag, not its target's `active` flag or project's
  `state`. Preserve those operator controls when compiling candidates. Verify
  the exposure plan still belongs to the activated target and source project;
  independently finding each GUID does not prove the live parent links agree.
- **Commissioning and reporting.** The audit found only operator authentication,
  not rig enrollment. The commissioning increment below adds separate scoped
  pairing and capture acknowledgements; equipment reporting remains operator-only.
  Production background delivery, preparation events and live plugin status still
  need integration. Do not expose an operator token in plugin settings or silently
  reuse Sync credentials. Disconnection must preserve local evidence and must not
  erase or expand already authorized work.

Plugin work can proceed on typed, bounded intake and validation without arming
equipment. Bind the coordinator/catalog/rig tuple and local NINA profile,
validate schema/engine compatibility and identity links, then persist a complete
validated snapshot atomically. Handle `304` only with that exact unexpired
snapshot; never treat it as renewed validity. An unknown or malformed response,
configuration mismatch or unreadable cache must fail closed with useful status.
Keep this separate from the immutable-program selection policy in Rust.

Delivery order from this audit: fix immutable issuance and activated inputs;
settle explicit equipment/template mapping; connect read-only plugin intake;
add acknowledged checkpoint/replacement accounting; then connect the guarded
production session. The public configuration editor and seven-slot cadence
owner are now tested; automatic operation ownership remains disconnected.
Before enabling acquisition, test a real
server program with the NINA simulators, then repeat through expiry, lost
network, restart, saved-but-ungraded captures and batch reconnect. Existing
separate server and simulator tests do not cover this combined gate.

Implementation update after the audit: Director plugin
[#28](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/28) adds
`CoordinatorProgramClient.ReadPreviewAsync`. It binds the coordinator/catalog/
rig/profile and current native configuration, validates schema 1 and exact goal
links, bounds HTTP/body processing, and rejects changed content under the same
ETag or assignment identity. It sends unconditional GETs and rejects `304`
because it has no durable cache yet. Credentials come from an in-memory provider;
there is no API-key setting or credential file. The result is explicitly a
preview, not a ledger activation or equipment permit. Engine-version negotiation
is absent from the current server envelope and is not claimed by this client.

### Pairing, preview cache and capture checkpoint increment

Implementation and local evidence, 2026-09-27; this is not a published acquisition
workflow. Backend [#535](https://github.com/theatrus/psf-guard/pull/535) and plugin
[#29](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/29) carry this
increment; plugin validation includes 574 passing tests and 12 nonblank WPF renders.
Director-specific `psfdpt_` one-use codes mint `psfdrc_` credentials
bound to the coordinator/catalog/rig and NINA profile, with a separate client ID.
Schema 11 stores only secret hashes; issue/consume/revoke are independent of Sync.
The three scopes permit only GET program and POST checkin/status. Operator APIs
issue codes and list/revoke clients; a pairing-code management UI and scoped
equipment registration remain open. The plugin keeps secrets in Windows Credential
Manager and nonsecret enrollment with the profile; no operator-token setting.

The plugin's durable preview cache validates origin, tuple, native configuration,
schema and immutable fingerprints and publishes complete snapshots atomically.
Expired snapshots remain historical evidence to detect revision drift, not offline
permission. Conditional `304` remains refused by intake; the cache does not renew
validity, replace a ledger, reset budgets, or resolve the compiler blockers above.

The capture checkpoint client reads bounded ledger event pages after its stored
cursor, validates exact acknowledgement identity and page outcomes, and persists
the acknowledged cursor atomically under a cross-process lock. Lost replies can
replay identical events. It retains all journal evidence and never uploads images,
applies grades, handles preparation-feed events, or dispatches equipment. The server
also rejects an entire page if the globally keyed ledger already belongs to another
rig, checking both stored events and its cursor before writing.

Real local smoke evidence: NINA nightly `3.3.0.1059` with ASCOM OmniSim and the
shared-core sidecar captured three FITS frames, completed seven preparation
operations and six inherited hooks. The actual plugin credential vault stored and
retrieved the scoped token. After sidecar restart, the client delivered six
reservation/saved events to an isolated PSF Guard server. A fresh checkpoint client
resumed at cursor 6 with zero new events; an independent fresh cursor replayed all
six as server-acknowledged duplicates. The test credential was revoked and removed
from the vault, and NINA closed normally. Local plugin evidence is
`artifacts/nina-smoke-e60b4c8bf97d4fa6922cb47a360cb037/probe/8707ef9c7d6644599f3e1846693cb785/result.json`.
That native run used the isolated pairing/ledger-isolation server build from
before the #533/#534 rebase and final malformed-JSON error sanitization. Those
final changes were validated separately by local HTTP/storage tests and clippy;
the native run does not claim to exercise the later frontend or parsing changes.

This run used fixture allocations, not a server-issued acquisition authorization.
Production automatic/status delivery, preparation feed, manual/sequencer controls,
offline allocation lifecycle, grade reconciliation, replacement accounting and the
full outage/safety/session lifecycle gate remain unfinished. Pairing and a cached
preview must not open the public acquisition gate.

The plugin passed 539 tests, including 45 intake cases and a real loopback
redirect test, plus a separate NINA nightly #59/ASCOM regression run with three
FITS captures. The HTTP tests use controlled responses, and the NINA fixture
does not consume HTTP programs. The combined acceptance gate remains open.
Server #530 adds receipt/status endpoints and saved-receipt pending credit,
superseding the audit's statement that those server endpoints are missing; it
does not supply the plugin sender or a reviewed allocation-replacement protocol.

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

Future design note only, outside the active implementation scope. A later
revision may support collaborative projects with other people and independent
PSF Guard instances. Do not implement invitations, participant coordination or
cross-instance exchange as part of the framing wizard or current multi-rig work;
that needs a separate implementation request. The following are future constraints.

Extend the same project/workload/contribution model used for one owner's rigs,
not a second collaborative scheduler. Retain coordinator and participant
identity, immutable grant identity, stable capture identity and assessment
provenance across the boundary. A participant can offer permitted capacity or
request work; accepting a shared project does not grant remote control over its
equipment. Shared-core capability matching and priorities must still respect
the participant's commissioned local policy. Collaboration APIs and permissions
remain deferred; do not add cross-instance execution in this increment.

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

Phases 0 and 1 have merged building blocks; neither acceptance gate is complete.
Phases 2-5 remain incomplete even where a lower-level primitive exists. Phase 6
is a deferred design note, not part of the current implementation scope. Checked
items below refer only to the stated implementation scope, not adjacent goals.
A phase is complete only when its full acceptance gate passes and its review and
validation evidence is linked here.

### Phase 0: compatibility and execution spike

- [x] Pin N.I.N.A. nightly #58 (`3.3.0.1058-nightly`), record the tested reference
  matrix and publish a runtime-only plugin with an exact sidecar artifact pin.
- [ ] Prove Director-owned execution through supported N.I.N.A. APIs without
  TS installed; preserve ordinary TS and Sync behavior when separately installed.
- [ ] Inventory the pinned TS container options, operation policies, conditions,
  and trigger lifecycle. Map each to shared-core policy or native N.I.N.A.
  execution, with parity tests and explicit reasons for any intended difference.
- [ ] Validate asymmetric meridian constraints against local TS reference cases
  and prove N.I.N.A. horizon export/parity; include multiple safe intervals in
  the engine contract.
- [x] Link the same core into PSF Guard and a bundled sidecar, and exercise typed
  C# planning, version negotiation, packaging, local failure and recovery tests.
- [x] Connect that tested local path to a PSF Guard first allocation and capture
  acknowledgements in a real public N.I.N.A. simulator session, including outage
  and replay refusal. Grade feedback, successors and the full lifecycle remain
  unchecked in later phases.
- [x] Implement a deterministic core linked into the PSF Guard Rust library and
  replay shared vectors through a native library from a .NET 10 console host.
- [x] Represent multiple eligibility intervals and subtract asymmetric local
  meridian exclusions without bridging horizon gaps; validate transit coverage.
- [x] Choose a separate thin plugin plus bundled Rust sidecar, following the
  Chatstronomy core/plugin distribution model with versioned local IPC.
- [x] Exercise the same golden decisions through a real Windows sidecar with
  bounded framing, version negotiation, peer checks, and process failure tests.
- [x] Add native exposure/target items and post-hook dispatch checks, and run the
  internal adapter in an isolated N.I.N.A./OmniSim sequence with FITS readback.
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

The following evidence records incremental building blocks, not independent
claims of current product completeness. The audit above identifies what is
merged, published, under review and still missing.

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
full-stack acquisition gate. Signed artifact provenance and Chatstronomy state
integration remain open gates. Later probes cover internal native execution,
not a production coordinator session.

The internal capture adapter uses N.I.N.A.'s public imaging and save interfaces.
It reserves a capture GUID before dispatch, snapshots the original profile's save
settings, writes `PGCAPID` into FITS/XISF metadata, and waits for a correlated final
save receipt. Queue admission is not save completion. Timeouts and cancellation
after admission retain uncertain evidence; they do not authorize another attempt.
The profile-scoped journal records identity, destination, and monotonic timings,
and later internal adapters correlate it with the sidecar ledger. Alongside public
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
recovery ledger. Durable accounting and native boundary checks now exist as
building blocks. Complete ownership/safety, operator recovery and server feedback
remain prerequisites for a production sequencer item. The original native smoke
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
Its meridian behavior remains useful reference evidence. Native sequencing,
hooks, image-save observation and cancellation now have partial adapter coverage;
the complete compatibility matrix is still required. No TS source or installed N.I.N.A. plugins are
changed by the shared-core spike.

The initial implementation lives in
[`crates/director-core`](../../crates/director-core/src/lib.rs) and
[`crates/director-ffi`](../../crates/director-ffi/src/lib.rs). PSF Guard re-exports
the core as `psf_guard::director`; the native library calls the same evaluator.
These two crates alone contain no server route, migration or hardware dispatch;
later crates and adapters provide the building blocks below. The
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
Windows, Linux, and macOS. Record hosted CI separately from local results and
require both for each changed contract. The existing application remains the default Cargo workspace
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

The published 0.1.0.1 Director runtime preview pins runtime 0.6.0 / IPC 7, including
the typed ledger host and shutdown drain handshake. Earlier IPC 3-6 packages
must not be mixed with newer sidecars. A mismatched version is
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
passed real N.I.N.A. runtime-lifecycle smoke tests. Signed release provenance and
full coordinator reconciliation remain gates. Internal native dispatch and local
journal recovery are implemented. The public session editor is implemented,
but its production acquisition entry point remains explicitly blocked.
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
IPC 5 introduced the bound preparation path; merged native adapters use the same
resolved recipe. Do not claim recipe enforcement from unbound APIs.
Pairing, allocation authority, production container execution,
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

The internal program-bound adapter has native simulator coverage; production
container execution remains required before unattended use. A host must
not reconstruct lost reducer state and replay an operation whose outcome is
unknown. Preparation does not consume capture attempts or credit images; the
ledger and a fresh native dispatch check remain separate requirements. Session
startup/shutdown, autofocus policy, guiding, flips, and calibration are still
open parts of the operation inventory, not implied by this initial reducer.

#### Durable execution ledger

[`crates/director-ledger`](../../crates/director-ledger/src/lib.rs) owns the
first local attempt/event storage contract. It depends on the shared core and
SQLite, leaving the planner itself free of I/O. IPC exposes it through explicit
storage operations. The merged internal native adapter and simulator probe now
use the ledger; server delivery and a production container are still missing.
It changes no existing catalog or Sync endpoint.

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
required. The current internal adapter and ASCOM probe exercise this ledger;
the full coordinator/production-container integration must preserve these
distinctions rather than treating a
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
complete native sequence or server test. Later geometry-bound selector, ledger
and IPC paths consume this model. Darkness, Earth-orientation acquisition and
the full production dispatch workflow remain gates.
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
crate, not a native acquisition or server loop. Later window compilation and
dispatch bindings use this screening. Complete observing criteria remain
required; point/span geometry does not itself provide hardware authority.

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
This is not the complete availability compiler: darkness and the production
session remain missing. Immutable constraints and IPC bindings are implemented
by later layers. The native integration must not treat these
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
below, exposed in IPC 7. The merged internal adapter calls them after native
before-hooks; the preview gains no acquisition authority.

The internal `director-core::dispatch` API also computes an inclusive latest
start for the selected work in its current safe window. Its result includes the
evaluated timestamp and no deadline for non-acquisition decisions. Preparation
checks include the pending operation, remaining steps and capture overhead;
completed steps are not charged again. Geometry checks use the compiled horizon
and meridian windows, never a later window beyond a gap. Assignment expiry and
exclusive condition freshness further constrain the bound.

The ledger's geometry deadline checks preserve that result through the same
transaction that validates the exact issued command or linked capture and
persists any refusal. A capture check temporarily restores only its already-spent
attempt for feasibility; it never changes stored credit or reserves another
exposure. Successful checks add no issuance or capture events. Existing
decision-only APIs project the same result for compatibility. Deadlines are not
persisted as replay authority, and recovered work still requires explicit
reconciliation rather than redispatch.

PSF Guard [#518](https://github.com/theatrus/psf-guard/pull/518) exposes both
timestamps in IPC 8/runtime 0.7.0. `dispatch_checked` includes required
`evaluated_at_ms` equal to the submitted state timestamp, and required
`latest_start_ms` (inclusive for Acquire, explicit null for other decisions).
Director plugin [#27](https://github.com/theatrus/psf-guard-director-nina-plugin/pull/27)
strictly decodes these integer fields and rejects mismatched evaluation time,
wrong goals or deadlines outside assignment/condition validity. The native
adapter measures monotonic elapsed time, rounded up to milliseconds, from
before sampling the check request through a final one-use synchronous guard.
That guard runs after native setting checks, progress callbacks and durable
pre-capture journal writes, immediately before entering the native operation.
It also checks the freshly sampled wall clock and rejects regression or expiry.
A late successful reply cannot dispatch. The one-use/session fences remain in force.
This bound is feasibility for already issued work, never a reservation, replay
grant, real-time safety interlock or promise of highest priority until dispatch.

The combined plugin passed 494 tests and the official N.I.N.A. nightly #59
ASCOM simulator sequence on 2026-09-27: three verified FITS captures, seven
preparation receipts, six inherited exposure hooks, ledger reopen retaining
pending credit, same-path horizon-change detection and successful cleanup.
The plugin retains the #58 API minimum; the smoke launcher explicitly permits
the reviewed #58/#59 hosts. This fixture uses synthetic safe/Earth-orientation
evidence and a local program, not a server allocation. It does not exercise all
seven configured slots or satisfy the full coordinator/offline acceptance gate.

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
checking that neither can be dispatched twice. Plugin PRs #21-25 updated the
adapter and artifact pin together; the internal N.I.N.A. probe exercises them.

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
The internal native adapter adopts this check; the preview gains no authority.

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
coordinator instance UUID, global project and independent rig identities,
originating catalog identities, and explicit catalog/source-project-GUID links.
This prototype separation is superseded by the database-backed rig model above;
its migration is not implemented. Names and URL slugs never establish identity,
and one global project may link to several catalogs. Project and rig listings
use bounded, stable-ID cursor pages;
renaming an entity does not move it across a page boundary. Catalog IDs must be
explicitly registered and retained by the operator adoption workflow, not
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
unequal 0/360-degree endpoints. This is the current storage shape, not the final
ownership model: the site/rig separation above requires migrating the horizon
to each referencing rig setup. Each rig setup binds one exact equipment
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

- [x] Add separate owned meta storage, transactional migrations, snapshot
  backup/restore, UUID identities and explicit catalog mapping primitives (#488).
- [x] Add explicit durable catalog lineage and operator preview/apply with
  stale-evidence checks and retryable cross-database commit recovery (#502/#503).
- [x] Persist immutable sites and rig configurations with shared validation (#489).
- [ ] Separate site location/time/weather from rig horizon and equipment
  constraints; migrate existing snapshots without changing effective geometry.
- [ ] Add site time-zone and weather-source configuration, forecast versus
  observed-condition provenance/freshness, and local safety precedence tests.
- [ ] Persist versioned global planning defaults and optional site/rig/project
  overrides. Add shared-core per-field resolution, effective-value provenance
  and UI override/reset controls rather than duplicating settings per project.
- [x] Enable opt-in operator project identity API with authentication and the
  database-management gate (#490).
- [x] Merge and validate site/rig identity and snapshot operator APIs (#494).
- [x] Implement and browser-test the identity management UI (#495).
- [x] Merge the shared objective/contribution model (#492).
- [x] Persist immutable intent with validated rig-setup references (#493).
- [ ] Add the objective/configuration editor.
- [ ] Add versioned rig optical geometry, stable mosaic panels and resumable
  framing drafts, separate from immutable validated intent and allocation.
- [ ] Build the framing wizard's target/reference, FOV/rotation, mosaic,
  per-rig objective/recipe and review steps. Keep multi-rig/site intent from the
  start; do not model a project as one camera footprint or one local night.
- [ ] Add broadband and narrowband survey backgrounds, additional provider
  choices, provenance/attribution, registered overlays and bounded offline caches.
- [x] Implement and browser-test explicit catalog linking without rewriting TS
  history or merging names (#503, #506).
- [x] Discover registered catalog project/profile evidence read-only, without
  inventing rig identities or changing source schemas (#498).
- [x] Derive the Director rig inventory from registered per-rig project databases
  using durable catalog identities. Retire standalone rig creation/profile-to-rig
  selection from the normal flow; retain profiles as setup provenance.
- [x] Preview and migrate unambiguous prototype rig links to database-backed
  rigs, reporting conflicts without changing source GUIDs or capture history.
- [ ] Add explicit resolution for conflicting prototype links (several rigs in
  one database or one rig shared by independent databases). Preserve immutable
  setup/intent references; current adoption refuses these cases without writes.
- [ ] Bind plugin enrollment/check-in and site/setup resolution to that selected
  database context, including offline cached evidence and reconnect validation.
- [ ] Connect Library projects to Director planning using the same project
  identity. Preserve Library's project/per-rig catalog navigation and existing
  review workflows; Director covers planning and acquisition, not their
  replacement. Add rigs as contribution plans without duplicate projects.
  The current UI reuses the existing project/target editor and reviewed mapping
  APIs with the database-backed rig model. Library -> plan workspace -> Library
  preserves source scope. It is not a framing wizard,
  intent export, downstream-project generator or combined progress implementation.
- [x] Browser-test navigation from an existing Library project to its Director
  plan and back with project/rig/database scope intact, and two database-backed
  rigs linked to one project. Check desktop/mobile layouts and stale reviews.
- [ ] Extend browser coverage to the full planned-project workflow without
  captured images once downstream plan creation exists.
- [x] Move current source/project linking into existing database settings and
  contextual project views; retire the standalone Director Catalogs workflow.
  Full TS import/sync connection management remains separate work below.
- [ ] Show objective-level project rollup and per-rig progress without treating
  incompatible footprints, sampling, bands or exposure purposes as equivalent.
- [ ] Define PSF Guard-owned per-rig catalog schemas and versioned access
  interfaces; remove TS-table assumptions from new Director code.
- [ ] Implement explicit TS import/sync connections in Settings/UI, with a
  bidirectional field/capability matrix, GUID mapping, preview/apply, conflict
  and unsupported-data reports. Preserve published Sync endpoint compatibility.
- [ ] Migrate copied real catalogs into native storage with backup/rollback,
  stable URLs/identity and grading/calibration/processing parity. Never mutate
  an external TS source schema in place.

Gate: one project references two rigs/catalogs with different FOVs and distinct
short/long objectives; identities survive catalog relocation and projections
rebuild without counting mirrored captures twice.

Schema-independence gate: a native per-rig catalog with no TS-shaped schema
supports the normal catalog/grading/calibration/processing workflows and a
Director contribution. Import then repeat bidirectional sync against supported
TS versions, proving idempotent supported-field transfer and explicit reports
for unsupported data. A same-name entity or relocated source cannot acquire a
new identity accidentally. An old Sync plugin still works through its documented
API; existing direct-TS catalogs continue to work during migration. This gate
is not satisfied by the separate meta database alone.

#### Confirmed catalog mappings

Schema 5 adds a one-to-one `catalog_rig` binding. The migration creates the table
but does not assign historical records. Operator preview/apply binds a registered
database, retaining one unambiguous prototype rig or creating a rig keyed by the
durable catalog UUID. It rejects ambiguous links without rewriting history.
Bound mappings must use that database's rig, and another independent catalog
cannot claim it. Copied/moved catalogs retain the same lineage and binding.

The earlier schema 4 added explicit source-profile-to-rig links and binds
each confirmed source project to that profile and a global project. A caller
first registers the stable catalog identity and creates or selects the global
project and rig. `link_catalog_project` then records both links in one writer
transaction. It never infers a rig from a database name, source row number or
project name, and never changes a source catalog.
`link_catalog_projects` applies up to 256 mappings in one transaction; a conflict
or storage failure rolls back the entire batch, including any earlier entries.

Profiles are scoped by catalog identity and retained as exact opaque source
IDs. Several profiles within the bound database share its rig, and several rig-local
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
identity across relocation/copies, and previews the proposed links. Database
settings use a read-only, paged mapping inventory and require explicit Apply.
Ambiguous historical frame ownership and contribution accounting still need a
separate workflow; these mappings alone do not resolve them.

### Phase 2: single-rig autonomous Director

- [x] Execute multiple targets locally within one admitted workload using
  shared-core priorities and fresh native geometry/progress. Preserve the
  seven-slot target lifecycle and verify an offline target switch in real NINA.
  Sequence-owned setup is implemented; automatic operation defaults are not.
- [x] Deliver target/periodic capture check-ins in a bounded background pump
  without gating ordinary local target transitions on network responses.
- [ ] After one-time commissioning, automatically refresh unchanged context,
  request eligible workloads, acknowledge bounded grants and run an enabled
  session without per-target or per-allocation operator clicks.
- [ ] Use the same resolved smart-filter, avoidance and priority policy in
  simulation and acquisition. Test inherited/off/zero values, mixed overrides,
  parent edits, multi-rig scope, hard-limit precedence and offline version parity.
- [x] Add the shared-library observing-preference preview: typed hierarchy and
  field provenance, seven editable weights/presets, score explanations,
  deterministic tie-breaking and bounded continuity. Geometry-bound preview
  retains hard limits.
- [x] Persist observing overrides and bind resolved policies into shared
  geometry execution and durable preparation. Weighted-program support remains
  for compatibility; the operator UI now uses global project ranking.
- [x] Replace project score editors with an ordered global project list and
  optional site/rig replacement orders. Compile precedence into shared-core
  priorities for offline execution; keep active grants immutable. Cross-grant
  corrections and score explanations for legacy programs remain open.
- [ ] Add [filter-specific Moon avoidance](#filter-specific-moon-avoidance-in-exposure-settings)
  to exposure template/recipe settings, preserve TS rules and enforce them in
  shared-core selection and native dispatch. Gate on offline and mixed-filter
  parity tests, including waits when every recipe is Moon-blocked.
  Explicit template policy, TS mapping, local lunar windows, capability gates
  and parked waits are the first increment. Inheritance, weighted preferences,
  detailed reasons and full phase-2 acceptance remain open.
- [x] Deliver the explicit-template Moon increment end to end: library editor,
  TS import/activation, immutable recipes, shared local lunar windows, v2
  capability gates and parked waits. Verified with NINA 3.3.0.1064 / OmniSim:
  priority-100 blocked recipe skipped, three eligible captures across two
  targets during server outage, parked native wait, reconnect delivery and
  replay refusal. Connected automatic intake/release also passed with enabled
  lunar rules. Inheritance, detailed diagnostics and deterministic Moon-rise
  during slow native setup remain part of the unchecked full acceptance above.
- [ ] Implement versioned allocation, acknowledgements, checkpoints, and limits.
- [ ] Renew or replace workloads through reconciled successor grants at safe
  boundaries. Carry forward pending/accepted/rejected credit, spent attempts,
  uncertain operations and outstanding offline reservations; do not reset them
  on a new check-in, process restart or lost response.
- [x] Add the ranked-priority increment: live automatic sessions finish the
  current operation, park, seal a settled partial ledger and request a fresh
  grant with the changed order. Preserve pending frames and spent attempts;
  ignore progress-only revisions. Arbitrary goal/grade/configuration changes
  and seamless mount continuity remain outside this increment.
- [x] Implement the local ledger's durable reservation/preparation/outbox
  primitives and crash/reopen tests; these do not include remote acknowledgement.
- [x] Implement separate scoped Director pairing, durable inspection history and
  capture-only checkpoint delivery with exact persisted cursors; validate real
  NINA simulator capture/restart/replay against an isolated server. This increment
  does not grant offline acquisition or integrate production background delivery.
- [ ] Add rig pairing, bounded cached offline authorization, remote inbox/outbox
  acknowledgement, retry/backpressure and safe revision activation.
- [ ] Add central rig telemetry ingestion and a permission-scoped live dashboard
  with current operation, elapsed time, progress, freshness and disconnected states.
- [ ] Add one resumable batch check-in path used by manual settings controls,
  sequence actions, periodic checkpoints and end-of-session reconciliation.
  Report counters/cursors and keep image uploads independent.
- [x] Deliver the capture-only increment of that path: live/deferred mode,
  settings check-in/cancel, native Check In action, previous-run delivery at
  session start, original-ledger reopening, resumable bounded pages and deferred
  clean workload release. No hardware replay or historical live-status replay.
  Grade/revision, timing and recovery journal reconciliation remain open above.
- [ ] Support the full native item/condition/trigger hook contract, including
  unsafe/recovery, nested waits, cancellation and cleanup; publish the tested
  compatibility matrix, including third-party safety actions.
- [ ] Ship capability-aware default operation policies through native N.I.N.A.
  without optional plugins. Expose policy ownership and prevent duplicate native,
  Director and plugin actions. Validate missing required devices/safety sources.
  Native center/AF/guide/dither/flip defaults, ownership checks, local chart and
  timed action history are now implemented. Keep this full gate open until
  rotation, real optical results and third-party combinations pass. Forced
  meridian crossing and resumed acquisition are simulator-validated above.
- [x] Add the internal shared-core recovery states and separate durable per-rig
  store: cooldown/hysteresis, cumulative probe/hold/failure budgets, persisted
  night-stop latches, duplicate refusal and restart tests. This is not a shipped
  quality-recovery feature.
- [x] Expose opt-in recovery contract 1 through runtime 0.9.0 / IPC 9, with
  one-shot issuance receipts, local journal pages, a separate per-rig lease and
  durable acquisition gating. Validate pipe restart and replay in real Windows
  processes.
- [x] Adopt runtime 0.9.0 / IPC 9 and a strict managed recovery client in plugin
  #40. Verify real-sidecar quality/stop/replay and existing full-stack acquisition
  with recovery disabled. This does not enable native recovery or Session controls.
- [x] Add an explicit local enclosure policy and independent fresh native dome
  clearance. Gate acquisition/park, stop slew/tracking on lost clearance and
  retain the interrupted owner's stop latch after reopening. Test actual shutter
  closure during an offline exposure and weather-unsafe open-air park separately.
  This is not yet the persistent night-stop or automatic recovery policy.
- [x] Expose saved per-Session abort park/stop choices with enclosure precedence,
  failed-park stop fallback and no automatic restart. Verify both choices in
  native offline NINA/ASCOM acquisition. See plugin #42; normal completion and
  planned waits keep separate behavior.
- [ ] Integrate [quality holds and session stop](#quality-holds-equipment-failures-and-session-stop)
  in the plugin through the versioned contract, local evidence classification,
  native bounded probes, Session controls and explicit resume authority.
- [ ] Connect the local enclosure-aware abort/park/stop owner to commissioned,
  persistent observing-night recovery. Roof closure and both local abort choices
  are tested; repeated guide/slew failures, failed-parking recovery and safety
  flapping across recovery sessions still need native offline validation.
- [ ] Prove explicit TS/Sync coexistence and the optional Chatstronomy adapter.
- [ ] Implement the shared-core operation state machine and the TS-style native
  container/options contract. Test configured trigger order and frequency,
  nested operations, cancellation, and failure propagation in real N.I.N.A.
  simulator sequences; report injected actions and durations back to the core.
- [ ] Enforce the current rig meridian/horizon snapshot at dispatch and refresh
  it on profile changes, horizon changes, and same-path file edits.
- [x] Add the active-operation constraint guard in the NINA plugin: read-only
  checks of the admitted site, meridian settings, native horizon and file hash
  during setup/capture; a changed, missing or stalled input cancels native work
  and uses the existing enclosure-aware abort policy. Restoring old settings
  cannot revive the consumed allocation. Dispatch still performs fresh shared-core
  feasibility checks. Automatically reviewing and commissioning changed context
  remains outside this increment; no new server or runtime interface is needed.

Gate: a real N.I.N.A. instance using simulated equipment handles slow autofocus,
failed centering, reprioritization, network loss, restart, and operator stop.
It never starts unauthorized work and reports ambiguous capture outcomes.
It does not start an exposure across a meridian exclusion or below the effective
local horizon, including after unexpectedly slow setup operations.
Run the same complete session with default policies and no optional plugins,
then with native and compatible plugin hooks (including unsafe/recovery). Test
safety changes while a hook or nested wait runs and during cleanup. A slow hook
causes fresh feasibility evaluation, not a duplicated action or stale exposure.
After initial setup, submit competing active workloads and run the complete
session without manual target selection or allocation admission. Verify that
server and sidecar resolve the same policy and priorities, respond to changed
conditions, and advance to a reconciled successor without double reservation
or credit. A lost workload reply must not create a second grant; no eligible
work means a visible bounded wait/check-in, not a fabricated capture or busy loop.

Telemetry/offline gate: watch the rig in central PSF Guard, stop the server,
continue within an already cached offline allocation, and restart the plugin and
server. Then deliver a large backlog in bounded batches with dropped replies,
duplicate events, cancellation and resumed cursors. The central view becomes
stale during the outage and correct on reconnect; it does not show historic
backfilled operations as current. Pending counts, grade revisions, timing data
and attempt limits reconcile without double credit. Cached expiry, required
evidence loss and storage pressure stop new work visibly. Repeat with deliberate
end-of-night check-in and deferred image upload. No server availability or image
copy is required for local safety or an already authorized offline session.

### Phase 3: timing-aware shared simulation

- [x] Record correlated monotonic preparation/capture durations in local evidence.
- [ ] Complete all operation/hook instrumentation and central delivery; learn
  contextual duration distributions with versioned, capability-aware defaults.
- [ ] Add virtual-clock simulation, decision explanations, and replay fixtures.
- [ ] Display uncertainty, constraints, and predicted versus actual progress.
- [ ] Feed the framing wizard's per-rig/site/night preview through this same
  engine, including inherited policy, setup freshness and existing allocations.

Gate: timing observations affect both hosts consistently; nested durations are
not double-counted and slow operations cause sensible goal reevaluation rather
than timetable catch-up. Live tests from phase 2 become replay regressions.

### Phase 4: quality and calibration feedback

- [x] Reserve local attempts and keep saved captures pending rather than accepted.
- [ ] Reconcile central pending/accepted/rejected assessment revisions and bounded
  reacquisition, including delayed batch delivery and late image availability.
- [ ] Request replacement calibration when coverage is invalidated.
- [ ] Connect project readiness and processing provenance to existing workflows.

Gate: delayed uploads and grading do not cause duplicate or unlimited work;
rejected lights and invalid flats reopen only the appropriate deficits.

### Phase 5: coordinated multi-rig acquisition

- [ ] Allocate compatible contributions across rigs and sites.
- [ ] Evaluate horizons, coverage, sampling, priorities, and separate stack groups.
- [ ] Add assignment handoff and project-level allocation/progress views.
- [ ] Complete the framing wizard's combined coverage, multi-site feasibility,
  reviewed activation and grade-driven revision workflow.

Gate: two rigs with different FOVs and horizons advance one project; loss of
contact with one does not authorize duplicate outstanding work on the other.
Create that project through the wizard using rigs at two sites with different
visibility/local nights: one wide-field contribution and a narrow-field mosaic,
with distinct short/long purposes. Verify panel rotation/overlap and sky geometry
near RA wrap and high declination, setup changes, missing optical data, manual
rotation, draft reload/back navigation and stale-review rejection. Exercise
cancel/save without acquisition, activate after review, run native simulated
capture, grade, and reopen the same coverage view with no duplicate credit.
The single-rig wizard must also work without sites/rigs beyond its one setup.
Survey gates: switch DSS2 Color and H-alpha without moving either rig's framing;
verify projection, orientation, RA-wrap/polar overlays and partial coverage with
deterministic fixtures. Test loading, cancellation, failed providers and offline
cache misses on desktop/mobile and in Tauri. Real-provider smoke checks supplement
fixtures but are not required network dependencies of CI or acquisition.

### Phase 6: remote instances and collaboration (deferred)

Future revision only. Keep the collaboration constraints above as design notes;
do not implement coordinator/participant APIs, invitations, cross-instance
contribution exchange or collaborative permissions in the current work. Define
its acceptance gate when that work is explicitly requested. Deferral does not
block single-coordinator planning and acquisition across multiple rigs/sites.

### Later: moving targets (design note)

A target can be a comet or an asteroid: a body on an orbit, not a fixed
place. Nothing in the current model allows for that, and the framing view
already draws them from the Seiza minor-body catalog, so the gap shows. When
this work is taken up, the target model needs, at least:

- an orbital identity (designation and the orbital elements' epoch) beside
  or instead of fixed ICRS coordinates, with the position computed for the
  night, hour and site being planned, the way the marks are placed today;
- tracking requirements the acquisition side can carry into N.I.N.A.:
  the body's rate and direction of motion at exposure time, whether to track
  the body or the stars, and exposure lengths bounded by the motion across a
  pixel, so a fast mover does not trail;
- per-night re-framing: the rectangle and any mosaic are re-solved against
  the body's position at the planned time, and the Target Scheduler target
  row is updated before each night rather than written once at activation;
- feasibility that follows the body: altitude, Moon separation and horizon
  checks at the moving position, plus the body's brightness from the
  catalog's magnitude model, so a fading comet drops out of the plan;
- provenance and quality: solves and stacks tagged with the body and the
  ephemeris used, so a later, better orbit does not silently move history.

Until then a comet is framed as a fixed place at the moment it was looked
at, and the framing view says so in its marks.

### One list, two groups, Live on the Sky

Decided 2026-09-29 and built in five steps by 2026-09-30 (#594–#598); the
user guides describe the result. The Library and the plan list
now render from the same rows, and the header's Library, Images and
Sequence tabs were three views of one thing while Planning was a second
list with a workspace attached. The unit of review is one rig's project,
because frames, grades, caches and sequence scores live in one catalog; the
unit of planning is the family, one plan shot by several rigs. The header
should say so instead of hiding it:

- **Library** stands alone: the one list of projects and families, with
  the Show select, search and archive fold the plan list grew. The plan
  list under Planning goes away; `/director` redirects into the Library.
- **Review** is a labeled group: the project picker, then **Images** and
  **Sequence**. The picker keeps grouping a family under one heading with a
  row per rig, so both views stay per rig without a seam. A family heading
  becomes selectable only once Grid and Sequence can span a family (below).
  As built on 2026-09-30 the Review and Plan groups became one scope: the
  project picker, Workspace, then a rig and target switcher for Images and
  Sequence, superset to subset, left to right.
- **Plan** was a labeled group, shown when Director is enabled (since merged
  into the one scope above): a plan picker over families, then the **Workspace** (framing, sky, feasibility,
  exposure plan, rigs, activation, attach and detach) at `/plan?plan=<key>`,
  where the key is the Target Scheduler GUID the rigs share (the same on
  every instance holding those databases), else the plan id, or
  `slug:row` for a row not yet planned. Old `directorProject` links
  forward there. Each picker offers the hop to the other group.
- **Live** has no scope of its own, so it is not a tab. It folds under
  **Sky**: the coverage map already reads every catalog and shows the past,
  and it gains the present, each rig drawn where it points now with its
  phase and target, and today's dashboard as a panel beside the map. A
  header status chip (`2 rigs · 1 exposing`, red when one has gone quiet)
  sits in the existing jobs slot on every view and opens Sky's live panel.
  The Library's family rows and the Workspace's rig list were to show their
  slice of the same status inline; that is not built yet.
- **Rig setup** and **Exposure templates** move under Settings. They are
  configuration, not daily work.

Grid, Detail, Comparison and Sequence keep the `db` slug in URL state; the
Library keeps merging databases through `useScopedDbId`; shared links keep
working through redirects.

As built, against this note: the Library lists only projects with frames,
so plans that have captured nothing wait in their own Library section with
New plan and rename; the workspace lives at `/plan?plan=<key>`, the key
being the Target Scheduler GUID the rigs share or the plan id when a GUID
is not unique (a detach keeps the GUID); the Live chip opens the Sky with a
rig list beside the map and the table under it, read-only viewers getting
rigs and templates there; and placing a rig needed the new optional
`pointing` status field (see the plugin handoff below). The workspace
page is headed "Planning" and is reached from the Library, the
header and old links.

### Plugin handoff after the navigation change

The navigation work ("One list, two groups, Live on the Sky", September 2026)
changed no route under `/api/director/v1`; the plugin's program pull,
check-in, status and pairing calls are as before. What the plugin work
should now take into account:

- **Status payload.** `POST /api/director/v1/rigs/{rig}/status` takes the
  envelope `coordinator_instance_id`, `catalog_id`, `session_id` (new for
  each acquisition session), `reported_at_ms`, optional `program_revision`,
  and `status`, which must be a JSON object; `accepted: false` in the reply
  means a newer report for that session, or a newer session, is already
  held. See the route table and payload paragraph in
  [DIRECTOR.md](../DIRECTOR.md#check-in-and-live-status). Inside `status`
  the Live table reads `phase` (or `state`), `target_name` (or `target`),
  `operation` with `operation_started_ms`, `wait_reason`, `safety`,
  `queue_depth` and `errors` (or `error`); the Sky adds `pointing: {
  ra_degrees, dec_degrees }` in J2000 degrees (RA 0–360, converted by the
  mount's `EquatorialSystem`), sent whenever the mount is connected.
  `phase` counts as exposing only as `exposing`, `imaging` or `capturing`.
  `target_name` is the Target Scheduler target row's name, one per mosaic
  panel; without `pointing` the Sky infers the rig's place from it and
  marks the place as inferred. A report over ten minutes old reads "old
  report" wherever it shows.
- **Deep links.** A plan's workspace is `/#/plan?plan=<key>` on the PSF
  Guard server the plugin paired with, where the key is the Target
  Scheduler project GUID when all rigs share it (true for projects Sync
  copied); the match ignores case. A plugin that wants an "Open in PSF
  Guard" link can build it from the project GUID it already holds; the page
  also resolves any rig's GUID, the plan id, or `<slug>:<project row>`, and
  rewrites the address to the plan's own key. If a detach has left two
  plans holding the GUID, the page lists both and asks which. The Library
  is `/#/`, and Live on the Sky is `/#/sky?live=1`.
- **Rig identity.** Unchanged: a rig is one registered database with its
  `psf_guard_catalog_identity`; the status and check-in tuple still carries
  `coordinator_instance_id` and `catalog_id`, and a mismatch is refused.
- **Connectivity.** The header's Live chip counts rigs from `GET
  /rigs/status`; a rig that stops calling turns the chip red after three
  minutes (`stale`) and reads offline after thirty. Any program pull,
  check-in or status report counts as contact, so a plugin that only
  reports status still reads online.

### Later: a mosaic's targets as one view (design note)

Asked for 2026-09-30, not yet. Today a mosaic is several Target Scheduler
targets, one per panel, and Images and Sequence show one target or all of a
project's targets as separate frames. A meta-target mode would treat a
mosaic's panels as one target: the grid grouped by panel in the mosaic's
layout, Sequence interleaving the panels by capture time, and the stack
previews placed side by side on the framing's sky. It belongs in the rig and
target switcher as one more choice ("M31 mosaic"), beside "All targets" and
the single panels, and it needs the panel layout from the plan's framing
draft rather than guessing it from target names.

### Later: review across a multi-rig project (design note)

Grid and Sequence should one day take a family as scope: frames from every
rig that shoots the plan, interleaved by time in Sequence and grouped by
rig in the grid, with Detail and Comparison reached from either. That
means fan-out queries across member catalogs, the `db` slug carried per
frame instead of per view, family-keyed URL state, and caches that stay
scoped below each rig's slug. Grading still writes per frame, so it fits.
Quality scores, comparison and stacking stay per rig, since optics and
pixel scale differ. Until then the project picker's family grouping keeps
review per rig, and the family heading in it is not selectable.

### Later: settings that sync between PSF Guard instances (design note)

A user runs more than one PSF Guard: a desktop for review and an always-on
server the rigs report to. Some of what they hold is instance-local (paths,
caches, registry) and some is shared knowledge that should meet in the
middle: the exposure template library, rig setup (optics, site, limits),
and in time plans and framing drafts. Today each instance keeps its own and
the user copies by hand. When this is taken up, alongside Phase 6:

- every shared record is keyed by a stable GUID and carries a revision, as
  the template library and Director identities already do, so two copies
  can be told apart from two edits;
- sync is a compare-and-set exchange of records, not a file copy, over the
  same HTTP the plugin already pulls programs through, with the last writer
  named and a conflict surfaced rather than merged silently;
- each record type says which fields travel and which stay local (a rig's
  site travels; its catalog path does not), and activation never follows a
  synced draft without the user applying it on the receiving side;
- the Settings pages that own these records show where a record came from
  and when it last agreed with the other instance.

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
- Run the native-default session without optional sequencing/safety plugins,
  then add native and third-party Advanced Sequencer hooks. Verify startup,
  target/exposure hooks, conditions, unsafe/recovery, cancellation, nested
  waits and cleanup. Missing extensions or unsupported contexts must fail
  validation visibly. Inspect actual hook ordering, cadence and timings.
- Watch the session in the central rig dashboard, including elapsed operations,
  errors and stale/offline state. Stop the server and finish an authorized
  offline block. Reconnect through manual and sequencer batch check-in, drop an
  acknowledgement mid-batch and restart both ends. Confirm resumable cursors,
  assessment reconciliation and bounded new allocation with no double credit.
  Repeat with unavailable remote image files and deferred end-of-night upload.
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
