# Director management

Director is experimental. It has no page of its own: plans live in the
**Library** beside the projects they shoot, each plan has a **workspace**
reached from the Library or from the header's scope, and the rigs
live on the **Sky** and in **Settings**. Director stays the name of the
plugin, the protocol and the API. This API manages global
project, site and rig identities in a separate meta database. Director clients can pair to inspect programs and report
receipts/status. Pairing does not allocate work or enable acquisition. The NINA
runtime preview and PSF Guard Sync remain separate.

## Collaboration Sign-In

PSF Guard owns the collaboration loop. Director receives ordinary rig plans and
reports execution; PSF Guard Sync transports database changes and images. Neither
plugin needs a direct Starfront connection. The independent
[Starfront TargetScheduler Collab plugin](https://github.com/theatrus/starfront-targetscheduler-collab)
provides the outer loop for Target Scheduler users without PSF Guard. Use one
collaboration host for each remote agent to avoid duplicate assignments or reports.

Open **Settings**, **Rigs**, then **Setup** for a rig. The selected rig's header
stays above its setup. **Optics**, **Site**, **Limits and delivery**, and
**Collaboration** use a section rail on desktop and a compact strip on narrow
screens, separate from the smaller collaboration tabs. Changes stay in
the form when you switch tabs; choose **Save rig profile** to save them.

In **Collaboration**, choose **Connect server**. The setup wizard has three
steps: **Server**, **Sign in**, and **Capture**. Enter the server's HTTPS URL
and remote rig name, then continue. PSF Guard offers browser sign-in or a pairing
code according to the server's capabilities.
For browser sign-in, open its approval page, approve there, then choose
**Check sign-in**. Registration does not join a project or enable acquisition;
configure this rig's filters, exposure lengths, binning and colour mode, then
choose **Finish setup**. You can close the wizard and resume with
**Continue setup** without enrolling a second agent. Optics come from the rig's
commissioned setup, not a second telescope profile.

Select a connected server to open its **Tonight**, **Automation**, **Capture**,
**Reports**, or **Connection** tab. Switching tabs or servers keeps capture
edits. Pairing leaves background work, activation and activity sharing off.

In **Tonight**, choose **Browse projects** to see compatible projects, then **Join** a project
or **Pull tonight's work**. Both use the rig's saved site to calculate its
noon-to-noon observing night and Moon context. **Night override** accepts a
different date without asking for Moon values. Set the observing site in rig
setup first. Joining gives consent
to a remote share; it does not start a sequence. **Review import** shows the
panel/filter visits. **Import draft** creates an ordinary inactive project
draft. Apply refetches the remote work and refuses a changed review.

In **Automation**, choose explicitly allowed joined or imported projects, then
enable **Pull tonight automatically**. The database follows the rig; there is
no database picker. Work is pulled once per rig-local observing night, not on a
15-minute timer. A durable checkpoint survives server restarts. Failed requests
back off and retry. A database lock retries the saved assignment locally rather
than fetching it again or skipping the night. **Refresh now** explicitly reruns
the saved automation,
including reports when enabled. It never joins projects for you.
**Activate in rig database** also applies safe assignments to that one database.
For a new untouched draft, Director uses a unique matching filter/binning
template, preferring the assigned exposure when several templates match. It
preserves the template's gain, offset and Moon policy. Ambiguous recipes need
review in Planning; edited or disabled recipes are never reconstructed. A new
assigned filter without a saved plan recipe also waits for Planning.

Automatic activation does not start NINA or push the entire database to a remote
peer. It also cannot expand a Director client's separately approved workload
scope. Use normal database sync for a separate telescope-side copy. Existing
plans remain available during outages or when a response omits an assignment.
The setup shows last success, next refresh, errors and held assignments. Failures
back off; reconnect after a rejected credential. A server restart restores the
saved policy and nightly checkpoint; detailed run counts are process-local.

Open that project's plan workspace, add one enabled rig recipe for each assigned
filter at its assigned exposure length, then select the rig's assignment under
**Activation**. Preview and Apply use the assignment's exact panels, camera
angles and frame counts, not an edited rectangular mosaic. This writes ordinary
Target Scheduler projects and exposure plans; use the existing database sync to
send them to NINA. Local priorities, scheduling limits and grading retries
remain in effect. Remote quality requirements govern contribution credit, not
permission for TS to run an exposure.

An assignment is not a deadline. Target Scheduler may finish it on later nights;
there is no extra nightly guard or forced stop. Reapplying it does not replenish
its quota. Activating reviewed newer work supersedes the old assignment, reuses
unchanged targets, and preserves captures and grades. Its new goal starts above
the accepted count when grading is enabled, or acquired count otherwise.

Enable **Submit contribution reports automatically** to queue and deliver eligible
contributions without an open browser. This separate opt-in checks for ready
images every five minutes and replays the durable outbox after outages, without
pulling tonight's work again or sharing live activity. It only uses activated
assignment provenance in the rig's database, groups frames by actual rig-local
capture night, revision, panel and filter, and applies the same evidence checks
as manual review. Missing files wait for arrival. Changed or incomplete cohorts
remain held; automatic reporting never invents missing measurements or changes
grades. Existing reports are not withdrawn or rewritten automatically.
Status separates delivery from acceptance. Rejected contributions and the
server's reasons stay visible across later idle passes and server restarts.

In **Reports**, the database is selected from the rig automatically. Select an
imported visit, its remote panel, and accepted images. The open image list refreshes
every 30 seconds and after catalog imports. Missing files appear only after they
arrive in a configured image folder. Newly arrived frames are not selected or
submitted by the manual view. **Review images** checks
GUIDs, saved files, exposures, fresh pixel solves and shared measured coverage.
Set **Observing night** to the night the selected images were captured, even if
the assignment was issued earlier. Candidates include a time-zone margin, so
review which images belong to that night. Activated targets associate captures
by stable target GUID, filter and assignment cutover time; delayed database sync
does not change their original task or revision. **Queue finalized contribution**
freezes the reviewed evidence. The review shows exposure, scale, focal length,
HFR, guiding, Moon, bandpass and camera data; unavailable measurements read
**Unknown**. If a selected image is no longer accepted, the refreshed view
withdraws that selection and its review. **Check in** delivers a bounded batch and keeps
failed or partially acknowledged batches for replay. A remote rejection is
recorded separately and never changes local image grades. Regrading or
withdrawing previously reported frames requires review; the public protocol
cannot replace a report with an equal or lower integration total. The same
saved image cannot be credited under two nights. Reports from mixed assignment
revisions require review rather than silently replacing an earlier aggregate.
Recorded exposure durations may differ from the assigned recipe by at most
0.1%, with a 10 ms floor and one-second cap, to tolerate shutter timing. Reports
use the actual saved durations and their mean, never planned integration.

**Share current activity** is off by default. When enabled, incoming Director
status forwards fresh activity and queued reports at most once per minute.
Target names and pointing coordinates remain private. Old or replayed status
never becomes current presence. Offline scientific reports retain their
original agent, task, panel and observing night.

Reports use a conservative common solved footprint. Frames without a current
pixel solve, disjoint footprints and unresolved calibration requirements are
held. Calibration must be recorded by the file's processing tool; a matching
master or a calibrated-looking filename is not proof. Director saves measured
guiding RMS in arcseconds (`PGGRMS`) and HFR with explicit pixel units (`PGHFR`)
in FITS and XISF headers. Pixel HFR is converted using that frame's solved
scale; raw-frame HFR is not reused for a resampled derivative. Missing guiding
data stays unknown, not zero. Bandpass comes from an explicit saved `PGBAND`
value in nm or a single-band filter label such as `Ha 3nm`, never the rig's
current configuration or the remote requirement.

Moon illumination uses the saved exposure midpoint. Separation uses the fresh
solved center and saved observing site, including lunar parallax. Missing site
coordinates leave separation unknown. These are planning-grade calculations,
not precision astrometry. An explicitly invalid saved timestamp is not replaced
with the current time.

For Director images, `PGCAPID` must match one acknowledged saved capture
receipt for this rig and its activated exposure goal. Images may arrive before
or after batch check-in, but review waits for both. Missing, malformed or
ambiguous receipts hold the report. The link never changes the catalog image
GUID used by Sync. Target Scheduler images without `PGCAPID` keep the existing
stable-GUID evidence path.
The reservation must also belong to that assignment's cutover interval. A
capture straddling an assignment change is held if its catalog timestamp and
receipt cannot resolve the same assignment; it is never relabeled as new work.

The server stores the agent token in `collaboration-credentials.json` beside
`config.json`. A custom registry uses
`<registry-stem>.collaboration-credentials.json` instead. Unix files use mode
`0600`; Windows files use a user-only ACL. The config directory must not be
writable by other ordinary users. Docker uses its existing writable config
volume and service UID, with no separate vault or unlock secret. Tokens are
plaintext on disk: restrict backups and do not include this file in diagnostic
bundles or shared configuration exports.

**Check connection** rereads the saved credential and checks the remote server.
If the credential file is missing, unreadable or the server rejects its token,
the original agent identity and queued reports remain intact. The current
AstroCollab protocol cannot reissue that same agent's token. Recover it through
the server when possible, or explicitly add a new connection; reports belonging
to the old agent are never reassigned to the new one. **Disconnect** removes the
local token but retains its identity and work. It does not revoke the remote
agent. HTTPS is required except for explicitly enabled loopback HTTP tests.

## Project Priority

Open a plan workspace and unfold **Project priority**, which says where the
plan stands in the global order while folded, to order projects with the
up/down buttons. Save the **Global order** once for all rigs. A **Site override**
or **Rig override** can replace that list; check **Use inherited order** to return
to the parent order. A rig's **Planning site** supplies site inheritance without
changing the native location, horizon or safety monitor.

The highest eligible project runs first. Completed work, blocked filters,
unsafe conditions, horizon and meridian limits still prevent acquisition.
Objective priorities choose work within the selected project; they cannot move
a lower-ranked project ahead. New projects follow the saved list in name/ID
order until you rank them. The list supports up to 1024 projects, as many as
the plan list shows.

Saving replaces the old score policy for newly issued programs, not active
allocations. Without a saved order, existing scheduling behavior is preserved.
Issued programs carry the order as shared-core priorities, so NINA follows it
offline without needing a server request between exposures. Director sessions
with Automatic workloads and Live check-in detect changed priorities for the
same goals, finish the current exposure, park and reconcile, then request a
new authorized program. Manual/deferred sessions retain their original order.
The handoff preserves pending frames and spent attempts; it cannot resume the
old grant if the connection fails during release. The UI no
longer exposes per-project importance, weights, presets or switching scores.

## Scheduling Limits

A plan carries Target Scheduler's per-project scheduling limits: minimum time,
minimum and maximum altitude, custom horizon and its offset, meridian window,
filter switch frequency, dither interval and smart exposure order. Set the defaults once on
a plan's **Priority and defaults** tab, for every plan or a site (pick the
scope at the top). A rig's own are in its **Setup**, under **Limits and
delivery**, beside the planning limits Director plans with. Set one plan's own
on its **Rigs** tab under
**Scheduling limits for every rig**, where a table shows what each rig gets and where each
value comes from. An empty field inherits and says from where. Both save from
the page's save bar. Over the API they are the `scheduling` key of
`PUT /preferences/{scope}/{id}` for the **global**, **site**, **rig** or
**project** scope. The nearest scope that sets a limit wins
(project, then rig, then the rig's site, then global); one nobody sets keeps
Target Scheduler's default. `GET /rigs/{rig}/preferences?project_id=...`
returns the resolved values in `scheduling.values` and, in
`scheduling.sources`, the scope each came from.

Activation writes the resolved limits into each rig's Target Scheduler
project, so they hold when Target Scheduler runs the rig without Director. A
project the activation creates takes every limit; an existing one takes only
the limits some scope sets, so a value edited by hand in Target Scheduler that
no scope plans is left alone. The preview lists each change as
`<project> · scheduling limits`, for example
`minimum altitude 10° → 30°`. Columns an older Target Scheduler schema lacks
are skipped.

### A rig's limits for every project

A rig's limits also reach the projects no plan holds, such as ones made in
N.I.N.A. Below the fields, **Target Scheduler projects** compares the saved
limits with every project in the rig's database, closed ones included, and
lists each one that differs with what would change. **Apply** writes them.
Each project takes what an activation would write: a plan's own limits for the
project it holds, a collaboration assignment's altitude floor, and the rig's
for the rest. The rig's value replaces one changed by hand in Target
Scheduler; a limit no scope sets stays as each project has it, and the list
names those. Apply is refused when a project or the limits changed since the
list was read, and nothing is written; the list then reloads. A rig whose
database lives on a Sync peer sends the rows there afterwards, as activation
does. Nothing applies by itself: a project made later waits for the next
Apply. Over the API, `GET /rigs/{rig}/scheduling` returns the comparison and
its `digest`, and `POST /rigs/{rig}/scheduling/apply` with that `digest`
writes it. Apply needs the server's database-management permission.

## Exposure Moon Rules

Expand **Moon** in the exposure template library to enable per-filter avoidance.
The stored policy includes full-Moon separation (degrees), half-width (whole
days, 1 to 14), Moon-down-only eligibility and altitude relaxation. Disabling
avoidance preserves its saved values. Imported TS templates retain their rules;
activation writes the rules into rig templates. Changing a shared template's
rules creates a separate rig template instead of changing other plans.

The shared engine excludes blocked recipes before choosing a target. Objective
priority wins among eligible work; equal-priority work prefers the more
Moon-sensitive recipe. When only lunar restrictions block the remaining work,
the core returns `moon_avoidance`; the NINA session waits and, with Park on Wait,
parks. The immutable grant carries these rules for disconnected execution.
Automatic work requests need `prepared_target_v2` or `local_sequence_v2` for
enabled lunar policies. Older modes are refused, including request retries.

The formula follows Target Scheduler's `MoonAvoidanceExpert` at
`17b36a4f8580c687ad18f8b94127d7ca1a2a702e`: Lorentzian separation versus days from
full Moon, linear altitude relaxation of separation and width, and the
configured upper relaxation altitude as the Moon-down boundary (not necessarily
zero degrees). Missing legacy columns use TS defaults: off, 60 degrees, 7 days,
zero relaxation, -15/5 degree bounds and Moon-down off. Invalid saved rules are
reported instead of silently ignored. A Target Scheduler template with avoidance
on and rules outside these ranges is left out of the rig's template list and of
imported plans, with a warning that names it; the rig's other templates work as
before. With avoidance off, Target Scheduler ignores the other Moon fields, and
so does Director: values outside the ranges then read as the defaults.

Execution intersects the allocated windows, horizon/meridian limits and lunar
windows for the entire preparation/exposure interval. Minute cells use shared
ephemerides with a conservative parallax/model/motion margin and a half-day
phase guard. These margins can defer work near a boundary. This is not precise
lunar occultation planning. Project order inherits globally with site/rig
overrides; inherited exposure Moon rules and detailed lunar telemetry remain
separate work.

## Where the store lives

Director is on for every server. The meta store is a separate SQLite file
beside the database registry: `director-meta.sqlite` next to `config.json`,
or `<registry>.director-meta.sqlite` for a registry with another name, so
`--registry /tmp/psf-guard-test.json` gets its own store. The desktop app uses
the same default. A server without `--allow-database-management` plans
read-only over its catalogs: plans, framing and rig profiles can be drafted in
the meta store, but activation, pushes and Target Scheduler edits are refused
with `403`, and no table is written into a rig database. Such a catalog is
planned under a derived identity (fixed by this instance and the file's path);
the first managing server to list it writes that same identity into the file,
so the rig does not change. `GET /status` reports `database_management` so the
page can say which kind of server it is on.

```text
psf-guard server --host 127.0.0.1 --registry test-registry.json --allow-database-management
psf-guard server --host 127.0.0.1 --registry test-registry.json --allow-database-management --director-meta /srv/psf-guard/director-meta.sqlite
```

`--director-meta` keeps the store at another path. The parent directory must
exist. A missing file is created atomically; an existing file must be a
recognized Director meta store. Catalogs, empty files, and unrelated SQLite
databases are not adopted. The normal server startup policy still requires
accounts for database management on a network bind unless the operator
explicitly trusts anonymous access. Prefer accounts; see
[authentication](AUTHENTICATION.md).

Back up the meta store with its SQLite-aware storage API before an upgrade.
Do not copy a live SQLite main file without its WAL. Stop older coordinator
processes before opening an upgraded store. HTTP backup, restore and path
switching are not exposed. Catalog adoption requires the explicit workflow below.

## Management screen

Plans live in the Library and the header; there is no separate list page.
The **Library** lists every project that has frames, and a plan shot by
several rigs is an outer pill there with a row per rig. The outer pill
carries the plan's stage (Framed, Planned, or Active on N rigs) and a
**Planning** button (⚙ on a compact row) that opens the plan's **workspace**; a project shot by one
rig has that button on its own row and card, and the rows and cards inside
a family leave it to the outer pill. A state change locks that row's select
until the database has it. Below the projects, **Plans
with nothing captured yet** lists what the Library has no row for: plans
framed before any rig takes them, and plans activated on rigs that have not
captured a frame. Each shows its rigs and their project states, or that no
database takes it yet; **New plan** starts one and the pencil renames one.
A plan leaves that list for the projects above once its first frame arrives.
A plan closed on every rig stays out of it until Show asks for Closed, and
when a database's projects cannot be read, plans linked to databases are
left out rather than shown as empty.

The header reads Library, then the **scope**, then Sky. The scope runs from
the whole to the part: the project picker (a plan shot by several rigs is
one row there, and the closed picker names it with its rig count), the
**Planning** button for that plan's workspace, then a switcher for which rig and
target **Images** and **Sequence** show. For a plan shot by several rigs
the switcher lists each rig with its targets; for a lone rig it lists the
project's targets (mosaic panels by the part that sets them apart, the
full name as a tooltip), and with one target it is a label, "Rig · target"
over the database and target, set apart from the buttons since there is
nothing to choose. Used on
a workspace, the picker opens the chosen project's workspace; it also lists
plans no database takes yet, which open straight into theirs. Every view
button shows the page you are on the same way, filled. Beside the jobs slot a
**Live** chip counts the fleet, `2 rigs · 1 exposing`, turns red when a rig
that used to talk has gone quiet, and opens **Live on the Sky** from any
view: each reporting rig drawn on the coverage map where it points now, a
list of the rigs beside the map (choosing one turns the map to it), and the
full Live table under the map. Rig setup and the exposure template library
are under **Settings › Rigs** and **Settings › Exposure templates**; a
read-only viewer cannot open Settings, so Live on the Sky shows them both
under the table, read only. Choosing a
project in the Review picker from the Library, Sky or a workspace opens
Images for it, and the workspace's own URL parameter (`plan`) stays behind, so a
plan you had open does not follow you into review.

A workspace lives at `/plan?plan=<key>`. The key is the Target Scheduler
GUID the plan's rigs share, so the same link opens the same plan on any
PSF Guard instance that holds those databases. Director's own plan id names
a plan instead when the GUID cannot: a plan with no database, one whose
rigs carry different GUIDs after an attach, or one whose GUID another plan
also holds, since a detached project keeps its GUID. Any rig's GUID still
finds its plan, and `plan=<database>:<row>` finds the plan of a project row
or says why the row has none yet (a project without a GUID cannot be
planned). Whatever the address, the page rewrites it to the plan's own key,
so history and bookmarks stay right. The workspace stays open through that
rewrite, unsaved edits and all, while opening another plan with edits
unsaved asks first, as leaving the page does. A workspace open when a detach makes
its GUID ambiguous stays on its plan; a fresh visit to such a GUID lists
the plans that hold it. An address that names nothing says so and links
back to the Library. Opening a plan from a Library row carries that row's
database, so its targets and exposures editor is open on arrival. Old `/director` links forward: `directorProject` to its plan,
`directorSource` with `project` to that row, and anything else to the
Library with its scope, `directorShow` and `directorSearch` becoming the
Library's `show` and `q`.

**Live** is the operator's table of rigs. For each rig it shows the link
state, from the server's own receipt times of the plugin's calls: *Online*
within three minutes of any call, *Quiet* within thirty, *Offline* beyond,
*Never seen* before the first. Then what the rig says it is doing, from its
last status report (phase, target, current operation and how long, wait
reason, safety, queue depth, and any errors it named); a report older than ten
minutes is marked stale rather than shown as the present. The next columns
give how long ago the rig last pulled its program (with the revision), checked
in (with saved frames grading has not yet accepted), and reported status, and
which activated plans it is assigned to, each a link to the workspace. The
table refreshes every fifteen seconds.

**Settings › Rigs** lists every registered database as a rig: whether
planning is enabled, its field of view and pixel scale from the rig profile,
whether the plugin has reported its camera, and the plugin's last live
status. **Setup** expands the database's planning setup in place: enable
planning, the rig profile, and the project links. **Library** opens its
catalog.

The **workspace** for one plan has a summary at the top and four tabs. It
opens on **Framing**, which places the target, unless the address names
another tab. **Exposures** holds the objectives and, for
each rig shooting the plan, its template, exposure and frames per objective.
**Rigs** lists each rig with its database's project and whether activation
has reached it. **Add a rig** offers every rig with a new project, which
activation creates in its database, and every project another plan holds in
a database this plan has none in. A rig joins with a template for each
objective, from its database or the library; one with no template for any
objective's band says so and stays out. A library template the database
already holds, written there by activation or with the same filter, camera
settings and Moon rules, shows as the database's own and is not offered
again. **Drop from plan** switches a rig's
exposures off: activation then sets its Target Scheduler project inactive,
and adding the rig back turns the same exposures on again. **Detach** gives
a database's project a plan of its own. Once a rig's
database has the project, its Target Scheduler settings and targets open
there; before that the rig says activation creates them. The plan's
scheduling limits follow the rigs. **Priority and defaults** holds the
project priority and the scheduling defaults.

Edits wait for the save bar. A copy saved elsewhere, or a plan an attach
brings in, never replaces unsaved edits: the tab says the plan changed and
offers **Reload**, and the save bar will not save over it.

Activation is no tab. Once every edit is saved, a bar at the top asks for an
activation while the rig databases lack the saved plan or framing, or a rig
that shoots the plan has none of it (the last activation skipped it, or it
joined since), and **Activate…** opens the preview and apply over the page.
The summary's activation line, **On the rigs** when nothing is missing,
opens the same dialog at any time, to push the last activation again. On a
server that does not write rig databases the summary says **Drafts only**
and no bar asks. Older links naming the Plan, Rig databases or Activate tabs
open Exposures or Rigs. A plan with no linked database yet starts its framing
by looking a name up in the CDS catalogs or by typing a center; activation
then creates and links the Target Scheduler project in each rig database.
Its **Library** link goes back with the scope it came from. A project
Director has not taken in yet opens a page that finds or starts its plan.

Sites are no longer edited as their own records; a rig's site lives in its
rig profile. The identity API for sites remains for the plugin.

### Rigs and plans appear on their own

Opening the Library, or any view with the header's scope, takes every
registered database in as a rig and every Target Scheduler project in it as
a plan. Nothing to enable, name or
link: the database keeps a small identity table so a moved or renamed file
stays the same rig, and each project row with a GUID gets a plan named after
it. Projects that share a GUID across databases, as Sync copies do, become
one plan with several rigs; projects that merely share a name stay separate
plans. The first time a project is listed, what Target Scheduler already
holds for it becomes Director's own drafts: its targets become the framing
(center and rotation from the rows, a grid of panels when the targets form
one, the rig's field as the panel size when its optics are known) and its
exposure plans become the plan (one objective per bandpass with the frames
a panel wants, bound to the template each plan uses at its exposure).
Director plans one target or one mosaic per project. A project whose targets
are separate, not panels of one grid, is drafted as its first target with
that target's own exposure plans, and the listing says so; Target Scheduler
keeps running the others. Activation leaves a rig to Target Scheduler, with
a warning, when its linked project holds more targets than the plan frames
there, so no target's exposure plans are taken over while others stay
unplanned. New
imports retain the source project's Low/Normal/High priority (0/1/2) on each
objective; missing, null or unknown priorities use Normal. Filters do not gain
priority from alphabetical ordering. Until someone saves the plan or its
layout here, or activates it, the plan follows its project: a listing after a
change in Target Scheduler (a desired count raised, a target moved) takes the
drafts in again under the same objectives, and keeps the survey and view the
person chose. From the first save or activation on, the drafts are the
operator's and nothing is imported again. The meta store's `draft_import`
table records which project a plan came from and the revisions the import
saved; plans imported before schema 26 are marked when both drafts are still
the import's (revision 1, saved together) and nothing activated them. The
targets are measured in the plane of the mosaic's centre, where N.I.N.A.
lays panels out, so a mosaic far from the equator still reads as its grid.

A project row without a GUID is skipped until it gets one. Target
Scheduler added GUID columns in its schema 22 and meant to give every
existing row one, but it only does so when a database stops at 22: one
upgraded from 21 or earlier straight to 23 in a single start keeps empty
GUIDs on every row older than the upgrade. Settings › Databases says so for
such a database and offers **Fill in GUIDs**, which copies the file beside
itself (`<file>.before-guid-fill-<seconds>`) and then gives each empty row
in `project`, `target`, `exposureplan`, `exposuretemplate`,
`acquiredimage` and `profilepreference` a new GUID, touching nothing else;
`psf-guard -d <file> fill-guids [--dry-run]` does the same from the command
line. Close N.I.N.A. on that rig first, and fill the rig's own database
rather than a sync copy of it, so the GUIDs start where the rows do. A file
the server cannot write is reported with no button, and nothing is copied
or changed. The next listing takes the projects in as plans. A file
with no project table, or with more than 4096 projects, is reported in the
Library's plans section and left out. A file that is busy or cannot be
opened is shown as the last listing read it, with a warning; one no listing
has read since the server started keeps its plans' links, marked
`source_unread`, with no rows or progress. A file whose identity table
cannot be written yet is planned under the identity it will get, and a
later listing writes it. A record that cannot be read, such as a damaged
framing draft, empties only its own part of its plan's row, and the
warnings name it.

The identity table, `psf_guard_catalog_identity`, and the Director side
tables are plain SQL, so N.I.N.A., Target Scheduler and any SQLite tool open
the file as before; a table an earlier preview build created with `STRICT`
is rebuilt in place, rows intact, the next time PSF Guard writes it. Sync
never carries these tables: a planning push, a grade push or a pull moves
Target Scheduler rows only, so a database synced from a peer gets an
identity of its own when it is adopted here. Copying the file by hand does
carry the identity, and two registered files with one identity are one
catalog: the first by slug stands for it and the copy is named in the
Library's plans section and left out of planning until it is removed from the registry
or its identity table is dropped.

### Attaching a database's project to a plan

Two databases that each made their own project for one target become two
plans, since their GUIDs differ. In the plan's workspace, below the rigs,
**Attach a project from another database** lists the
projects of other plans in databases this plan has no project in yet.
Attaching moves every database link of that other plan onto this one and
retires it; this plan keeps its own framing and plan drafts and takes the
other's only where it has none. Nothing in the rig databases moves: the next
activation takes the attached project's targets over where they stand, by
name, place, or as the lone target, and takes over their exposure plans for
the same work rather than adding twins.
**Detach** beside a linked database hands that project a plan of its own
again, named after the project; this plan keeps its drafts, less the work it
planned for that database's rig. When there is such work, Detach first says
how many exposure goals go and asks again (**Detach anyway**); the project
keeps its rows in Target Scheduler either way. This plan's activation record
forgets the rig, so its next activation neither sets the detached project
Inactive nor writes to it: a rig database's own record of which plan owns a
project follows the plan links, and activation updates it when a link moves. A plan never
holds two projects in one database, so attaching such a plan is refused.

### Plans are Library projects

A plan has no list of its own: it is its projects in the Library.
Rows and cards use the same pills in the same order: the database, Target
Scheduler's state there (Draft, Active, Inactive, Closed), accepted against
desired with a bar that reads **Done** once the goal is met, the grading
split, and the first and last capture with how long ago that was. A plan
shot by several rigs is an outer pill, **N rigs**, with the plan's total
progress, its stage and its Planning button, and one row per rig under it,
since each rig holds its own targets and exposure plans and can be ahead of
or behind the others. The Compact / Detailed choice is Settings → Review →
Library.

The **Show** select narrows the Library family by family: Active, Inactive
or Draft keep a family when any rig has that state; Closed and Done ask for
every rig; Still to shoot keeps what is neither done nor closed; with
Director on, No database keeps only plans nothing shoots yet. Closed projects sit in the
archive, which opens when Show or search narrows the list. The search box
matches project and target names. Both live in the URL (`show`, `q`), a
"Showing N of M projects" line says how much is hidden, and **Show all**
clears both. When the server runs with database management, a row's state
pill is a select: choosing Active, Inactive, Draft or Closed writes that
project row in Target Scheduler at once, the same edit the plan editor
makes. The plans with nothing captured yet follow Show and search too.

The header's project picker follows the same rule. A project that lives in
several databases under one GUID is one row there, badged with its rig count
and naming every database; it opens to one block per rig, each with that
rig's images and targets, and typing any rig's name, database or target
finds it. The grid still shows one database at a time, so choosing a block
scopes the grid to that rig. Grouped by database in Settings, each rig keeps
its own row and names the others.

## Protocol 1

Routes below start with `/api/director/v1`. Responses use the normal
`{success, data, error, status}` envelope. The API requires normal browser
authentication or a user API token when accounts are configured. The existing
trusted-loopback and explicit anonymous-access policies still apply when no
accounts exist. A database Sync key or pairing code does
not grant access. Read-only users may inspect identities but cannot mutate them.
All metadata routes also require the database-management gate. Scoped Director
client credentials are the limited exception to operator authentication described
below; Sync credentials remain unrelated.

### Pair a Director client

This operator API has no pairing UI yet. Open Director to adopt the registered
database first, then use its exact `catalog_id` and bound `rig_id`; get the coordinator's
`instance_id` from `/status`. Use an interactive read-write browser session, or the
existing explicitly trusted local/anonymous operator mode, to issue a code:

```text
POST /api/director/v1/rigs/{rig_id}/pairing-token
{"coordinator_instance_id":"<instance UUID>","catalog_id":"<catalog UUID>"}
```

The response `data` contains `protocol_version: 1`, `coordinator_instance_id`,
`catalog_id`, `rig_id`, `pairing_token` and `expires_at_ms` (Unix milliseconds).
Codes start with `psfdpt_` and expire after one hour. Issuing another code for the
same rig invalidates its previous unused code, not its existing clients. Operator
API tokens and read-only sessions cannot issue, list or revoke Director clients.

The plugin exchanges the code without an Authorization header:

```text
POST /api/director/v1/pair
{"protocol_version":1,"pairing_token":"<code>","profile_id":"<NINA profile UUID>","client_name":"NINA observatory"}
```

`profile_id` must be a nonnil UUID; `client_name` is 1-80 UTF-8 bytes, without
control characters or leading/trailing whitespace. Requests reject unknown fields
and bodies over 4096 bytes. The response `data` is:

```json
{
  "protocol_version": 1,
  "coordinator_instance_id": "<instance UUID>",
  "catalog_id": "<catalog UUID>",
  "rig_id": "<rig UUID>",
  "profile_id": "<NINA profile UUID>",
  "client_id": "<client UUID>",
  "token": "psfdrc_<64 lowercase hex digits>",
  "scopes": ["program:read", "checkin:write", "status:write"]
}
```

Persist the token only in the OS credential manager, separate from Sync. Do not
log or display it. Store the nonsecret binding tuple with the NINA profile and
validate the response before replacing any previous pairing. Use HTTPS off a
trusted local network; pairing codes and tokens are bearer secrets. Do not follow
redirects with either secret. Responses are `Cache-Control: no-store`.

For subsequent requests send `Authorization: Bearer <token>` and exactly one
`X-PSF-Director-Profile: <canonical lowercase NINA profile UUID>` header. Only these
exact methods/routes are allowed, for the paired rig:

- `GET /api/director/v1/rigs/{rig_id}/program`
- `GET /api/director/v1/rigs/{rig_id}/allocation`
- `POST /api/director/v1/rigs/{rig_id}/allocation/start`
- `POST /api/director/v1/rigs/{rig_id}/checkin`
- `POST /api/director/v1/rigs/{rig_id}/status`
- `POST /api/director/v1/rigs/{rig_id}/equipment-reports`
- `POST /api/director/v1/rigs/{rig_id}/workloads/request`
- `POST /api/director/v1/rigs/{rig_id}/workloads/release`

Their existing coordinator/catalog query or body fields are still required and
validated. A ledger already reported by one rig cannot accept any sequence from
another rig, even if that rig has its own valid credential. Equipment registration,
planning edits and operator-wide status remain
operator-managed. A profile header asserts the profile context; possession of the
bearer token remains the authentication proof. It is not equipment attestation.

### Reviewing native equipment evidence

The Director Session's **Report equipment** command submits the connected
camera, filter mapping and the configuration fingerprint for that session's
native horizon/meridian policy. Acquisition can remain disabled. The report is
staged, not applied: it cannot replace equipment setup, activate a plan, create
an allocation or grant a launch. Existing pairing scopes remain unchanged;
staging this evidence is limited reporting, not operator authority.

All routes below are relative to `/api/director/v1`:

- Paired `POST /rigs/{rig}/equipment-reports`: strict body fields are
  `coordinator_instance_id`, `catalog_id`, `report_id` (fresh UUID),
  `observed_at_ms`, `configuration`, and complete `filter_names`.
- Operator `GET /rigs/{rig}/equipment-reports`: latest report per live paired
  client, including its client/catalog/rig/profile tuple, observation and receipt
  times, and nullable `accepted_revision` / `accepted_from_revision`.
- Interactive operator `POST /rigs/{rig}/equipment-reports/{client}/accept`:
  `coordinator_instance_id`, `catalog_id`, `report_id`, `expected_revision`
  (current rig profile revision, or zero). Returns the saved `RigProfile`.

Acceptance preserves optics, site, horizon, limits, sky quality and Sync peer.
It compares the exact report UUID and profile revision atomically. A report
older than fifteen minutes, a changed pairing, or an outstanding allocation is
refused with `409`; no allocation is replaced or refunded. Older observations
cannot replace newer reports. Repeating an identical report preserves its
original receipt time; retrying an acceptance is safe only while that accepted
profile revision remains current. Revocation removes pending reports, not the
operator-approved profile. A receipt is not a readiness or acquisition permit.

The review UI is not implemented yet. These endpoints are the handoff for the
rig setup screen; show the exact paired profile and report age, compare pending
and active capabilities, and require explicit acceptance. Do not auto-accept
on refresh, pairing or activation. Keep allocation admission a separate action.

An operator lists nonsecret client records with `GET /rigs/{rig_id}/clients` and
revokes with `DELETE /rigs/{rig_id}/clients/{client_id}` (both relative to the API
prefix above). Listing returns an array of `client_id`, `catalog_id`, `rig_id`,
`profile_id`, `client_name`, `created_at_ms` records; revocation returns
`{"revoked":true}` or `false` if absent. The limit is 256 clients per rig; revoke
unused records before issuing more. Every authenticated request rechecks the
credential and current catalog-rig binding. Invalid credentials, profile or route
scope return 401; disabled management returns 403, and a write that waited its
full turn for the metadata store answers 503.

Metadata schema 11 stores only SHA-256 hashes of random 256-bit secrets in separate
Director tables. Code consumption and client creation commit together. A failed
write leaves the code usable; if a successful pair response is lost, issue a new
code and revoke the orphan client. Codes and clients survive restart; backups
contain their hashes, so protect backups and never run a restored coordinator
beside its source. Neither pairing nor receiving the current program is permission
to activate a durable ledger: allocation/refresh correctness remains a separate
delivery requirement.

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/status` | Reports `protocol_version`, `enabled`, `instance_id`, and `database_management`. |
| GET | `/catalogs/{slug}/discovery` | Read project/profile evidence from one already registered catalog. |
| GET | `/catalogs/{slug}/mappings` | Optional `after` source-project UUID and `limit` 1-256; returns `catalog_identity`, optional bound `rig`, `items` and `next_after`. |
| GET | `/projects` | Optional `after` UUID cursor and `limit` from 1 to 256 (default 64). |
| POST | `/projects` | `{"id":"<caller-generated UUID>","name":"M31"}` |
| GET | `/projects/{id}` | Exact project UUID. |
| PATCH | `/projects/{id}` | `{"expected_revision":1,"name":"Andromeda"}` |
| POST | `/projects/{id}/attach` | `from_project_id`: the plan to absorb. Moves every database link of that plan onto this one, takes its framing and plan drafts where this plan has none, and retires it. `409` when both plans link the same database, `404` when the other plan is gone, `400` for a plan attaching itself. Answers `into`, `absorbed`, `moved_links`, `framing_taken`, `plan_taken`. |
| POST | `/projects/{id}/detach` | `catalog_slug`, `source_project_guid` and `name`: moves that one database link onto a new plan called `name` and answers the new plan's identity. `404` when the link is not this plan's. |

The same identity operations and body/query shapes are available at `/sites`
and `/rigs`. The latter remains a compatibility API for prototype references;
normal clients should enable planning on a registered database instead. Identity
names never establish a binding. Identity routes do not delete an identity or
grant credentials; the separate pairing routes above do the latter.

A global project identity is not a Target Scheduler project or a catalog-local
integer ID. Same-name projects stay distinct. Keep the caller-generated UUID
across create retries. Retrying the same ID and name is idempotent; changed
content under an existing ID returns `409`. Renames require the current revision
and increment it when the name changes. Reload after a revision conflict.

Listings return `items` and `next_after`, ordered by stable UUID rather than
name. Each page is a fresh read, not a long-running snapshot; restart a full
listing to discover concurrent inserts before its cursor. Request bodies are
limited to 4 KiB for identity operations, and unknown fields are rejected.
There is no deletion endpoint.

## Catalog adoption

Adoption is automatic (see above). The reviewed endpoints remain for tools:
`GET /catalogs/{slug}/discovery` lists a database's project rows with their
GUIDs and profiles, `GET /catalogs/{slug}/mappings` lists its links,
`POST /catalogs/{slug}/rig/preview|apply` binds it to a rig, and
`POST /catalogs/{slug}/adoption/preview|apply` links chosen rows to a chosen
plan with a preview digest. Same-GUID rows always join the plan that GUID
already has; a row cannot be moved to another plan through these calls.

## Configuration snapshots

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/sites/{site}/snapshots` | Optional `after` UUID and `limit` 1-256; returns `ids` and `next_after`. |
| POST | `/sites/{site}/snapshots` | Complete `SiteSnapshot`: `id`, `site_id`, `location`, `horizon`. |
| GET | `/sites/{site}/snapshots/{snapshot}` | Exact immutable snapshot owned by the named site. |
| GET | `/rigs/{rig}/setups` | Optional `after` UUID and `limit` 1-256; returns `ids` and `next_after`. |
| POST | `/rigs/{rig}/setups` | Complete `RigSetup`: `id`, `configuration`, `site_snapshot_id`, `minimum_altitude_degrees`, `maximum_altitude_degrees`, `meridian_exclusion`. |
| GET | `/rigs/{rig}/setups/{setup}` | Exact immutable setup owned by the named rig. |

Snapshot bodies allow 256 KiB, matching the shared-core bound, so native horizon
breakpoints are not thinned to fit an identity form. Send canonical horizon
content, never a local file path. The site/rig must already exist, and a setup
must reference a registered site snapshot. A POST whose owner differs from the
URL returns `400`; fetching an ID through another owner returns `404`.

The coordinator setup UUID is distinct from `configuration.id`, the native
equipment fingerprint. Keep both unchanged on retry. Identical retries return
the stored content; changed content under the same snapshot/setup ID returns
`409`. Create a new ID to change location, horizon, equipment or limits. These
routes do not select a latest/active setup, replace an in-flight assignment, or
make a registered rig eligible to acquire. See the typed
[configuration model](../crates/director-meta/src/configuration.rs).

These are operator APIs using ordinary user authentication. Scoped Director
credentials cannot edit snapshots or register equipment. Assignment issuance and
snapshot editing remain separate work. Do not put an operator API token in a
plugin profile as a substitute for pairing.

## Rig profile

Each database bound to a rig carries one mutable rig profile in the meta
store: optics, site, horizon, sky quality, altitude and meridian limits, and
the camera configuration the N.I.N.A. plugin last reported. Plans read it;
activation later freezes an immutable setup revision from it. A profile grants
nothing.

Open **Settings**, choose the database, then **Project planning links**. Once
planning is enabled, the **Rig profile** card sits above the mapping table.
**Use frame headers** copies the sensor size, pixel size, focal length,
aperture and site from the newest frame whose file the server can find; the
card names the file so a stale header is never mistaken for a measurement.
Every section shows where its values came from: frame headers, set by hand, or
reported by the plugin. The field of view and pixel scale update as you type.

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/catalogs/{slug}/rig/profile` | The rig, its profile (revision 0 when nothing is saved), the computed `field_of_view`, and header-derived `defaults`. `404` until planning is enabled on the database. |
| PUT | `/catalogs/{slug}/rig/profile` | `expected_revision` plus `optics`, `site`, `horizon`, `sky_quality` (each `{value, source}` or `null`), `limits`, and `peer_id` (a registered Sync peer or `null`). Sources may be `manual` or `frame_headers`; `409` when the revision moved, `400` for a peer this server does not know. The plugin's configuration is kept as stored. |
| PUT | `/rigs/{rig}/equipment` | Plugin report: `coordinator_instance_id`, `catalog_id`, core `configuration`, `optics`, optional `site`, `horizon` and `limits`, and `reported_at_ms`. All three identities must match this server's binding or the call returns `403`. An identical report does not bump the revision. |

**Remote site** names the Sync peer that holds the rig's real database when
the rig runs on another PSF Guard. The database registered here is then a
copy pulled from that peer, and activation pushes the plan back to it; see
[Activation](#activation). Leave it at **This server only** for a rig whose
N.I.N.A. writes into this server's database.

Optics hold the unbinned sensor size in pixels, pixel pitch in micrometres,
effective focal length and clear aperture in millimetres, and how the camera
angle can change: a rotator, turned by hand between sessions, or fixed. Frame
headers written by N.I.N.A. record the binned pixel size and image size, so the
server folds both back to the sensor before offering them. Typed models:
[optics](../crates/director-core/src/optics.rs) and
[profile](../crates/director-meta/src/profile.rs).

## Sites and horizons

A site holds a location (latitude, east-positive longitude and elevation) and
a horizon. Add and edit sites under **Settings**, **Rigs**, **Sites**. Each
rig picks its **Planning site** in its Setup card and takes the site's
location and horizon. A rig's own values win: a location typed into its card,
and a horizon pasted there or reported by the N.I.N.A. plugin. Leave the
rig's fields empty to use the site's. The card says where planning takes each
from. Feasibility, the visibility chart and the program sent to the plugin all
use this resolved location and horizon; without one, a rig has no location
and the horizon is flat at its minimum altitude.

A horizon is a N.I.N.A. `.hrz` file: one `azimuth altitude` pair per line in
degrees, `#` for comments. Paste it or upload it, and download it back as a
`.hrz` for N.I.N.A. The server reads the file, sorts the points, and closes
the curve at 0° and 360°, taking the altitude where the line from the last
point to the first crosses north. It refuses a line that is not two numbers,
an azimuth outside 0 to 360, an altitude outside -90 to 90, and a repeated
azimuth, naming the line.

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/sites/{site}/profile` | The site and its profile: `location` and `horizon`, each `{value, source, reported_at_ms}` or `null`, and `revision` (0 when nothing is saved). `404` for an unknown site. |
| PUT | `/sites/{site}/profile` | `expected_revision` plus `location` and `horizon`, each `{value, source}` or `null`. Sources may be `manual` or `frame_headers`; `409` when the revision moved. |
| POST | `/horizons/parse` | `{text}` holding a `.hrz` file; returns the canonical `Horizon` or `400` naming the line it could not read. Stores nothing. |

`GET /rigs/profiles` carries each rig's resolved `site`: the planning `site`
it names, `location` and `horizon`, and `location_from` and `horizon_from`
(`rig`, `site` or `none`). Typed model:
[site profile](../crates/director-meta/src/site_profile.rs).

## Framing view

Open a plan from its Planning button in the Library, or pick its project in the
header and choose **Planning**. The workspace is a wide screen:
**Framing** comes first. The sky takes the width the window has and the
height left once tonight's visibility strip fits under it, and that column
stays put while the form beside it scrolls, so the horizon chart is always
on screen; the plan and activation sit under it side by side, and the linked
databases last.

The sky is drawn the way N.I.N.A.'s framing assistant and the Sky view draw
it: a stereographic globe about the center of the view, so a drag turns the
sky under the pointer and a rectangle away from the center leans with its
local north. A drag works one of two ways, as in N.I.N.A., chosen by the
**Rectangle** and **Sky** buttons at the top of the stage. **Rectangle**
(the default, like N.I.N.A. with a survey image): drag the rectangle to move
the target, and drag the sky to look around without moving it. **Sky** (like
N.I.N.A.'s sky atlas): the rectangle stays where it is and dragging anywhere
turns the sky, and the target with it; Shift-drag looks around without
moving the target. The turn button beside them keeps the rectangle upright
and turns the sky by the camera angle instead, N.I.N.A.'s "rotate sky"; the
compass shows where north has gone. The buttons after it switch the layers:
the equatorial grid, the constellations, deep-sky marks, comets and
asteroids, and the Sun, Moon and planets. Every choice is remembered in the
browser. Scroll or use the corner buttons to zoom, from three arcminutes
across out to a hemisphere; the zoom follows how far the wheel or trackpad
moves, so a swipe zooms as much as the same distance of wheel notches.

The survey picture follows at every zoom, out to a hemisphere: the server
renders it at the width asked for, and the offline maps composite every tile
of the set for a wide view in a fraction of a second (decoded tiles stay in
memory between renders, and a wide view takes the small versions of each
tile). Over it sit constellation figures and names once the view is wide
enough, and an equatorial grid whose spacing follows the zoom, with
declination labelled down the left edge and right ascension along the top;
drawn stars and the Milky Way band stand in only until a picture is up.
Marks name what is in the field. When the server holds the Seiza object
catalog (the same one the astrometry uses, see `astrometry.objects` in the
registry), deep-sky objects are drawn at their catalog size and angle,
brightest and largest first, with a label for the most prominent; when it
holds the minor-body catalog, comets and asteroids are placed for the moment
the view is looked at (comets with their tail direction, asteroids with
their motion); and the Sun, Moon and planets come from a built-in ephemeris
good to a few arcminutes. Each layer has a button at the top of the stage,
and says on the stage in a few words when its catalog is not on this
server; the note's tooltip gives the reason. Deep-sky
marks start on; comets, asteroids and the solar system wait for their
button, since a zoomed-in field otherwise fills with faint asteroids.
**Catalogs marked** under **View** picks the catalog families that get
marks: Messier, NGC, IC, Sharpless, LDN and LBN start on, the map an imager
frames by; Barnard, vdB, SNR, UGC, WR, PGC and HD wait for a chip, PGC and
HD above all, since PGC lists hundreds of faint galaxies in any field and
HD every star the survey already shows, and with them in, the limit of 300
marks left no room for the nebulae and clusters.
The choice is remembered in the browser and sent as `catalogs`. A mark is a
catalog place, not pointing evidence; the survey picture and the plate solve
say where things really are. The
projection is stereographic, in the browser, on the server's survey images
and on the offline maps alike. The picture is drawn on the GPU: for every
frame, each pixel of the stage looks along its direction on the sky and
samples whichever of the tiles fetched so far holds it, finest first, with
the same arithmetic the server used to render the tile run in reverse. So
the picture sits under the grid and the rectangle exactly at every zoom
and turn while the pointer moves, a newly landed tile changes nothing but
detail, and the tiles around and above the view stay on hand for a pan or
a zoom step. A browser without WebGL gets the newest tile laid in through
one fitted transform instead, which is right at the center of the view. At framing widths the projection is the
tangent plane to within a pixel, and the readout under the stage gives the
width of sky the stage really spans. The rotation handle lives on the
target's own plane, so it stays on the rectangle's up direction wherever the
view is.

**Framing** shows the sky around the project's first target on a survey
image, with the target's catalog coordinates and rotation already filled in,
the way N.I.N.A.'s framing assistant does. When the server holds N.I.N.A.'s
offline sky maps (below), a new framing starts on the offline DSS map and the
narrowband maps lead the chips, so framing works with no network at all.
**Find a target** offers names as you type: the server's Seiza object
catalog answers from its name index (designations, common names and
aliases, one row per object with the kind beside it) with no network at
all, and the last row of the list asks CDS Sesame (Simbad, NED, VizieR)
for the name as typed. Enter takes the highlighted row, else an exact local
match, else goes online. Sesame's answers, hits and misses alike, are kept
under the cache root (`director/sesame`), hits for ninety days and misses
for a day, so a name goes online once. A pick moves the target and the view
there without saving anything; **Undo** beside the notice puts the previous
target back, and **Back to saved framing** drops every change since the
last save. The rectangle and the
mosaic grid are drawn in the browser with the same tangent-plane geometry the
server applies when it activates, so they follow the pointer at once; a
fixture shared between the core's tests and the browser's keeps the two in
step. Chips on the sky switch the layer:
the DSS2 colour plates N.I.N.A. starts from, and the narrowband surveys
(Finkbeiner H-alpha and the Northern Sky Narrowband Survey's H-alpha, O III,
SHO and colour layers); the **Survey** list under View holds every layer.
The layer is how the sky is shown, not part of the framing: picking one is
never an unsaved change, and this browser keeps it for every plan.
On a narrow stage, such as a phone's, the chips fold behind one chip that
names the layer on screen, so they keep clear of the sky and the scale. The rectangle is the field of the **panel rig**: the first rig
holding the project that knows its optics is chosen for you (rigs take their
optics from their own frames when they are adopted), and you can pick another
or type a panel size. Drag the rectangle to move the target, drag its handle
to turn it, and drag the sky to look around; scroll to zoom. The camera angle
also takes a number, quarter turns, or the rig's fixed camera angle. Rows,
columns and overlap under **Shared framing** lay out a mosaic; every panel is
drawn and named. Coordinates show five decimals of a degree and angles two;
the draft keeps every digit. **Rigs** has one card per rig, this plan's rigs
first. Each card shows a swatch in the colour of the rig's outline on the
sky, its field and pixel scale, and its part: **Sets panel size**, **Shared
framing** or **Separate framing** with its grid and angle. On the planning
page, each rig has an **On** switch: rigs that shoot the plan are listed under
**On**, the rest under **Off**, and several can be on. Turning a rig on
frames the plan with it, and clicking an on rig's heading (swatch, name and
field) does the same: its field sets the shared framing's panel size, and its
card is highlighted. **Shared framing** names the rig its size
comes from and the size the rectangle uses; **Type a size** sets one by
hand instead, starting from that rig's, and **Use current field** appears
when the rig's field has changed since it was picked. When the rig that
sets the size is turned off, here or elsewhere, the first rig that is on,
knows its optics and shoots the shared framing takes over; with none, the
size stays and the line says its rig is off. **Outline** draws its field on the sky, named
in small text, and **Frame separately** gives it a grid, camera angle and panel size of its own, edited
in its card. A separate framing follows the shared center until it is dragged,
typed or moved to the view center; **Shared center** puts it back in step.
**Use shared** drops it. The separate framing of a rig that is off, or no
longer listed, is drawn dim and stays where it is; one whose rig is gone has
no card, so it is listed under **Rigs** with **Remove**. When every rig that is on frames separately, no
rig shoots the shared framing, so its rectangle is hidden and activation
plans no target for it. **Move target to view center** and **Center view on target**
keep the two apart on purpose: panning the sky never moves the plan. **Save
framing** keeps a draft on the project; a draft saved elsewhere since you
loaded is refused until you reload. A value the server would refuse (a
panel side outside 0.01° to 30°, a mosaic wider than 30°) is marked at its
field with the reason, and the save waits until it is fixed. Saving leaves
the view where it is, and an edit typed while the save was out stays an
unsaved edit. A reload that fails keeps the framing on screen under a line
that offers to try again. Angles are kept in 0° to 360°, rounded to a
billionth of a degree, so a quarter turn and back is no change. Survey imagery is attributed below the
view and is a composition aid, not pointing evidence.

### Finished stacks on the sky

The framing view also draws the panels that have been shot. A panel is a
target an activation wrote, or, for a rig no activation reached, a target of
its linked Target Scheduler project: a plan taken in from Target Scheduler
that already matches it is never activated and still shows its stacks. For
each panel the view finds the stacks the Sky page and the stack preview jobs
publish for that target, one per colour composition and one per filter, and
lays the chosen one on the sky where its plate solve puts it: the stack's
reference frame solve carried through the stack's orientation, so a colour
or mono stack lands on its true position, not where the plan hoped.

The **Finished stacks** button among the sky layers, or the **Stacks** box
under the sky, turns the layer on and off; the setting is kept in this
browser. **Stack shown** picks which stack: **Best per panel** takes a
current colour stack first, then the mono stack with the most integration;
a composition or a filter shows that stack on every panel that has one, and
is kept for the plan in this browser. The list under the picker gives every
panel with its rig, frames accepted against desired, and why its stack is
not drawn: none built yet, none of the picked kind, or built but not solved.
Panels from different rigs are drawn side by side and never blended;
overlaps show seams and depth differences, which is the point. When the
framing has changed since the activation the list says so: the stacks stay
where their solves put them and the rectangles show the new plan.

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/projects/{id}/mosaic` | Per panel, activated or from a linked project: `panel_id`, `rig`, `catalog_slug`, `target_id`, `progress` (`desired`, `acquired`, `accepted` over its exposure plans), `status` of the best stack (`ready`, `unsolved`, `no_stack`, `missing_target`, `missing_catalog`), `stacks` (each with `key` such as `color:SHO` or `mono:Ha`, `label`, and `wcs` when solved; best first) and `preview`, the first of them. `activation_revision` is `null` before the first activation; `framing_stale` flags a framing saved since. |

### A mosaic as one view

Target Scheduler keeps a mosaic as one target per panel. When a project's
targets are one mosaic, the header's rig and target switcher offers it as
one more choice, such as **M31 mosaic (4 panels)**, beside **All targets**
and the single panels. The choice sets `mosaic=1` next to `db` and
`project`, so a reload or a shared link opens the same view.

- **Images** groups the frames under their panels, row by row from the top.
  Each panel's first group carries its heading (`r1c2 · M31 Panel 2`), and
  the grouping mode still splits frames within each panel. Arrow keys,
  Space and Shift selection run across panels as across any groups.
- **Sequence** lists every panel's frames in capture order, each card named
  by its panel, with a filter choice and a button that opens each panel's
  own sequence. Quality scores stay per panel.
- **Stacks** shows each panel's latest stack in the mosaic's grid, and a sky
  overview that places every solved stack by its plate solve with the
  framing view's projection. Nothing is stitched.

Grading, calibration and stacking stay per panel.

The panels come from Director's record when it activated the project: the
rig database's `psf_guard_director_target` rows name each target's panel
(`r{row}c{column}`). Without that record the targets' coordinates and
rotation must form a full grid, read the same way a framing draft is
imported from Target Scheduler. Target names are never read for this, and
a project whose targets form no grid has no mosaic choice.

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/api/db/{db}/projects/{id}/mosaic` | `mosaic`: `null` when the project is not a mosaic, else its `name`, `source` (`director` or `inferred`), `rows`, `columns` and `panels` row by row, each with `target_id`, `target_name`, `panel_id`, `row`, `column` and the latest stack `preview` with its `wcs` when solved. |

The imagery comes from the same HiPS surveys N.I.N.A.'s framing assistant
downloads, including the narrowband layers, fetched by the PSF Guard server
and cached under its cache directory. N.I.N.A.'s own framing cache lives on
the rig's Windows machine in `%LOCALAPPDATA%\NINA\FramingAssistantCache`
(`CacheInfo.xml` with `RA`, `Dec`, `FoVW`, `FoVH`, `Rotation`, `Source` and a
JPEG per entry); reusing it is planned through the Director plugin, which is
the process that can read that folder.

## Acquisition plan

The **Exposures** tab says what the project wants and how each rig shoots it.
An objective is one bandpass and purpose (faint detail or unsaturated stars)
with a goal in accepted hours or accepted frames per rig. Project precedence is
set in **Project priority**, not as a score on each objective. Add a rig
on the **Rigs** tab to have it take part: for every objective the editor picks the rig's first
exposure template whose filter resolves to that bandpass, and starts the
exposure length from the template's default, or from the rig's optics and sky
when the template has none. Hours become frames per rig through that exposure,
so a fast rig under bright skies shoots more short frames than a slow one under
dark skies for the same goal. Each rig can set its own goal for an objective in place of the
plan's, for a slower or faster rig. For an hours goal, a rig more than a
quarter away from f/5 is offered a suggestion that reaches f/5's depth, read as
signal per pixel going as one over the f-ratio squared: about 24 h at f/10 for a
6 h goal. A suggestion applies only when clicked, and **×** returns the rig to
the plan's goal. Activation and feasibility use each rig's own goal. Switching a goal between hours and frames keeps
the goal: the number is converted through the exposure of the first rig
shooting the objective, or the first rig's default for that band. A rig binds each objective to a template in its own database, or to one
from the shared **Exposure templates** library (below), which activation
writes into the database for it. A rig with neither for a bandpass sits that
one out. When the framing is a mosaic, each rig's section has a
panel chooser: every panel by default, or the panels that rig alone should
shoot, so a wide rig can take the whole field while a long-focus rig takes
one corner. A **Coverage** box names any objective and panel no rig covers.
Each rig's row totals its planned frames and hours across the panels it owns.
**Save plan** keeps the draft on the global project with the same reload rule
as framing. Feasibility by night and activation into rig databases follow.

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/templates` | The exposure template library: Director's own templates, each with `id`, `revision`, `name`, `filter_name`, `gain`, `offset`, `bin`, `readout_mode`, `default_exposure_seconds` and the `bandpass` its filter resolves to. |
| PUT | `/templates/{id}` | Save a library template; the body is the whole template and its `revision` is the one read, 0 for a new one. `409` on a stale revision. Open to every server, like plan drafts. |
| DELETE | `/templates/{id}?revision=` | Remove a library template at the revision read. Plans that chose it keep their copy of its settings. |
| GET | `/catalogs/{slug}/templates` | Every Target Scheduler exposure template in the database, across profiles, with the `bandpass` its filter name resolves to (`id`, `name`, `kind`). Read only. `warnings` names each template left out because its Moon avoidance is on with rules Director cannot plan with. |
| GET | `/projects/{id}/plan` | The project and its plan draft, or `plan: null`. |
| PUT | `/projects/{id}/plan` | The whole draft with `revision` set to the one read. `409` when it moved; `404` for an unknown project or rig. |

Filter names map to bandpasses in the shared core: `L`, `Lum` and `Clear` are
`luminance`; `Ha`, `H-alpha` and `Ha 3nm` are `h_alpha`; `OIII` and `O3` are
`oiii`; a name the core does not know becomes its own bandpass rather than
being folded into a neighbour. Default exposures also come from the core:
120 s broadband and 300 s narrowband at f/5 under Bortle 4 to 5 skies, longer
for slower optics and darker skies, shorter under bright skies, rounded to a
common length. Typed models: [bandpass](../crates/director-core/src/bandpass.rs)
and [plan](../crates/director-meta/src/plan.rs).

## Exposure template library

**Settings › Exposure templates** is Director's own list of templates: a name, the filter as the rig calls it, gain, offset, binning,
readout mode and a starting exposure. They belong to no database. A plan may
bind a rig to one of them where the rig's database has no template for the
band, and activation writes it into that database under the library's GUID,
so every rig ends up with the same template and a second activation finds
it again. Rows are edited in place and saved one at a time with the
revision they were read at; **Copy from** lists a rig's own templates and
copies any of them into the library, skipping ones already there. Removing
a template leaves plans that chose it with their copy of its settings.

## Activation

**Activation**, below Plan, pushes the framing and plan into each participating
rig's database, the same rows Target Scheduler and the Director plugin read:

- one Target Scheduler project per rig, named after the global project, in
  the Active state, marked as a mosaic when there is more than one panel, under
  the profile that owns the database's existing projects, with the plan's
  [scheduling limits](#scheduling-limits);
- one target per panel the rig owns, named after the target with the panel
  id appended for mosaics, at the panel center with the plan's camera angle,
  or with the rig's own grid and angle when it is framed on its own.
  A target the project already has (imported from Target Scheduler, or made
  by hand) is taken over rather than doubled: the one with the panel's name,
  else the one at the panel's place, else, for a single-panel framing, the
  project's only target not yet owned by a panel. The preview lists these as
  `adopt` with where they move, or `unchanged` when one already sits at the
  panel;
- one exposure plan per rig objective and panel, bound to the chosen template
  (or one matching its settings, created if needed; a library template is created under the library's own GUID, so every rig database carries the same one), with `desired` set to the
  frames that objective needs at that rig's exposure length. A plan the
  target already has for the same work is taken over rather than doubled:
  one no activation owns, on the resolved template, else on a template with
  the same filter, gain, offset, binning and readout mode, at the same
  exposure length either way (a plan left at the template's default exposure
  counts at that default). Another exposure length is other work and gets a
  plan of its own. A taken-over plan keeps its `acquired` and `accepted`
  counts; its `desired`, template and enabled flag follow the plan, and the
  preview lists it as `adopt` with its row number and what changes, or as
  `unchanged` when it already holds the plan's values.

Rows are owned by GUID. A target or exposure plan activation would take over
but that has no GUID yet refuses the rig, which says which row and points at
**Fill in GUIDs**: a GUID made at activation would exist only in that copy of
the database, and skipping the row would make a twin of it.

The preview accounts for every exposure plan on the targets: plans it adds
(`create`), takes over (`adopt`), changes (`update`), turns off (`disable`)
or leaves alone (`unchanged`), and with `keep` the rig's own plans no
objective asks for, which stay as they are in Target Scheduler. A template
activation creates is listed under `template`.

A plan an earlier activation made that nothing plans now is turned off
(`disable`, `enabled = 0`), so Target Scheduler stops taking it: its
objective turned off or removed, its band changed, its panel gone from the
framing or from the contribution's panels. Its frames and grades stay, and
its target stays. Work that comes back (a rig dropped and added again, an
objective made anew) takes those plans back, counts and all, instead of
adding twins.

A rig turned off in the plan, or no longer in it after an activation gave
it rows, has its Target Scheduler project set Inactive (`disable`), so the
scheduler stops taking it; its targets, exposure plans, frames and grades
stay as they are. Turning the rig on again sets the project Active. A project
already Inactive or Closed in Target Scheduler is left as the operator set
it, and stays that way when the rig comes back on. A rig that is off is
listed only when its project changes, and leaves the activation record's
rigs; the record keeps the projects it set Inactive (`inactive_rigs`), and
**Push again** sends those to their peers too. A rig an activation cannot
reach (its database not registered, no panel size, a refused row) keeps its
last record, so its rows stay accounted for and a later activation can still
turn it off.

A second activation updates the same rows in place: coordinates, angle,
exposure and desired counts change, names the operator edited stay, and
`acquired`, `accepted`, captured frames and grades are never touched. Panels
removed from the framing leave their targets behind rather than deleting data.

When the rows were changed in N.I.N.A. after an activation and they are the
ones to keep, a rig's block in the preview offers **Take Target Scheduler's
values** instead of overwriting them. It reads that rig's linked project the
way an import does and writes it into the saved plan:

- each band's frames: the band's goal where only this rig shoots it, else
  this rig's own goal. Goals compare by the frames they come to at the rig's
  exposure length, so six hours of 300 s frames and 72 frames are the same;
- the template and exposure length of each band, and which bands are on: a
  band the project no longer shoots is turned off for the rig, unless some
  exposure plan used a template Director cannot read, when none is;
- the framing, when the project's targets moved by more than 1″ and no other
  rig uses the same layout: the rig's own framing if it has one, else the
  shared one. A single panel can sit anywhere; a grid moved by hand is
  matched as near as a grid allows, and an activation then lines its targets
  up.

What the plan has no place for stays as Target Scheduler has it and is named:
a Draft project (an activation makes it Active) and targets turned off (an
activation turns them on). Both drafts save together under the revisions the
preview saw; a save since refuses, and the preview runs again on what is now
saved.

Before asking for an activation the page runs the same activation on an
in-memory copy of each rig's planning tables (`GET /activation/check`), so
it never locks N.I.N.A.'s database. A rig whose rows would all read
`unchanged` or `keep` already holds the plan, whatever the activation
record says.
Panel ids are places in the grid, so growing the grid or turning a mosaic can
move a panel to other sky. A panel that moves by more than a quarter of its
size and already has frames gets a new target at its new place; the old
target keeps its frames and its plans are turned off. One without frames
moves in place. Taking over a target writes J2000 coordinates and switches
it on, since the program serves only those; a linked project still in Draft
goes Active on its first activation, and every project change is listed.
Rows added to an existing project go under that project's N.I.N.A. profile,
and a template is bound by its GUID first; its id is trusted only when the
row still has the chosen settings, since Target Scheduler can reuse ids.
The preview digest leaves out the frame counts the preview quotes, so a
frame saved between Preview and Apply does not refuse the Apply; it covers
each rig's profile revision, since a rig's field sizes the panels.
PSF Guard remembers which rows it owns in three tables it adds to the rig
database, `psf_guard_director_project`, `psf_guard_director_target` and
`psf_guard_director_plan`, keyed by the rows' GUIDs; Target Scheduler and Sync
ignore them. A newly created project is linked to the global project in the
meta store, so it appears under Project planning links at once.

**Preview activation** shows, per rig database, what would be created,
updated or left alone, names any objective and panel no rig covers, and gives
any reason a rig is skipped: no registered database on this server, a Target
Scheduler schema older than 22, a database with no N.I.N.A. profile yet, or
one that cannot be opened, locked or written. Each rig database waits up to a
minute for its write lock, as every other writer on the server does, since
N.I.N.A. and a directory refresh can hold the file that long. A plan saved
before duplicates were refused may hold two contributions for one rig and
objective; the first counts.
**Apply** carries the preview digest and is refused when the framing, plan or
database changed in between. The activation record is checked before any rig
database commits, so a record too large to keep refuses the Apply with
nothing written. Each rig database then commits on its own: one whose commit
fails says so in its row and keeps what the last activation knew of it, and
the rest are applied and recorded. Preview and Apply read the metadata store
on a read connection and take its one writer only to record the result, so
check-ins, program pulls and saves never wait behind rig databases. A rig
whose plugin has not yet reported its camera still gets its rows; the Target
Scheduler plugin can run them until Director acquisition arrives.

### Rigs at another site

A rig can run on another PSF Guard, at a remote observatory, with its own
database that N.I.N.A. and Target Scheduler write. To plan for it from here:

1. Register that PSF Guard as a peer under **Settings, Remote PSF Guard**,
   and pull its catalog into a database on this server. That copy is the rig
   here: it is adopted like any other and holds the rig's profile.
2. Open **Rigs, Setup** for the copy and set **Remote site** to the peer.
3. Activate as usual. Apply writes the rows into the copy, then sends the
   planning tables (projects, targets, exposure plans, templates and rule
   weights) to the peer through Sync's planning push, previewed and applied on
   the peer in one step. Captures and grades never travel this way.

The **Remote site** column of the report says what happened per rig: *Pushed
to* the peer, or the peer's refusal or the network error, in which case the
local activation still stands. **Push to remote sites again** resends the
last activation to every remote rig's peer, for a site that was offline at
Apply. A peer removed from the registry is named in the rig's warnings and the
plan stays on this server until Setup names another.

The remote PSF Guard sees the pushed rows as ordinary Target Scheduler
projects with the same GUIDs, so its own Library and Sync work on them; it
does not become a second planner for them. The rig's Director plugin still
pulls its program from this server, the coordinator, and reports here.

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/projects/{id}/activation` | `{ activation }`: the last applied activation (revision, framing and plan revisions, and per rig the project, target and plan GUIDs) or `null`. |
| POST | `/projects/{id}/activation/preview` | Empty body. Computes and rolls back; returns the per-rig report and `preview_digest`. `422` until the project has a framing with a panel size and a plan with a ticked rig. |
| GET | `/projects/{id}/activation/check` | The preview's report, worked out on an in-memory copy of each rig database's planning tables; never writes and never takes a rig database's lock. |
| POST | `/projects/{id}/plan/take-target-scheduler` | `{ rig_id, plan_revision, framing_revision }`. Writes that rig's linked Target Scheduler project into the saved plan and framing drafts and answers the new revisions with `taken` and `left` lines; `409` when either draft was saved since, `422` when the rig has no database here or no project linked to the plan. |
| POST | `/projects/{id}/activation/apply` | `{ preview_digest }`. Commits each rig database in turn, records the activation, links new projects, then pushes each remote rig's planning rows to its peer; `409` when the digest no longer matches. Each rig row carries `push` (`peer_id`, `peer_name`, `applied`, `summary`, `error`) or `null`. |
| POST | `/projects/{id}/activation/push` | Empty body. Sends the last activation's rows again to every remote rig's peer and returns one row per pushed rig; `422` before any activation. An unreachable peer is an `error` in its row, not a failed call. |

## Program pull

The Director plugin fetches its work from the coordinator the way Sync does:
over HTTP, on its own schedule, and it keeps running on the last program it
holds when the network is away.

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/rigs/{rig}/program?coordinator_instance_id=&catalog_id=` | The rig's current program, built from the last activation: editing the plan does not change it until the next activation. Finished goals are left out and named in `omitted`; past 256 goals the highest-priority ones are kept and the rest named. Send `If-None-Match` with the last `ETag` to get `304` when nothing changed. `403` when the coordinator, catalog and rig do not match this server's binding; `422` until the rig has reported its equipment or an activated plan gives it work; `404` when the catalog is not registered here. |

The envelope carries the shared core's `Program` (schema 1): an `Assignment`
valid for 24 hours from the pull with one goal per activated exposure plan,
`requested` from the plan's `desired`, `accepted` from the rig database as it
stands, `attempts_remaining` at one and a half times the frames still owed,
and every goal eligible for the whole span so the plugin's own geometry adds
altitude, horizon and meridian windows from its constraints. `targets` come
from the panel rows, in ICRS milliarcseconds, with a position angle only when
the rig reported a rotator. `recipes` bind each contribution's template to a
filter the plugin reported, by exact name or by bandpass, with gain, offset,
binning and readout mode checked against the reported controls. `links` join
each goal to its global project, objective, contribution, panel, Target
Scheduler project GUID, target GUID and exposure plan GUID. `rig` repeats the
profile's site, horizon, limits and rotation so the plugin can build
constraints, and `omitted` names any plan row the program could not express
and why. The server binds the program through the core before answering; a
program that does not bind is a `422` with the reason, never a partial pull.

## Check-in and live status

| Method | Route | Body or query |
| --- | --- | --- |
| POST | `/rigs/{rig}/checkin` | `coordinator_instance_id`, `catalog_id`, `ledger_id`, optional `program_revision` (the revision the plugin runs), and `events`: up to 256 ledger `ExecutionEvent`s from that one ledger in ascending sequence, sent verbatim. Returns `acknowledged_through` (every sequence up to it is stored), `highest_seen`, per-event `outcomes` (`applied`, `duplicate`, `conflict`), the sequences in `conflicts`, and `program_revision` with `program_changed`. `400` for a page that mixes ledgers or rigs, runs backwards or is empty; `403` on a tuple mismatch. |
| POST | `/rigs/{rig}/operations` | Same envelope and acknowledgement as check-in, with up to 32 schema-1 preparation events. Independent contiguous cursor; gaps return `409`, changed duplicates report conflicts. Uses the paired client's existing `checkin:write` scope. Does not change capture credit or live status. |
| POST | `/rigs/{rig}/status` | `coordinator_instance_id`, `catalog_id`, `session_id`, `reported_at_ms`, optional `program_revision`, and `status`: the plugin's coalesced live report (phase, goal and target IDs, elapsed time, wait reason, safety, connectivity, queue depth), stored verbatim. `accepted: false` means a newer report was already held for that session, or a newer session exists. |
| GET | `/rigs/status` | Operator view, one row per bound rig (and any rig that reported and lost its binding): `catalog_slug` and `catalog_name`; `status` (or `null`) with `status_age_ms` and `status_stale`; capture `checkins` cursors; `contacts` (`program_pull`, `check_in`, `status`, each `{at_ms, detail}` or `null`, server receipt times); `connectivity` (`state` of `online`, `stale`, `offline` or `never`, `last_contact_ms`, `age_ms`); `assignments`; `pending_receipts`; and up to 20 `recent_operations` (completed preparation events with their first server receipt time). |

A receipt is stored once by ledger and sequence and never rewritten: a replay
is acknowledged again, a replay with different content is reported as a
conflict and left out, and a gap holds the acknowledged cursor back while the
later events are kept. Acknowledgements name one ledger and one contiguous
cursor, never a range across feeds. Saved captures the rig reported appear as
`pending` credit in the next program pull, capped at the frames still owed,
until grading turns them into `accepted` counts in the rig database. Check-in
and status both answer `program_changed` so the plugin knows to pull again;
neither of them alters a plan, a rig database or the program. Grades still
come from PSF Guard grading the images; a receipt is evidence that a frame
was taken, not that it passed.

Every program pull, check-in and status report is noted as contact with its
server receipt time, and the Live table derives connectivity from those alone.
For the status payload the Live table reads `phase` (or `state`),
`target_name` (or `target`), `operation` with monotonic `operation_elapsed_ms`
(falling back to `operation_started_ms`), `wait_reason`, `safety`, `queue_depth`
or `queue_state`, and `errors` (or `error`). `fresh_for_ms` declares the report's
lifetime, bounded to 15 seconds through ten minutes; older clients default to
ten minutes. Stale timers freeze at the reported duration. Completed operations
are shown separately, with completion and receipt ages: a reconnect is not a
new observation. Preparation history is metadata schema 22 and never affects
capture accounting. Nested native hooks are currently aggregate preparation
timings, not individual central receipts. The Sky
places a rig from `pointing`, `{ "ra_degrees": …, "dec_degrees": … }` in
ICRS (J2000) degrees with right ascension 0–360, when the plugin sends it:
the mount's position now, whatever it is doing. Without it the Sky uses the
centre of the Target Scheduler target the report names, looked up by name
among that rig's own plan targets, and draws that place dotted and labelled
"at target", since it is a guess from the plan rather than a report. A rig
with neither is listed beside the map but not drawn. The phases `exposing`,
`imaging` and `capturing` count as exposing. The rest of the payload is
stored and shown nowhere yet.

## Visibility

Under the framing stage, **Visibility** times the target from each rig that
has a site in its rig profile, the way N.I.N.A.'s framing assistant does it
for one site. It is a strip there, sized to stay on screen with the sky: the
chart on the left, drawn at one unit per pixel so it fills its box and
nothing else, and beside it the rig, the verdict and the estimate, with the
legend and the nights table folded under **Legend and the next nights**.
The server spreads the rigs and their nights over its thread pool and
prepares each instant's Earth-side astrometry once for the Sun, the Moon
and the target, so a week for two rigs answers in well under a tenth of a
second. Tonight is the night under way: it runs from local noon to the next,
so in the afternoon it is the coming night and after midnight the one that
began the evening before. A verdict line says whether the target is visible
tonight and for how long: dark hours (Sun below −12°), hours the target sits
inside the limits Target Scheduler applies there, the peak altitude, the
Moon's phase, separation and hours up. Those limits are the higher of the
rig's and the plan's minimum altitude, the lower maximum, the rig's custom
horizon raised by the plan's offset once the plan's
[scheduling limits](#scheduling-limits) turn **Custom horizon** on, the
plan's meridian window, and the rig's meridian pause. A rig framed
separately is timed at its own center. Below it an altitude chart draws
the night from an hour before dusk to an hour after dawn: the target's track,
the horizon curve at the target's azimuth (or the flat minimum), the Moon's
track dashed, a marker at the target's meridian transit with the rig's
meridian pause drawn as a broken red stretch and left out of the visible
hours, and shaded bands for darkness and astronomical night. A table
gives the same numbers for the coming week, and a rig in the plan gets the
hours it still owes, its goal less the frames Target Scheduler already
accepted there, and the nights that takes at this week's rate. Pick another
rig from the list to compare sites.

| Method | Route | Body or query |
| --- | --- | --- |
| POST | `/projects/{id}/feasibility` | Optional `nights` (1 to 14, default 7), `center` (defaults to the saved framing's center; a rig framed on its own center is timed there), `rig_centers` (rig id to center: where the framing view has a separately framed rig now, saved or not; up to 64) and `start_ms` (default now; the first night is the one under way then). For every rig with a site: the `center` it was timed at, the applied `limits`, `nights` summaries, tonight's `curve` (five-minute samples of Sun, Moon and target altitude with the horizon at each azimuth), `hours_needed` (the plan's goal less accepted frames) and `nights_to_complete`; rigs without a site are named in `warnings`. `400` for a `start_ms` outside 1970-01-02 to 2099-01-01; `422` until there is a center to time. |

Times come from the shared core's planning-grade Sun and Moon positions and
the same horizon and limit rules the rig's geometry applies; they are
estimates for choosing targets and nights, not the rig's dispatch decision.

## Framing drafts

A global project can carry one framing draft: the target center, the camera
angle, the mosaic grid, which rig's field defines a panel, which other rigs to
overlay, and the survey and zoom the operator was looking at. Drafts are
editable and versioned; activation later freezes intent from them. The shared
core computes every footprint, so the browser only draws.

| Method | Route | Body or query |
| --- | --- | --- |
| POST | `/framing/preview` | Stateless. `center`, `position_angle_degrees`, `panel` (`width_degrees`, `height_degrees`), `mosaic` (`rows`, `columns`, `overlap_percent`), optional `overlays` (other fields to place on the center) and `view` (`center`, `rotation_degrees`). Returns each panel's sky corners and, with a view, its corners as offsets from the view center with the view's up as `+eta`. `400` for a grid above 16 by 16, more than 256 panels, an extent past 30 degrees, or a view that cannot see the center. |
| GET | `/projects/{id}/framing` | The project and its draft, or `draft: null`. |
| PUT | `/projects/{id}/framing` | The whole draft with `revision` set to the one read (0 when none existed). `409` when it moved; `400` when `project_id` disagrees with the URL, the geometry fails the core's checks, or `view_fov_degrees` is outside 0.02 to 180; `404` for an unknown project or `panel_rig_id`. `layout_revision` moves only when what activation writes changes; centers and angles within 1e-7° of the saved ones count as unchanged. |
| GET | `/rigs/profiles` | Every registered database bound to a rig: `rig`, `catalog_slug`, `catalog_name`, the rig `profile` (or `null`) and its `field_of_view`. Unbound or unreadable databases are left out. |

Offsets are gnomonic (tangent-plane) degrees with east positive, exact for any
field a camera sees. Panel rows count from the top of the mosaic as the camera
sees it and columns from the east. Typed models:
[framing](../crates/director-core/src/framing.rs) and
[draft](../crates/director-meta/src/framing.rs).

## Offline sky maps

N.I.N.A. publishes its framing assistant's whole-sky caches for download:
the DSS plates (3.3 GB), and the Northern Sky Narrowband Survey in SHO
colour with stars (1.4 GB) and starless (0.5 GB). Each is a folder of 5°
tiles with a `CacheInfo.xml` index, the same format N.I.N.A. keeps under
`%LOCALAPPDATA%\NINA\FramingAssistantCache`. PSF Guard reads such folders
from `<cache>/director/sky-maps/` and offers each as a survey layer, named
`nina:<folder>`, rendered on the server from the tiles that cover the view
(the 75, 150 or 500 px versions for wide views, the full tiles when zoomed
in) and cached like a fetched cutout. Nothing leaves the machine.

Install one with the server's cache root:

```bash
psf-guard sky-maps install full --cache-dir /path/to/cache
psf-guard sky-maps install nsns-ohs --cache-dir /path/to/cache
psf-guard sky-maps install nsns-ohs-starless --cache-dir /path/to/cache
psf-guard sky-maps list --cache-dir /path/to/cache
```

`install` takes a zip URL too, and a folder copied from a rig's own framing
cache works as well: drop it under `sky-maps/`. A running server offers a new
folder within half a minute. The maps carry their own licences (the NSNS
sets are CC BY-NC-SA); the attribution line under the framing view shows it.

An offline map is preferred wherever it can stand in for an online survey:
the DSS set for the DSS2 colour plates and the SHO set with stars for the
online NSNS SHO layer (`stands_in_for` on the survey listing names the
pair). A plan whose framing was saved on the online layer opens on the
offline map; the online layer stays a click away on its chip, and a layer
picked by hand is kept in this browser and comes first.
When the server first lists a set it decodes the smallest version of every
tile into memory in the background, a few tens of megabytes, so the first
wide view renders at once; and once a framing view has its own tile, the
browser quietly fetches wider views of the same place out to a hemisphere,
which the server keeps, so zooming out and the next visit have a picture
already. A view's own tile is asked for a little wider than the view (three
tenths more) at the stage's own pixel density, up to the server's 2048
pixels a side, so the picture is sharp across the whole stage and not only
where the last narrow tile happened to land.

## Sky survey cutouts

The framing view draws on survey imagery fetched by the server from the CDS
HiPS2FITS service, the same source N.I.N.A.'s framing assistant uses. The
survey list matches N.I.N.A.'s: DSS2 color, red, blue and near infrared, SDSS,
DESI Legacy, SkyMapper, 2MASS, CTA-FRAM, the Mellinger panorama, the Finkbeiner
H-alpha composite and the Northern Sky Narrowband Survey layers. Narrowband
layers are marked as such and never stand in for one another.

| Method | Route | Body or query |
| --- | --- | --- |
| GET | `/sky/surveys` | The allowed surveys: `id`, `name`, `hips`, `kind` (`broadband`, `narrowband`, `panorama`), `bandpass`, `attribution`. |
| GET | `/sky/search` | `q` (2 to 64 characters typed so far), optional `limit` (8 by default, at most 25) and `online` (false by default). Answers `local`: the Seiza object catalog's names starting with `q`, one row per object with `name`, `common_name`, `kind`, ICRS degrees, the `matched` designation or alias and `source`, an exact match first, plus whether the catalog is `available`. With `online=true` it also asks CDS Sesame through the cache: `online` (the resolved name or null), `online_state` (`hit`, `miss`, `failed`, or `skipped` when not asked), `online_note` and `online_cached`. |
| GET | `/sky/resolve` | `name`: an object name. The local Seiza catalog answers a name it knows in full (source `Seiza object catalog`); otherwise CDS Sesame (Simbad, NED, VizieR), whose answers are cached under `director/sesame` in the cache root, hits for ninety days and misses for a day. Returns the resolved `name`, ICRS `ra_degrees` and `dec_degrees` and the `source`; `404` when no catalog knows the name. A catalog position, not a pointing solution. |
| GET | `/sky/objects` | `ra` and `dec` in ICRS degrees, `fov` (stage width in degrees), optional `aspect` (width over height, 4:3 by default), `at` (Unix milliseconds, now by default), `limit` (300 by default, at most 1000), `limit_mag` (16 by default), `catalogs` (comma-separated catalog prefixes to keep in `objects`, matched against the letters a designation starts with; every catalog when absent) and `hide` (prefixes to leave out; `PGC,HD` when neither is given, nothing when `catalogs` is, empty to hide nothing). Answers the marks within the stage's reach: `catalogs` and `hidden` (the prefixes applied), `objects` from the Seiza object catalog by prominence after the catalog filter, `minor_bodies` (comets and asteroids from its minor-body catalog at `at`, brightest first, with motion and a tail or motion direction), and `solar_system` (Sun, Moon and planets from the built-in ephemeris). Each catalog layer says whether it is `available` on the server and, if not, why. |
| GET | `/sky/cutout` | `survey` (an `id` from the list), `ra` and `dec` in ICRS degrees, `fov` (image width in degrees, 0.02 to 180), optional `width` and `height` in pixels (64 to 2048, default 1024) and `rotation` in degrees east of north. A cached image answers `200 image/jpeg`. A miss starts one fetch and answers `202` with `Retry-After: 1`; poll the same URL. A failed fetch answers `502` with the reason for about a minute. |

Cutouts are tangent-plane JPEGs cached under `<cache>/director/sky/` by
survey, center, field, size and rotation, so a framing session that returns to
the same view works without the network. The server only ever calls the one
provider with an allowed HiPS identifier and bounded sizes; it is not a URL
proxy. Imagery is attributed to its survey in the response list and remains a
composition aid, not evidence of pointing, transparency or coverage.

## Automatic workload policy and exchange

These experimental routes use the existing rig/database identity. Commissioning
is interactive editor access only; paired credentials and PATs cannot approve
a policy. The commissioning UI is not implemented yet.

`GET /api/director/v1/rigs/{rig}/workload-policy` returns the policy or null.
`PUT` on that route uses:

```json
{
  "coordinator_instance_id": "<instance UUID>",
  "expected_revision": 0,
  "policy": {
    "rig_id": "<rig UUID>", "catalog_id": "<catalog UUID>",
    "client_id": "<paired client UUID>", "profile_id": "<NINA profile UUID>",
    "profile_revision": 2, "configuration_id": "<reviewed configuration ID>",
    "project_ids": ["<active Director project UUID>"],
    "enabled": true, "revision": 1
  }
}
```

The new revision must equal `expected_revision + 1`. Up to 128 distinct active
projects are allowed. A changed reviewed profile/configuration requires a new
policy. All compiled goals must belong to the allowlist; activating an unrelated
project does not expand authority. Disabling intake does not erase issued work
or retroactively stop a disconnected executor.

Paired `POST /rigs/{rig}/workloads/request` takes `coordinator_instance_id`,
`catalog_id`, a durable `request_id` UUID, `configuration_id`, and
`execution_mode` identifying the executor's supported behavior. Prepared mode
requires one target. Local sequence mode accepts multiple targets for the
on-rig Rust core to prioritize using live NINA constraints; native sequence
hooks own target setup. Both refuse rotation and Director-owned
centering/dithering before a new grant is stored. The `*_v2` variants add Moon
rules; `prepared_target_v3` and `local_sequence_v3` also accept observing policies.
`native_single_target_v1` accepts one target with native preparation and
`native_imaging_v1` accepts multiple targets. Both native modes accept dithering,
Moon rules and observing policies; requested rotation requires centering enabled.
The client additionally requires its configured rotator connected. These modes
do not bypass reviewed equipment identity, commissioning, one-shot launch, or
local safety/geometry checks. Historical retry replies are
also checked against the requested capability. Local target switches do not
require a workload request or coordinator check-in. Persist the
UUID before sending. The response data contains `request_id`, `state`,
`workload`, and `retry_after_seconds`. `issued` returns a workload containing
the unchanged allocation envelope, `released: false`, and null `ledger_id` and
`terminal_sequence`. `waiting` returns null workload and a 30-second retry
delay; it grants no launch. Retrying a released UUID returns its original
allocation and terminal ledger/cursor, with state `released`. Only after
validating that history may a client advance its request UUID. The separate
`allocation/start` remains one-shot and must never be retried on an ambiguous
response. An unreleased grant blocks a different request UUID even after expiry.

Paired `POST /rigs/{rig}/workloads/release` takes `coordinator_instance_id`,
`catalog_id`, `allocation_id`, `ledger_id`, `terminal_sequence`,
`operations_quiescent: true`, and `parked: true`. These last fields are executor
attestations, not remote verification of a physical mount. The native client
must finish hooks, stop dispatch, verify no active preparation or unresolved
capture, park successfully, and deliver the complete feed before calling.
The server also validates the consumed launch, exact event identities,
contiguous feed and settled capture transitions. Release returns the workload
with `released: true`, the ledger and terminal sequence. Exact retries are
safe; different terminal proof conflicts. Sealed feeds permit exact duplicates
but refuse additional events. Failed or uncertain sessions require future
explicit reconciliation, not this clean-completion path.

Successors preserve spent attempts and pending saved credit. They do not grant
fresh rejection retries, reset goal caps, accept ungraded images, or refresh
material equipment changes. The prepared-target plugin still supports one
target with sequence-owned centering/focus/guiding/flip operations. Multi-target
automatic execution, grade feedback, budget increases, recovery UI and
collaboration are unfinished. History stops intake at 4096 grants per rig;
there is no automatic pruning that could erase outstanding authority.

## Contention and recovery

Storage runs off the asynchronous HTTP worker. The metadata store is SQLite in
WAL mode with one writer and a pool of read-only connections. Reads (plans,
framing, rig profiles, feasibility, mosaic, marks) are served from the pool at
once, beside any write and beside one another; they never queue behind a
write or answer busy for one. Writes take the single writer in turn; one that
has waited twenty seconds for it answers `503` with `Retry-After: 1`, which the
browser retries. The plan list reads each rig database on a read-only
connection with no gate held and builds the list on a pooled reader. It takes
the writer only to record something new (a rig, plan, link, first drafts or
header optics), and opens a rig database for writing only to add its missing
identity table, committed at once, so N.I.N.A. never waits behind it. A rig's
header search, which can scan every image folder, runs outside every gate and
runs again only once more frames arrive. Work that binds or writes the rig
databases (new rigs on the plan list, rig binding, adoption, activation) has a
gate of its own so it does not hold the store. A canceled HTTP request may still commit its already admitted
transaction. Use the same create identity on retry, or GET after an ambiguous
rename result. Errors do not return filesystem paths or raw SQLite diagnostics;
detailed failures are logged locally.
