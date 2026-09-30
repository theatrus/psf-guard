# Navigation plan: one Library, Review and Plan groups, Live on the Sky

Status: agreed 2026-09-29. Step 1 shipped (header regroup, Live chip and
drawer, Rigs and Exposure templates under Settings). Step 2 shipped (the
Library holds every plan; `/director` without a plan redirects there).
Step 3 shipped (the workspace lives at `/plan?plan=<key>`). Step 4 shipped
(Live on the Sky; the header chip opens it and the drawer is gone). Delete this file once every step
below has shipped and [DIRECTOR.md](../DIRECTOR.md) and
[SKY_COVERAGE.md](../SKY_COVERAGE.md) describe the result. The reasoning
sits in [director.md](director.md) under "one list, two groups, Live on the
Sky".

## Goal

Make the header say what the app is: one list of projects, reviewed one rig
at a time and planned one family at a time. A user should find any project
from the Library, open its frames or its plan without changing tabs, and see
whether a rig is shooting right now from anywhere.

Done means: the Planning tab and its second list are gone, the header has a
Library button and two labeled groups (Review, Plan) each led by a picker,
Live is drawn on the Sky and summarised in a header chip, rig setup and
exposure templates live under Settings, and every link that worked before
still lands somewhere sensible.

## Rules the work keeps

- The unit of review is one rig's project. Grid, Detail, Comparison and
  Sequence keep the `db` slug in URL state and read `useScopedDbId`.
- The unit of planning is the family: projects that share a Target
  Scheduler GUID across databases. A project without a GUID is a family of
  one, keyed by database and id, as `projectFamilies` does today.
- The Library merges databases and carries scope so it can hand it back.
- Director stays the name of the plugin, protocol, API and meta store. Only
  visible text and routes change.
- Old routes redirect. No shared link breaks.

## End state

```
PSF Guard   Library  │ Review  ▾ M31 · C925   Images  Sequence │ Plan  ▾ M31 (2 rigs)   Workspace │  Sky      [jobs] [2 rigs · 1 exposing]  Settings  Help
```

- **Library**: rows and families, Show select, search, archive fold, the
  Planning gear on a family head opening its Workspace and on a member row
  opening that rig's Images.
- **Review**: the project picker (families grouped, one row per rig), then
  Images and Sequence. Grey until a project is picked. The picker offers
  "in plan …" to hop to the Plan group.
- **Plan** (Director enabled): a plan picker over families, then the
  Workspace. The picker offers "rigs: …" to hop back to Review.
- **Sky**: the coverage map plus a Live panel: each rig drawn where it
  points now with phase and target, and the dashboard beside the map.
- **Header chip**: `N rigs · M exposing` in the jobs slot, red when a rig
  has gone quiet, opening Sky's Live panel.
- **Settings**: Rigs (setup) and Exposure templates pages.

## Steps

Each step is one PR with its own tests, guide update and release note.

### 1. Header regroup

Scope: `static/src/App.tsx`, `ProjectTargetSelector.tsx`, a new plan
picker, header CSS, Settings navigation. Move `DirectorRigs` and
`TemplateLibrary` to Settings pages. Add the Live chip reading the rig
status query. No route changes.

Accept when: the bar renders the two groups with their pickers and Sky; the
Review group is disabled with a hint when no project is in scope; the chip
shows counts and opens the dashboard (a drawer until step 4); Settings has
Rigs and Exposure templates; the header e2e and the picker unit tests pass.

### 2. Library absorbs the plan list

Scope: `Overview.tsx`, `DirectorPlans.tsx`, `planFilters.ts`,
`DirectorPage.tsx`. Move the Show select, search, "Showing N of M" and the
plan-row actions onto the Library. The family head gets the Workspace link
and the rig-state select the plan rows have. `/director` without
`directorProject` redirects to `/` with the same query.

Accept when: every assertion in the plan-list unit tests holds against the
Library; the Director e2e specs read the Library; a closed-everywhere family
sits under the Library's archive; `directorShow` and `directorSearch` keep
working as Library URL state.

As built: the Library lists only projects with frames, so a plan whose rigs
have captured nothing (or that no database takes yet) would have vanished
with the plan list. Those wait in a **Plans with nothing captured yet**
section after the active projects and before the archive, which also
holds New plan and rename. The
Library's search and Show are `q` and `show`; the old names redirect. Read
-only viewers find rigs and templates in the Live drawer, since they cannot
open Settings. The plan card with its survey thumbnail is gone; the
workspace keeps the framing.

### 3. Workspace by family key

Scope: `ProjectWorkspace.tsx`, `DirectorProjectContext.tsx`, the plan
picker, routing. The Workspace is addressed by family key (`plan=<guid>` or
`plan=<db>:<id>`); `directorProject` redirects into it. The Plan picker
lists families with progress and rig pills from the same components as the
Library rows.

Accept when: a Workspace opened from the Library, from the picker and from
an old `directorProject` link shows the same plan; attach and detach still
work; the Plan picker's unit tests pass.

As built: the route is `/plan`, and the key is the Target Scheduler GUID
when every rig shares one, else Planning's plan id (a plan with no
database, or rigs with different GUIDs after an attach); any rig's GUID
and `slug:row` also resolve. `/director` forwards old links. The plan
picker already drew its rows from the Library's pill components after
step 1.

### 4. Live on the Sky

Scope: `static/src/components/sky/`, `DirectorDashboard.tsx`, the chip.
Draw each rig at its current target from the rig status query, with phase
and a stale marker; mount the dashboard as a panel beside the map; the chip
opens Sky with the panel open. Remove the drawer from step 1.

Accept when: a rig reporting `exposing` on a target appears at that target's
place with its name; a quiet rig is marked stale on the map, in the panel
and in the chip; the Sky e2e spec covers one live rig from a seeded check-in.

As built: the plugin's status carried no position, so the Sky reads a new
optional `pointing` field (ICRS degrees) and falls back to the named
target's centre among the rig's plan targets; the design record's status
row states the contract for the plugin. The panel is a compact list beside
the map, with the full Live table under the timeline and, for read-only
viewers, rigs and templates under it. `e2e-director/live.spec.ts` covers
one rig placed by its target and one by its pointing.

### 5. Remove the Planning page

Scope: delete `DirectorPage.tsx` and the `/director` route, keep the
redirects; update `docs/DIRECTOR.md`, `docs/SKY_COVERAGE.md`,
`docs/README.md`, the docs site, and this file (delete it).

Accept when: no navigation entry or link in the app says Planning; a full
Playwright run passes; `grep -rn "director" static/src` finds only the API
client, routes that redirect, and the Director-named components.

## Tests that guard the whole

- Playwright: header groups at desktop and narrow widths; a shared link to
  `/grid?db=…&project=…`, `/sequence?…`, `/director?directorProject=…`
  and `/director?db=…&project=…` each land on the right view.
- Vitest: pickers group families and hop between groups; the chip's counts
  and stale colour; Library filters and archive with multi-rig fixtures.
- Rust: none expected. Any API change is out of scope for this plan.

## Out of scope, recorded elsewhere

- Grid and Sequence across a multi-rig project: design note in
  [director.md](director.md). Until then the picker groups a family and
  keeps review per rig.
- Exposure templates and rig setup that sync between PSF Guard instances:
  design note in [director.md](director.md).
- Moving targets: design note in [director.md](director.md).

## Risks

- Header space at narrow widths. Two labeled groups plus a chip will not fit
  a phone; the groups collapse to icons with the picker as the label, and
  the chip becomes a dot. Decide this in step 1 with a narrow-width e2e.
- Picker load. The project picker already fetches every project across
  databases; the plan picker must reuse that query, not add a second one.
- Link rot in the docs site, which still names Planning in places.
