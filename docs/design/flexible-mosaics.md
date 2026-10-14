# Flexible mosaic panels

Status: model, migration, API, panel editor and activation built
(2026-10); the open questions at the end are proposals for review.
Current behaviour lives in [DIRECTOR.md](../DIRECTOR.md#mosaic-panels).

## The problem

A plan's framing used to hold a mosaic as rows, columns and one overlap.
That grid was the stored shape: panel ids were places in it (`r1c2`), and
the grid was always centered on the framing's center. Three things followed.

- A framed target that already has frames cannot grow into a mosaic. Setting
  1 × 2 re-centers the grid, so the shot panel moves half a step, and
  activation has to give it a new target and turn its plans off.
- Panels can only be added a whole row or column at a time, and only as a
  rectangle.
- Growing or turning a grid moves panels to other sky under the same ids.

The person wants to keep the shot panel where it is, add panels around it in
any direction, and drag panels by hand, with the minimum overlap enforced.

## What there was

Findings from the code before this change, for the record.

- **Framing draft.** One JSON payload per plan in the meta store's
  `framing_draft` table (schema 28): target name, center, camera angle,
  `mosaic {rows, columns, overlap_percent}`, the panel rig and size, compared
  rigs, the survey and view, `rig_framings` (a rig framed on its own with its
  own center, angle, grid and size) and `layout_revision`, the revision that
  last changed what activation writes. Saves are compare-and-set.
- **Core geometry.** `director-core::framing` lays the grid out on the tangent
  plane at the framing's center, along the camera's axes: row 1 at the top,
  column 1 at the camera's left (east at angle zero), stepped by the
  unoverlapped share of the panel. Every panel shares the camera angle. The
  browser draws with a port of the same code, held to a fixture the core
  writes.
- **Plan.** Each rig's contribution lists the panel ids it shoots; an empty
  list means every panel.
- **Activation.** One Target Scheduler project per rig, `isMosaic` set when
  there is more than one panel, one target per panel the rig shoots, named
  `{target} {panel id}` (a single panel keeps the plain name), at the panel's
  center with the framing's angle, and exposure plans per panel. PSF Guard
  records each target's panel in the rig database's
  `psf_guard_director_target (target_guid, project_guid, panel_id,
  framing_revision)`. A panel that moved more than a quarter of its size and
  has frames gets a new target; the old one keeps its frames, and its
  `panel_id` is retired as `{id}~{revision}`.
- **Import.** A Target Scheduler project becomes a framing draft: one target
  is a single panel at its center, targets on a grid become that grid
  (measured on the plane of their mean center), and separate targets are
  drafted as the first alone.
- **Library.** `mosaic_scope.rs` treats a project as a mosaic when Director's
  record names every target's panel as `r{row}c{column}`, or when the targets'
  coordinates form a full grid. Images, Sequence and Stacks then show the
  panels together, Stacks in a CSS grid by row and column.
- **Target Scheduler's mosaic mode.** A project's `isMosaic` flag holds no
  geometry. Its only effect is the *Mosaic Completion* scoring rule (weight 0
  by default), which favours the targets of a mosaic project that are least
  complete relative to the project's average, to balance the panels.
  N.I.N.A.'s framing assistant makes grids and sends them as targets; Target
  Scheduler never sees rows or columns.

## The panel model

A framing's panels are a **layout**: a minimum overlap and a list of panels.

```json
{ "min_overlap_percent": 20,
  "panels": [ { "id": "r1c1", "x": 0, "y": 0, "locked": true },
              { "id": "p2", "x": -0.8, "y": 0, "locked": false } ] }
```

- **Place.** `x` and `y` are the panel's offset from the framing's center on
  the tangent plane at that center, along the camera's axes, in panel sizes:
  `x` in panel widths toward the camera's right (west at angle zero), `y` in
  panel heights toward its up (north at angle zero). The core turns them
  into sky centers exactly as it laid out a grid, so a grid's panels land
  where they always did.
- **Why panel sizes, not degrees.** A grid steps by a share of the panel, so
  a new panel size re-spaces it and keeps every overlap. Offsets in panel
  sizes keep that: choosing another rig's field keeps the overlaps the
  person set. A locked panel off the center would move under a new size,
  so a locked panel off the center holds the size (below).
- **Id.** Stable for the panel's life. A grid names its cells `r{row}c{column}`;
  a panel added since is `p{n}`, numbered on from the count of panels (a
  panel added to a 2 × 2 is `p5`). Ids never change when a panel moves, so
  contributions, activation records and Target Scheduler names stay attached.
  Ids are letters, digits, `-` and `_`, at most 32, so they fit the
  activation record and the target name, and never contain the `~` that
  marks a retired panel.
- **Center.** The framing's center is the plane's anchor, not the mosaic's
  middle. Adding panels on one side does not move it, so nothing already
  placed moves either.
- **Rotation: one camera angle per framing.** Every panel of a framing shares
  the camera angle, as every panel shares the panel size. Reasons: a rig
  without a rotator shoots every panel at the angle its camera sits at; a
  locked panel fixes that angle for the panels around it; overlaps stay the
  overlap of two rectangles on the same axes, and turning the whole mosaic
  never changes one. A rig that needs another angle or size is framed on its
  own, as before. Per-panel angles for rotator rigs are an open question.
- **Hard rules** (a save is refused): 1 to 256 panels, distinct ids and
  places, finite offsets, minimum overlap 0 to 90%, every panel within 30°
  of the center and the whole mosaic within 30° along each camera axis.
- **Issues** (said, never refused): see the next section. A gap may be
  filled later and a lone panel may be meant.

A grid is now one way to make a layout. `Layout::grid` gives the panels a
rows-by-columns grid makes, and `as_grid` reads a layout back as a grid when
every cell is named and placed as that grid would, so the familiar rows,
columns and overlap inputs still work on a grid.

## Overlap, defined

Overlap is read **per pair, in the camera frame, on the tangent plane at the
framing's center**, where the panels are laid out. For panels at `a` and `b`:

- across `x`: `1 − |aₓ − bₓ|` of a panel width; across `y`: `1 − |a_y − b_y|`
  of a panel height. Negative is a gap.
- The **seam** is across the axis with the smaller value; that value is the
  pair's **thickness**, and the other is its **run** along the seam.
- Two panels are **joined** when the thickness is at least the minimum (side
  by side, or at a corner, as diagonal neighbours of a grid are).
- Two panels sit **side by side** when the run is at least half a side. Such a
  pair that is not joined is a **thin overlap** (thickness between 0 and the
  minimum; touching counts as 0%) or, with uncovered sky between them, a
  **gap**, when the gap is narrower than a panel and no other panel covers it
  (sampled at points inside the gap). A corner that overlaps by less than the
  minimum is no seam and is left alone.
- A panel joined to nobody is **isolated**; groups of joined panels that never
  meet are **disconnected** (said only when nothing else explains it).

Because every panel of a framing shares the camera angle, these numbers do
not change when the mosaic turns; a test lays panels out at several angles
and measures the overlap of their sky footprints in the camera frame.

Two honest limits, the same as N.I.N.A.'s framing assistant: Target
Scheduler gives every panel the framing's angle at its own center, and at a
high declination local north turns across a wide mosaic, which narrows the
overlap at its far corners a little. The tangent plane also enlarges sky away
from its center, which widens the real overlap of outer panels. Keep the
minimum at 10% or more for wide mosaics.

## Locks

A panel is **locked** two ways:

- **by its frames**: a panel whose target has captured frames, read from the
  rigs that shoot the shared framing. The framing view locks it in the draft
  as its own quiet choice, saved with the next save, and it cannot be
  unlocked or removed there;
- **by hand**: the person locks or unlocks any other panel.

A locked panel's center and camera angle on the sky cannot move. The server
refuses a save (`422`, naming the panels) that moves a panel the body still
marks locked, or moves the framing's center, angle or panel size under it.
Unlocking is an edit of its own, so a body may unlock and move a panel in one
save. A panel at the framing's center does not move with a new panel size,
so only a locked panel off the center holds the size.

The browser keeps a locked mosaic's center, angle, target search and rig
choice disabled with a line saying why, hides the rotation handle, and offers
no regrid. Activation keeps its backstop: a panel moved more than a quarter
of its size that has frames gets a new target.

## Adding and placing panels

- **Extend in a direction.** N, NE, E, SE, S, SW, W and NW are sky
  directions. Each adds on the panel side that faces nearest that way at the
  camera angle: a cardinal direction picks the nearest side, a diagonal the
  nearest corner. At angle zero north is up and east the camera's left.
  - **Beside the chosen panel:** one panel there.
  - **Whole mosaic:** a panel at the end of every row (for east and west) or
    every column (north and south), so a ragged mosaic grows without gaps; a
    diagonal grows both ways and fills the corner.
  - The overlap is the minimum or any higher value the person sets; a place
    already taken is said rather than doubled.
- **Ring around** adds a column each side, then a row above and below across
  them: a lone framed target becomes the middle of a 3 × 3.
- **Fill holes** fills the bounding rectangle of panels that sit on one
  lattice at the minimum overlap (counted from a locked panel, else the
  first); panels on no common lattice have no hole to name.
- **Grid.** While nothing is locked, a grid shows its rows, columns and
  overlap, and changing them lays the grid out again. A layout that is no
  grid offers *Lay out as a grid instead*.
- **Drag.** In *Panel* drag mode a drag moves one panel. Each axis settles,
  within 14 screen pixels, on a place lined up with another panel or at the
  minimum overlap beside one. While dragging, the panel's shared strips are
  shaded with their share. A drop that leaves a thin overlap, a gap or a lone
  panel the layout did not have springs back with the reason; holding Alt
  (Option) places it anyway, flagged.
- **Keyboard.** With a panel chosen and the sky focused: arrow keys nudge it
  1% of a panel along the camera axis nearest the arrow's way on screen
  (Shift: 10%), `L` locks or unlocks it, Delete removes it, Escape lets go,
  Ctrl+Z and Ctrl+Shift+Z (or Ctrl+Y) undo and redo panel edits.
- **Undo.** Every panel edit is one step, fifty deep, for this visit; the
  save bar's *Discard* returns to the saved framing.

## Migration

- **Store.** Meta store schema 29 rewrites each `framing_draft` payload, in
  the same transaction as the version bump: the shared `mosaic` and each rig
  framing's `mosaic` become the `layout` that grid makes, under the grid's own
  ids and in the same places. Nothing else in the payload changes, and the
  row's revision and `layout_revision` stay, so no open editor is told its
  draft is stale and no activation is asked for.
- **What it refuses to touch.** A payload that does not parse, or that would
  not read back as a sound draft, or that already has both shapes, is left
  byte for byte, so a damaged record stays the one record that cannot be
  read. Opening again changes nothing.
- **Older bodies.** A body or stored payload that still has `mosaic` is read
  as the grid it describes, so an open browser tab from before the upgrade,
  an MCP client or a backup restored into a store mid-way all read the same
  panels. `POST /framing/preview` takes either. A body with both is refused.
- **Rig databases.** Nothing changes in Target Scheduler tables or
  `psf_guard_director_target`: a grid's panels keep their `r{row}c{column}`
  ids, so every recorded target still matches its panel and the next
  activation reads every target `unchanged`.
- **Tests.** Stores at schema 28 with drafts as first stored (no rig
  framings, no layout revision), with a rig framing from before rig framings
  had a center, with the last grid shape, a single panel, a damaged payload
  and a payload with both shapes; a store from schema 25, before draft
  imports; a check that every panel lands within 1e-12° of where the grid had
  it at several sizes, angles and declinations, against the old arithmetic.

## Target Scheduler mapping

What Target Scheduler sees does not change shape:

- one project per rig, as before, `isMosaic = 1` once it has more than one
  panel. The flag holds no geometry; with a Mosaic Completion weight it
  favours the panels that are furthest behind, which suits new panels beside
  a long-shot one;
- one target per panel the rig shoots, at the panel's center with the
  framing's angle, J2000, named `{target} {panel id}` (`M31 p2`). A framed
  target that becomes a mosaic keeps its name and its row: activation reads
  it `unchanged`;
- exposure plans per panel as before. Nothing is written that Target
  Scheduler cannot read, and optional columns are probed as before.

What PSF Guard keeps on its side:

- the layout (ids, places, locks, minimum overlap) in the meta store's
  framing draft;
- each target's panel id in the rig database's `psf_guard_director_target`,
  which Target Scheduler and Sync ignore. The Library reads these records to
  show the panels as one mosaic whatever their shape: a grid's ids are its
  cells, and other panels are placed in the rows and columns their targets'
  coordinates fall on (banded by a quarter of the closest two targets'
  spacing, along the first target's camera axes). A retired panel's old
  target is listed too, since it holds frames. Without Director's record,
  only a full grid of targets reads as a mosaic, as before;
- before a plan's first activation, the framing view counts a linked
  project's frames against the panel each target stands for: a lone target is
  `r1c1` and targets on a grid are its cells, as the framing taken in from
  them names them;
- activation lists layout issues (a thin overlap, a gap, a lone panel) as
  warnings in its preview, never as a refusal.

## Workflow and UI

1. **A framed target.** The Framing tab shows the plan's panel. If it has
   frames, it is drawn solid in the locked ink with a padlock, the list says
   "40 frames", and the center, angle and target search say the locked
   panels hold them.
2. **Make it a mosaic.** In *Mosaic panels*, choose *Whole mosaic* or a panel
   (click it on the sky or in the list), then a direction on the pad: E adds
   a column east, N a row north. *Ring around* surrounds it.
3. **Place by hand.** Pick *Panel* among the sky's drag modes and drag a
   panel. It settles at the minimum overlap; the shared strips show their
   share in green, or in the warning colour when short. The + on each side
   and corner of the chosen panel adds a panel there.
4. **Mistakes.** A drop short of the rule springs back and says why; Alt
   places it anyway, dashed in the warning colour and listed under the panel
   list. Undo and Redo sit beside the list.
5. **Save and activate.** The save bar lists what changed ("panels p2, p3
   added", "r1c1 locked"); activation creates the new targets and leaves the
   shot one alone.

States are drawn from data attributes (`data-panel-state='locked' |
'invalid'`, `data-selected`) and ARIA (`aria-pressed`, `aria-invalid`) with
tokens scoped on `.framing`: `--framing-panel-locked`,
`--framing-panel-chosen`, `--framing-panel-invalid` and `--framing-overlap`.

On a phone the panel editor stacks under the sky; pad buttons and the panel
list are tap targets, and *Panel* drag mode works with a finger.

## Alternatives considered

- **Absolute sky centers per panel.** Simplest for locks, but a new panel
  size or a turned mosaic would leave gaps everywhere, and the grid could no
  longer be read back. Offsets in panel sizes keep both.
- **Offsets in degrees.** The same problem when the panel size changes.
- **Overlap by area, or on each pair's own tangent plane.** Area mixes a thin
  long seam with a fat corner; per-pair planes differ from where activation
  places the panels. Edge thickness on the framing's plane is what a
  registration tool needs and what the layout controls.
- **A Target Scheduler project per panel.** Loses the mosaic flag and the
  shared project settings, and the Library would need a new grouping.
- **Per-panel rotation.** See the open questions.

## Built and proposed

Built: the layout model and its checks in the core and the browser; schema
29 and older bodies; locks by hand and by frames, held by the server;
extending beside a panel or the whole mosaic, ring and fill; dragging with
snapping, shared strips and refusal; keyboard and undo; the add handles;
activation into separate targets with layout warnings; the Library reading
Director panels that form no grid; frames of a linked project counted
against their panels before activation.

Proposed, not built:

- a ghost of the panel a + or a direction would add, shown on hover;
- the same panel tools for a rig framed on its own (it keeps its grid
  inputs; a layout made by API is listed as "placed one by one");
- importing a free layout from a Target Scheduler project without
  Director's record (it still reads only grids);
- per-panel rotation and per-panel size (below).

## Open questions

1. **Per-panel camera angle.** A rotator rig could shoot each panel at the
   angle that suits it. Worth it, or keep one angle per framing and frame such
   a rig on its own?
2. **Mixed fields.** If the camera changes, should old locked panels keep
   their old field while new panels use the new one (a size per panel)?
3. **Unlocking a shot panel.** Panels with frames cannot be unlocked or
   removed in the editor. Should there be a deliberate "retire this panel"
   that keeps its target and frames and frees the place?
4. **`isMosaic` for irregular layouts.** Keep it on for every layout of more
   than one panel (as now), or offer a switch per plan?
5. **Default minimum overlap.** Grids default to 20%. Is that right for free
   layouts, and should the minimum differ between edges and corners?
