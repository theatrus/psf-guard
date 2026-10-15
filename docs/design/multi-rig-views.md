# Multi-rig views

Status: **Proposed**. Nothing is built. The answers in §7 are proposals the
user has not confirmed; settle them before phase 1.
Last updated: 2026-10-15

This record plans views across rigs. It extends
[Multi-database architecture](multi-database.md) and borrows from
[Director](director.md) and [Flexible mosaic panels](flexible-mosaics.md).

## 1. Problem

One target is often shot by more than one rig: Heart and Soul on the Askar 107
and the Esprit 100, or a mosaic whose panels Director splits between rigs. The
person wants to review, grade, compare, sequence, stack and calibrate it as one
piece of work.

The Library, Director and the project picker already treat these rigs as one
family. Every view after the picker shows one rig at a time: the header
switches rigs but never shows them together, and sorting, grouping, scoring
and stacking all assume one rig.

Terms:

- **Rig**: a registered database, usually one N.I.N.A. install. A
  *calibration rig* (`psf_guard_rig`) is narrower; one database can hold
  several.
- **Family**: the projects that stand for one piece of work across databases.
  Each **member** is one `(db, project)` pair; the **anchor** is the member
  the URL's `db` and `project` name.
- **Night**: one observing night, split at the catalog's night boundary
  (`catalog_night_boundary` and `night_of` in `src/server/sky_coverage.rs`),
  not at UTC midnight.

## 2. Current state

- **Header.** `ProjectTargetSelector`
  (`static/src/components/ProjectTargetSelector.tsx`) shows a family as one
  "N rigs" row (`projectSelector/ProjectFamilyOption.tsx`) whose entries each
  open one rig. `RigTargetSelect` (`components/header/RigTargetSelect.tsx`)
  offers one `(db, project, target | mosaic)` per option, grouped by rig; no
  option spans rigs.
- **URL.** `static/src/hooks/useUrlState.ts` holds `db`, `project`, `target`
  and `mosaic=1`. `useScopedDbId` returns `null` on merged routes; `scopeKey`
  drops scoring params when `db` or `project` changes.
- **Two family rules.** The Library (`components/libraryFamilies.ts`), the
  picker (`utils/projectTargetNavigation.ts`) and `RigTargetSelect` off
  `/plan` use "one project GUID in at least two databases". On `/plan`,
  `RigTargetSelect` uses the plan's links (`header/useCurrentPlan.ts`). A
  project attached to a plan from another database keeps its own GUID, so it
  is a sibling rig in Planning and a stranger everywhere else.
- **Precedent.** `mosaic=1` widens scope without dropping `db`. It groups
  Images by panel (`groupImagesByPanel` in `utils/imageGrouping.ts`), lists
  Sequence across panels (`MosaicSequence.tsx`), and keeps grading,
  calibration and stacking per panel (`src/server/mosaic_scope.rs`).

| View | Across databases today |
|---|---|
| Library, Sky | Yes, through `useMergedPerDb` and `useMergedSkyCoverage` (`hooks/useDatabases.ts`). |
| Planning | Yes: rigs, framings, activation and finished stacks per rig. |
| Images (`GroupedImageGrid.tsx`) | No. One `['db', dbId, 'all-images', projectId, targetId]` query. |
| Detail, Compare | No. Image IDs are per-database integers; `ImageComparisonView` takes one `dbId`. |
| Sequence (`SequenceView.tsx`), Stacks (`stacks/StacksView.tsx`) | No. One database, one target or `mosaic=1`. |
| Calibration (`calibration/CalibrationPage.tsx`) | No. `useCalibrationHealth` fans out for the Library only. |
| Scoring (`src/sequence_analysis.rs`), colour (`src/server/stack_preview/color.rs`) | No. One database each. |

## 3. Decision

Choose **Option C**: one widening flag, `rigs=all`, that works like
`mosaic=1`. `db` and `project` stay in the URL and name the anchor. Images,
Sequence and Stacks widen to every member of the anchor's family. Each write
names its frame's own database; selection and batch actions act on the anchor.

```
[Heart and Soul · 2 rigs ▾] [Planning] ┊ [All rigs · all targets ▾] [Images] [Sequence] [Calibration] [Stacks]

RigTargetSelect, open:
  ┌──────────────────────────────────┐
  │ All rigs · all targets           │  ← rigs=all
  │ All rigs · Heart mosaic (4)      │  ← rigs=all&mosaic=1
  ├ Askar 107 ───────────────────────┤
  │ Askar 107 · all targets          │
  │ IC 1805                          │
  ├ Esprit 100 ──────────────────────┤
  │ Esprit 100 · all targets         │
  │ IC 1848                          │
  └──────────────────────────────────┘
```

Not chosen:

- **A: a family scope above the project** (`/grid?family=<key>`, `db`
  optional). Every scoped view changes at once, `selected=` becomes
  `(db, id)` pairs, and the `db` rule that guards writes gets weaker. Merged
  lists also invite interleaving rigs, which reads as noise (§4.4).
- **B: one rig per view, faster switching.** Smallest change, but the person
  still cannot see both rigs' frames, nights or stacks together. C takes B's
  cross-rig Compare (phase 2).

## 4. The model

### 4.1 Family

`useRigFamily(db, project)` returns members as `(db, project)` pairs: projects
in other databases that share the anchor's GUID (trimmed, any case), plus,
when Director is on, the projects linked to the same plan (`catalog_slug` and
`source_row_id` from `GET /api/director/v1/plans`). Never names or
coordinates. Members keep a fixed order: the plan's rig order, else database
name A to Z. A family groups views; it is not sync identity. Targets
correspond across rigs only through Director panel ids.

### 4.2 URL state

- `rigs=all` sits beside `db` and `project`, never beside `target`, as
  `mosaic=1` does. `rigs=all&mosaic=1` shows a mosaic's panels across rigs.
- `rigGroup=inner` puts the rig inside the grouping mode; absent, the rig is
  outermost. `grouping` and `order` keep their values.
- The anchor follows the person. Opening, selecting or grading a frame of
  another rig moves `db` and `project` to that rig and keeps `rigs=all`.
- `scopeKey` treats the family as the scope while `rigs=all` is set, so an
  anchor move keeps scoring params. `useScopedDbId` returns the anchor.
- `selected=` and `current=` keep the anchor's integer IDs.

```
/grid?db=askar-107&project=5&rigs=all
/grid?db=askar-107&project=5&rigs=all&grouping=date&rigGroup=inner
/detail/88?db=esprit&project=9&rigs=all
/compare/412/88?db=askar-107&project=5&rightDb=esprit&rightProject=9&rigs=all
```

Images builds one merged list, not one grid per rig. Each member's rows come
from its existing `all-images` query, stamped with `db_id` as `useMergedPerDb`
does, and `db:id` keys React, grouping, navigation and the quality and
calibration-mark maps. Query keys do not change, so switching between one rig
and all rigs is warm. Phases 1-3 need no new endpoint.

### 4.3 Grading and selection

Keyboard grading and the grade buttons act on the focused frame and write
through `useGrading(frame.db_id)`; undo works per database. Selection belongs
to the anchor rig, because every batch action writes one database. Space on
another rig's card moves the anchor and starts a new selection there. Batch
grade, move, export and **Stack selection** act on the anchor. A remote rig's
heading says "copy of <peer>; grades return with Send grades".

### 4.4 Sorting

Order sorts frames within each rig section. Rig sections are groups, and
groups keep their place (#836), so a frame arriving on the Esprit never moves
the Askar's section. `order=newest` turns the frames inside each group
around; a flip at rest lands on the first card of the first rig section
(`hooks/useReorderAnchor.ts`). Never interleave rigs in one strip: two rigs
on one night alternate every few minutes. Time across rigs goes on a shared
axis instead: night, then rig in Images, and one lane per rig in Sequence.

### 4.5 Grouping

Rig is a placement around the modes in `types/grouping.ts`, not a new mode.
By default each rig is a section with the chosen mode inside it ("how is each
rig doing?"). With `rigGroup=inner` the mode is outermost and each group holds
a sub-group per rig: Filter puts the Askar's Ha beside the Esprit's, and Date
lines one night up across rigs.

- With a mosaic: panel, then rig, then mode, since Director assigns panels
  to rigs.
- Every group key in a merged list carries `db_id`.
- Date means night, by each catalog's boundary. The label is the evening's
  local date, so two rigs' nights line up by label.
- Filter names stay per rig: "Ha" and "H-alpha" do not merge.
- Sessions stay per rig; night, then rig is how they line up.

### 4.6 Quality scores

Scores compare across rigs only as per-rig ranks: `quality_score` is relative
to its own rig's sequence, so 0.4 means "poor for that rig" on either. HFR
(pixels or arcseconds), star counts, background, SNR and flux depend on
scale, optics, gain and filter, so a quality sort across rigs uses
`quality_score` or a per-rig percentile. Within a rig section, any measure.

Sequence scoring and statistical grading stay per rig; a pooled sequence
marks the coarser rig's frames as outliers. A second rig at the same site may
later label a frame, never reject it: its cloud is sky evidence, not evidence
about this frame's pixels.

### 4.7 Stacking

First, per-rig stacks side by side (phase 3). Building, calibration and
caching stay per rig; no new artifacts.

Later, a **stack of stacks**: integrate each rig on its own, with its own
masters, rejection and weights; then register the integrations onto one grid,
normalise, weight each by its noise at the output sampling, and average
without rejection. That is colour composition's shape, one input per rig. One
`LiveStacker` across rigs would take one set of masters, sigma-clip the
sharper rig's star cores against the softer rig's, and overweight the
upsampled rig. A combined same-filter stack stays out of scope (§8).

### 4.8 Cross-rig colour composites

Ha from the Askar with RGB from the Esprit is the most useful combined
product. It needs registration across scales and a chosen output grid only;
each channel comes from one rig, so weights and rejection do not apply.

- `StackColorRequest` sources name `(db, project, target, filter, artifact
  revision)` instead of one `target_id` in one database.
- The output grid is the finer-sampled channel's, cropped to the covered
  region (`crop_report`).
- Automatic re-composition (`src/server/stack_preview/automatic.rs`) listens
  to every source database's builds.
- Mono and one-shot colour in one channel are refused.

### 4.9 Where cross-database artifacts live

Under the reference input's database:
`<cache>/<ref-db>/stack-previews/color/<job>/`. The manifest lists every
foreign input, and the job key includes each input's database slug, source
fingerprint and artifact revision. Budgets, the janitor
(`src/server/stack_preview/janitor.rs`) and database removal keep working per
database. A foreign input that disappears marks the job out of date. A plan
namespace would give every cache tool a second root and change key whenever a
project is attached or detached.

## 5. Phased plan

**Prerequisites, done in `night-and-rig-grouping-fixes`:**

- Date grouping (`groupGridImages`) used the UTC date; it now uses the
  catalog's night boundary, for one rig too.
- `capture_profile_from_metadata` left out the instrument, so a merged
  catalog scored two rigs' 300 s, gain 100 frames as one sequence.
- `groupImagesBySession` keyed on `target_id` and the project's display name,
  so two databases' sessions could merge.

| Phase | Scope | User-visible |
|---|---|---|
| 0 | `useRigFamily`; scoring split by instrument (done) | Attached projects join families |
| 1 | All rigs in Images | Yes |
| 2 | `rigGroup=inner`, nights across rigs, Detail and Compare across rigs | Yes |
| 3 | Sequence lanes, Stacks by rig, Calibration family chip | Yes |
| 4 | Seiza PR: registration, reprojection, weights | No |
| 5 | Cross-rig colour composites | Yes |

**Phase 0: family hook.** The picker, `RigTargetSelect` and the Library
switch to `useRigFamily` (§4.1). The scoring split by instrument is done in
the prerequisite PR.

- *Files:* new `static/src/hooks/useRigFamily.ts`; `RigTargetSelect.tsx`,
  `utils/projectTargetNavigation.ts`, `components/libraryFamilies.ts`,
  `components/Overview.tsx`.
- *Tests:* a GUID family; a project attached with its own GUID; a lone
  project; Director off; a plan link without `source_row_id`. Extend
  `RigTargetSelect.test.tsx`, `ProjectTargetSelector.family.test.tsx` and
  `Overview.plans.test.tsx`.
- *Done when* the three surfaces list the same members, and an attached
  project shows as a sibling rig off `/plan`.

**Phase 1: All rigs in Images.** "All rigs · all targets" writes `rigs=all`.
Images builds the merged list (§4.2), grouped rig, then mode. Rig headings
show pixel scale (pixel solve or rig profile, labelled), counts and a rig
colour. Grading, selection and sorting follow §4.3-4.4. A frame opens in
Detail in its own database with `rigs=all`; next and previous stay in its rig.

- *Files:* `RigTargetSelect.tsx`, `hooks/useUrlState.ts`,
  `GroupedImageGrid.tsx`, `utils/imageGrouping.ts`,
  `hooks/useReorderAnchor.ts`; `App.css` gains `--rig-N` tokens promoted
  from `--framing-rig-N` in `director/FramingView.css`.
- *Tests:* URL round trip; a grade in the second section lands in its
  database; Space there moves the anchor and clears the old selection;
  scoring params survive the move; equal target IDs in two databases never
  share a group; newest first keeps sections in place. Playwright adds a
  second database through `POST /api/databases`.
- *Done when* a two-rig family is reviewed and graded from one Images view,
  each grade landing in its frame's database.

**Phase 2: grouping and navigation across rigs.** `rigGroup=inner` for
filter, Date and mosaic panel. Date as night across rigs: each member splits
at its own catalog's boundary, and nights line up by label. Detail's next and
previous walk the merged on-screen order. Compare takes `rightDb` and
`rightProject`, grades each side through its own database, and matches sky
scale from fresh pixel-derived solves only.

- *Files:* `utils/imageGrouping.ts`, `hooks/useUrlState.ts`,
  `ImageDetailView.tsx`, `utils/imageDetailRoutes.ts`,
  `ImageComparisonView.tsx`.
- *Tests:* `imageGrouping.test.ts` for each inner placement and for two
  catalogs with different boundaries; Detail stepping across a rig boundary;
  Compare grading each side into its own database.
- *Done when* one night reads across both rigs, and Compare shows an Askar
  frame beside an Esprit frame.

**Phase 3: Sequence, Stacks and Calibration by rig.** Sequence draws one lane
per rig on a shared time axis, each with its own analysis and score chart.
Stacks lists each rig's stacks under its heading; with a mosaic, each panel's
stack per rig on the sky overview. Calibration stays one database at a time;
with a family in scope its chip reads "Heart and Soul · 2 rigs ✕" with one
segment per member, above a read-only `useCalibrationHealth` summary.

- *Files:* `SequenceView.tsx`, `MosaicSequence.tsx`, `stacks/StacksView.tsx`,
  `stacks/MosaicStacks.tsx`, `calibration/CalibrationPage.tsx`,
  `hooks/useDatabases.ts`.
- *Tests:* lanes keep per-rig scores; Stacks headings per rig; a Calibration
  segment moves the anchor and opens that member's calibration rig.
- *Done when* Images, Sequence and Stacks all have an All rigs form, and
  nothing pools frames across rigs.

**Phase 4: Seiza groundwork.** Solve-seeded registration (a starting
transform from both images' WCS, refined on stars); reprojection onto a
caller-chosen TAN grid, SIP-aware for wide fields; sampling-aware weights for
combining resampled integrations.

- *Workflow:* a Seiza worktree and a reviewed Seiza PR. For testing, PSF
  Guard pins that PR's git SHA in every Seiza crate whose types cross the
  boundary. The user ships the Seiza release; PSF Guard moves back to
  released versions before its own PR merges.
- *Tests:* Seiza unit tests, plus real frames from two rigs near 1.04″/px and
  1.41″/px registering within 1 px RMS.
- *Done when* a Seiza release carries the three APIs.

**Phase 5: cross-rig colour composites.** §4.8 and §4.9. Before phase 4
ships, a raised Seiza `scale_tolerance` with a clear failure message may
serve as a first step.

- *Files:* `stack_preview/color.rs`, `stack_preview/automatic.rs`,
  `stack_preview/janitor.rs`, `components/stackColorSources.ts`,
  `components/StackColorPreviewPanel.tsx`.
- *Tests:* the job key changes with any input's revision; a missing foreign
  input marks the job out of date; the janitor keeps a job whose inputs live
  in two databases; a mono and a colour channel are refused.
- *Done when* Ha from one rig and RGB from another compose into one preview,
  cached under the reference input's database.

## 6. CLAUDE.md amendments

Each lands with its phase, not in this record's PR.

1. **Phase 0, GUID rule.** "A rig family is GUID kin plus Director plan
   links. It groups views; it is not sync identity. Targets correspond across
   rigs only by Director panel id."
2. **Phase 1, `db` slug rule.** "`rigs=all` widens Images, Sequence and
   Stacks to the anchor's family. `db` and `project` name the anchor, which
   selection and batch actions act on. Every image a view opens or grades
   names its own `db`; `useScopedDbId` returns the anchor." Phase 2 adds
   "Compare names a database per side."
3. **Phase 1, image evidence.** "Scores, statistical grading and capture
   profiles compare frames within one rig. Across rigs, sort by per-rig
   scores, never raw HFR, star counts or ADU."
4. **Phase 5, artifacts below the slug.** "An artifact built from several
   databases lives under its reference input's slug; its cache key includes
   every input's slug and source fingerprint."

## 7. Open questions

1. **What makes rigs one family?** Proposed: GUID kin across databases plus
   Director plan links. Never names.
2. **Rig outermost or innermost by default?** Proposed: rig, then mode. It
   matches the Library and mosaics, and sections stay still as frames arrive.
3. **Does Date mean night everywhere?** Proposed: yes, as Calibration and
   AstroBin export already do. The prerequisite PR makes it so.
4. **Can one selection span rigs?** Proposed: no. A selection belongs to the
   anchor rig, because every batch action writes one database.
5. **Is a combined same-filter stack in scope?** Proposed: not now. Per-rig
   stacks first, then cross-rig colour after the Seiza PR, then reconsider.
6. **Can a target be narrowed across rigs?** Proposed: only by Director panel
   id. Without a panel record, picking a target leaves `rigs=all`.
7. **Two rigs in one database?** Proposed: keep members as `(db, project)`
   pairs, rely on the instrument split in scoring, and defer view support.
8. **Where does picking a family land?** Proposed: Images, Sequence and
   Stacks open on All rigs; Calibration opens on the rig with the newest
   frames.

## 8. Out of scope for now

- Combined same-filter integration across rigs. Reconsider once phase 5
  proves the registration path; until then, WBPP.
- Stitching mosaic panels across rigs into one image. Panels stay side by
  side, as Planning draws them.
- One selection spanning rigs, with `(db, id)` keys in the URL.
- Cross-rig sky evidence beyond an advisory label.
- Folding filter aliases across rigs; the AstroBin filter map could serve.
- An "all databases" grid. The merged view stops at one family.
