# Flexible mosaic panels

Status: built (2026-10). The first round placed panels one by one; the
second made shot panels follow their frames, gave each panel its own camera
angle and a turn of its own, and gave a rig framed on its own the same panel
tools. The open questions at the end are proposals for review. Current
behaviour lives in [DIRECTOR.md](../DIRECTOR.md#mosaic-panels).

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
  `panel_id` is retired as `{id}~{revision}`. A target not yet owned is
  taken over by name, by the panel it stands for (a lone target is `r1c1`)
  only within a quarter of the panel's shorter side, or by place. One with
  frames found further away is left as it is and the panel gets a new
  target, so taking it over never moves frames; the preview says how far
  away it sits.
- **Import.** A Target Scheduler project becomes a framing draft: one target
  is a single panel at its center, targets on a grid become that grid
  (measured on the plane of their mean center), and separate targets are
  drafted as the first alone. A plan that still follows its project keeps
  the minimum overlap, and each panel's lock and shot geometry, when it is
  taken in again; a move in Target Scheduler that would move a locked panel
  ends the following, and the drafts stay as they are.
- **Library.** `mosaic_scope.rs` treats a project as a mosaic when Director's
  record names every target's panel as `r{row}c{column}`, or when the targets'
  coordinates form a full grid. Two live panels at least make a mosaic: a
  single panel beside its retired `{id}~{revision}` target is one target.
  Images, Sequence and Stacks then show the panels together, Stacks in a CSS
  grid by row and column.
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
  "panels": [ { "id": "r1c1", "x": 0, "y": 0, "locked": true,
                "measured": { "center": { "ra_degrees": 38.25, "dec_degrees": 61.47 },
                              "position_angle_degrees": 36.5,
                              "size": { "width_degrees": 2.3, "height_degrees": 1.6 },
                              "source": "solved", "size_source": "solved", "frames": 12 } },
              { "id": "p2", "x": -0.95, "y": 0, "locked": false, "angle_offset_degrees": 6 } ] }
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
- **Shot geometry (`measured`).** A locked panel whose frames were read
  carries the center, camera angle and field they were shot with, where the
  numbers came from, and how many frames they were read from. The panel is
  then drawn, checked and activated as shot, not at its planned place: see
  [Shot panels follow their frames](#shot-panels-follow-their-frames).
- **Turn (`angle_offset_degrees`).** A planned panel may turn from the
  framing's angle on the plane, −180° to 180°, counter-clockwise as the
  camera sees the sky (the way a larger position angle turns). Absent means
  0, and a panel at 0 writes no field. See [Turned panels](#turned-panels).
- **Hard rules** (a save is refused): 1 to 256 panels, distinct ids and
  places, finite offsets, turns within half a turn, shot geometry only on a
  locked panel and with a valid center, angle and size, minimum overlap 0 to
  90%, every panel within 30° of the center (a shot one by where it was
  shot) and the whole mosaic within 30° along each camera axis.
- **Issues** (said, never refused): see the next section. A gap may be
  filled later and a lone panel may be meant.

A grid is now one way to make a layout. `Layout::grid` gives the panels a
rows-by-columns grid makes, and `as_grid` reads a layout back as a grid when
every cell is named and placed as that grid would, so the familiar rows,
columns and overlap inputs still work on a grid.

## Overlap, defined

Overlap is read **per pair, in the camera frame, on the tangent plane at the
framing's center**, from each panel's **outline**: a planned panel is its
rectangle at its place, turned by its turn; a shot panel is the rectangle its
camera covered on the plane about its own center, at its own angle and size,
projected onto the framing's plane. Outlines are convex quadrilaterals, so
two of them intersect in a convex polygon (Sutherland–Hodgman clipping).

- Where two outlines intersect, the shared polygon's extent along each
  camera axis, over the **larger** panel's extent along it, gives the share
  across each axis. Where they do not, the overlap of their extents along
  each axis does, negative for a gap. Outlines that miss although their
  extents overlap on both axes (turned corners passing by) are apart by the
  shortest distance between them, across the axis that distance mostly runs
  along, so two turned panels side by side with sky between them read as a
  gap, not as touching.
- Extents apart along an axis share no area, so such a pair is not clipped,
  and a gap is checked for cover only against panels that reach into it.
  The answers are the same; a 16 by 16 mosaic reads in a few milliseconds.
- The **seam** is across the axis with the smaller share. The pair's
  **thickness** is the shared strip's mean width across the seam (its area
  over its length), over the larger panel's extent; the other share is its
  **run** along the seam. A turned outline makes the strip wider at one end
  than the other, and the mean is what counts.
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

**Across framings.** A rig framed on its own covers the same object as the
shared framing and the other rigs, so its panels are judged with theirs as
one cover of the sky (`coverage_issues` in the core, `readCoverage` in the
browser). The cover is the shared framing while a rig shoots it, and the own
framing of each rig that is on, in the draft's order.

- Two panels of one framing are read as above, in that framing's own camera
  frame and against its own minimum. A thin overlap is said only here.
- Two panels of different framings are read in the first one's frame, every
  outline taken there through the sky. They are joined when they overlap or
  touch: across cameras only the cover counts. Side by side with sky between
  them they are a gap.
- A gap is covered by a panel of any framing, and isolated panels and groups
  apart are read over every panel. A rig's own panels therefore need not
  touch where another framing fills the sky between them.
- One framing alone reads exactly as before.

The framing view lists the issues that touch the framing being edited, and
groups apart, with another framing's panels given as `shared r1c1` or
`{rig} p2`; a drop or a turn springs back on the same reading. Activation
lists every issue of the cover as a plan warning, a rig's own panels under
its name. `POST /framing/preview` reads one framing and stays as it was.

For panels of one size on the camera's axes this is exactly the first
round's rule, `1 − |aₓ − bₓ|` of a panel width across a vertical seam, so
every layout reads as it did. Shares are of the larger panel, so each of two
panels of different sizes shares at least the minimum of its own side. The
browser runs a port of the same code (`panelShape.ts`), held to fixtures the
core writes, one of them a shot panel beside planned and turned ones.

One honest limit, the same as N.I.N.A.'s framing assistant: the tangent
plane enlarges sky away from its center, which widens the real overlap of
outer panels a little. Keep the minimum at 10% or more for wide mosaics.

## Shot panels follow their frames

A panel's frames say where it really is. When the camera behind a rig
changes, its old panels keep the field they were shot with and new panels
use the new field; a locked panel is drawn where its frames were taken.

**Where the numbers come from**, best first, for each framing panel whose
target has frames that are not rejected (`GET /projects/{id}/mosaic`
returns them under `measured`):

1. **Fresh pixel-derived solves** (`solved`): the quality scan's persisted
   plate solves of the frames, only those that still match their file and
   only real solves (hinted or blind), not embedded WCS. Up to the newest 60
   frames. The center is the median of their centers on the tangent plane at
   their mean center, so one stray solve does not pick the plane; the camera
   angle is the median of their position angles, read from each solution's
   CD matrix (the field rotation the sequence analysis uses, turned to sky
   handedness), each folded by a half turn onto their own axis (about their
   axial mean) so frames either side of a meridian flip agree wherever the
   plan points, then given on the half nearer the planned angle; the field is
   the median of pixel dimensions times the solved scale. On real N.I.N.A. 3
   frames from rotator rigs, the angle read this way is the sky angle the
   rotator reported, to within a degree or a half turn, so the sign is
   N.I.N.A.'s and Target Scheduler's.
2. **The frames' headers** (`header`): up to eight newest frames' FITS
   headers, each read once per version of its file (its size and
   modification time); a read that fails is tried again next time. The center from an embedded TAN solution's image middle, else the
   mount's RA and Dec; the angle from the embedded solution, else the
   rotator's reported angle (`RotatorPosition`, when a rotator was connected),
   else the planned angle; the field from the header's pixel scale and
   dimensions.
3. **The target as planned** (`plan`): Target Scheduler's own coordinates
   and rotation, when they are J2000; a target kept in JNow gives none.

The field (`size_source`) is the solves', else the headers', else the size
the panel was measured with before, else the plan's panel. The last two keep
a panel's size steady when the plan moves on to a new camera before any
frame was read. Where several rigs shot one shared panel, the rig with the
better source and then more frames speaks for it.

**Prediction or evidence.** Only `solved` is evidence of where the pixels
point; `header` and `plan` are what the mount and the plan said. The source
travels with the numbers to the activation preview, so a prediction is never
shown there as a solve. The sky does not say it.

**Saving it.** Only the server reads frames, so it writes the shot geometry
itself: on every save that keeps a panel locked, the panel's `measured` is
the server's reading of its frames at that moment, keyed by the stored
draft's framings, whatever the body sent; a panel whose frames can no longer
be read keeps the geometry it had, and an unlocked panel carries none. A
browser that read the mosaic a minute ago therefore cannot be refused for
it. The framing view locks a panel with frames and copies the reading onto
it to draw it, as its own quiet choice; it reads the mosaic again after
every save, after a refused one and on Reload. Geometry that moves a
panel is a layout change, since neighbours' overlaps change: a center more
than half an arcminute away, an angle more than 0.05°, or a side more than
a thousandth longer or shorter. A night's new solves move a median by
arcseconds, and activation keeps a shot target until it is a quarter of a
panel away (4.5′ for a 0.3° field), so finer changes are the same place to
activation and do not move `layout_revision`; the reading is still saved.
A reading that appears when a panel is locked, or goes when it is
unlocked, is the server's own and no move while it sits within a quarter
of the panel's shorter side of where the panel is planned. A plan that
follows its Target Scheduler project keeps following through both, and so
does a new frame count.

**Locks.** A shot panel is held as shot: a new panel size does not move it,
so only a planned locked panel off the center holds the size. Its planned
place, the framing's center and angle, and its turn still hold while it is
locked.

## One camera angle per panel on the sky

Every panel of a framing is laid out on one tangent plane, along the camera's
axes at the framing's angle θ. Away from the center's meridian, north at a
panel's own center leans against the plane's north, so a camera there at θ
would draw turned against its neighbours. Activation writes each panel's
Target Scheduler rotation as the angle that keeps its up along θ (plus its
own turn) on the plane:

    PAᵢ = θ + turnᵢ + γᵢ,    γᵢ ≈ Δαᵢ · sin δ

where Δαᵢ is the panel's right ascension from the center and δ its
declination: 3° of right ascension at +60° leans 2.6°. γ is zero on the
center's meridian, opposite either side of it, and negative in the south.

The core computes PA exactly (`camera_angle_at`), not by the first-order
sum: with P the panel's center in the center's (east, north, out) frame and
d = (sin θ, cos θ) the direction to draw, the tangent at P that the gnomonic
projection draws along d is T ∝ P × w with w = (P_z d_y, −P_z d_x,
P_y d_x − P_x d_y), and PA is T's position angle at P. The gnomonic plane
does not keep angles, so for θ away from 0° and 180° the exact angle and
θ + γ differ, by a few hundredths of a degree three degrees out.

**Tests.** γ for panels 3° apart in right ascension at +60° is 3 · sin 60°
within 0.01°, positive to the east, antisymmetric, zero on the meridian,
negative in the south, and equal to the bearing of a 1e-6° step north on the
plane; a camera turned by the exact angle draws its up along θ to 1e-7°,
the first-order sum within 0.1°, and without the turn it draws off by −γ.
A footprint's corners round-trip through the plane and back. The browser
port matches the core's angles in all three fixtures.

**N.I.N.A.** N.I.N.A.'s framing assistant shares one rotation across a
mosaic by default; its *Preserve alignment* option (added with its framing
rework, Bitbucket PR #1233) gives each panel its own rotation far from the
equator, so the panels form one rectangle on the sky. Director now does the
latter for every planned panel.

**Reading back.** A Target Scheduler project taken in as a framing reads its
angle back with the lean taken off (`plane_angle_at`, the exact inverse):
targets whose rotations agree on one plane angle read as that angle. Targets
that share one stored rotation, as N.I.N.A. and activations before this
change wrote them, keep it.

**Frames already shot.** A target with frames whose stored rotation is the
framing's angle plus its turn, with no lean (what activations wrote before
this change), keeps that rotation, so new frames match the old. A locked
panel with frames keeps its target's coordinates and rotation outright: its
target reads `unchanged`, "kept where its frames were shot".

**Every rig gets the same angles.** Planning always sets each panel's
rotation, whatever turns the rig's camera: a rotator, a camera turned by
hand between sessions, or a fixed one. Activation never reads the rig
profile's rotation mode, and its preview never says how to reach an angle;
that is execution's concern. A test activates the same framing for a
rotator, a manual and a fixed rig and reads the same preview and the same
angles written, to the bit.

## Turned panels

A planned panel may turn on its own (`angle_offset_degrees`). Its outline
turns with it, so overlaps are read as drawn, and activation writes
PA = θ + turn + γ for every rig.

- **On the sky.** The chosen unlocked panel has a turn handle past its top
  edge along its own up; dragging it turns the panel about its middle, read
  on the framing's plane. The shared strips and their shares follow live. A
  turn settles within 2° on 0 (the framing's angle), on a neighbour's turn,
  or on how a shot panel's outline leans. A turn that leaves a thin overlap,
  a gap or a lone panel the layout did not have springs back with the
  reason; Alt (Option) turns it anyway, flagged, as a drag does.
- **Keys.** `[` and `]` turn the chosen panel a degree (Shift: ten), by the
  key's place, so a keyboard layout or Shift does not change them. They
  clashed with nothing.
- **Panel editor.** **Turn** takes a number, **Framing angle** puts it back to
  0, and a line gives the panel's camera angle here, the framing's turned by
  north's lean and its own turn.
- **Locks.** A locked panel keeps its turn: a shot panel's angle is its
  measured angle, and a save that would turn a locked panel is refused like a
  move (`422`).
- **Rigs.** Every rig gets the turned panel's angle, whatever turns its
  camera. Planning sets the rotation; reaching it, by a rotator or by hand,
  is execution's concern, so neither the editor nor the activation preview
  says how.
- **Undo.** Each turn is one step, like a move.

## Rigs framed on their own

A rig framed on its own has its own center, angle, size and layout, and the
same tools: while its card's editor is open, **Mosaic panels** works on its
panels (the legend names the rig), the sky draws them in the rig's colour
with choosing, dragging, turning, the + handles, keys and undo, and the
shared framing stays in sight, plain. Its panels lock by their own frames
(the targets of that rig's own project) and by hand, its locked panels hold
its center and angle in the editor, and the server holds them on save as it
does the shared framing's. Done goes back to the shared framing; a rig that
is off keeps its framing where it is, drawn dim. Activation writes its panels
into that rig's own Target Scheduler project, each at its own angle.

## Locks

A panel is **locked** two ways:

- **by its frames**: a panel whose target has captured frames, read from the
  rigs that shoot that framing (the shared one, or a rig's own). The framing
  view locks it in the draft as its own quiet choice, saved with the next
  save, unless the panel is `unlocked_by_hand`;
- **by hand**: the person locks or unlocks any panel. Unlocking a panel with
  frames sets `unlocked_by_hand`, so the view does not lock it again; locking
  it clears the flag, and the server clears it on any locked panel. A shot
  panel moved more than a quarter of its size, or turned more than a degree
  (`MAX_TURN_IN_PLACE_DEGREES`), gets a new target at activation, so
  unlocking it never moves or turns its frames. A grid relayout and *Take
  values* keep the flag by panel id.

A locked panel's center and camera angle on the sky cannot move. The server
refuses a save (`422`, naming the panels) that moves or turns a panel locked
in both the stored draft and the body, or moves the framing's center, angle
or panel size under it. That includes the shared center or angle a rig's own
framing follows (its `center` or angle is `null`), and a panel size given or
taken away (a rig framing's typed size dropped back to its rig's field). The
refusal says what holds: "Locked panel r1c1 would move. A locked panel keeps
its place, its turn, and the center, camera angle and panel size it was
placed with, including a shared center or angle a rig's own framing follows.
Unlock it first.", naming the rig for a rig's framing. Unlocking and locking
are edits of their own, so a body may unlock and move a panel in one save, or
place a panel and lock it there; unlocked, a panel is planned again and
drops its shot geometry. A panel at the framing's center does not move with
a new panel size, nor does a shot panel, so only a planned locked panel off
the center holds the size.

The browser keeps a locked mosaic's center, angle, target search and rig
choice disabled with a line saying why, hides the rotation handle, and offers
no regrid. Dragging the rectangle, or the sky in *Sky* mode, looks around
instead of moving the held center. A rig framed on its own that follows the shared center or angle
holds them the same way while it has a locked panel, and its editor will not
send it back to the shared center or angle, move it to the view center, or
change its size while that would move one; taking a center or angle of its
own where it is moves nothing and stays open. A panel that turns out to have
frames after it was moved here (the mosaic was still being read) goes back
to its saved place as it locks, with a note. Activation keeps its backstop:
a panel moved more than a quarter of its size that has frames gets a new
target.

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
- **Against real edges.** New panels, the + handles and dragging meet a shot
  panel at its real outline: a panel beside it shares the minimum of the
  wider of the two sides. Against a shot panel turned off the camera's axes,
  a new panel is drawn in until its outline shares the minimum.
- **Drag.** In *Panel* drag mode a drag moves one panel. Each axis settles,
  within 14 screen pixels, on a place lined up with another panel or at the
  minimum overlap beside one. While dragging, the panel's shared strips are
  shaded with their share. Beside a turned panel, or as a turned panel, the
  settled place is drawn in until the outlines share the minimum, as adding
  does. A drop that leaves a thin overlap, a gap or a lone panel the layout
  did not have springs back with the reason; holding Alt (Option), or
  **Place anyway** in the note, places it anyway, flagged. The panels
  falling into groups counts only when the move split joined panels, not
  when fixing the last issue shows groups that were already apart. Settling
  stays on with Alt: a drag across the screen bends a hair off the camera's
  axes, so a free drop would land a fraction of a percent short of its
  neighbour; the arrow keys place a panel finer. The drop is checked where
  the last move put the panel. One pointer drags at a time: a second finger
  or a pen is ignored until it lets go, and a cancelled touch, a lost
  capture, Escape or a move with no button held (the release was lost) puts
  the panel back. The handles take a press within 22 CSS pixels and are
  drawn larger on a small stage or a touch screen.
- **Keyboard.** With a panel chosen and the sky focused: arrow keys nudge it
  1% of a panel along the camera axis nearest the arrow's way on screen
  (Shift: 10%), `[` and `]` turn it a degree (Shift: 10°), `L` locks or
  unlocks it, Delete removes it, Escape lets go, Ctrl+Z and Ctrl+Shift+Z (or
  Ctrl+Y) undo and redo panel edits. `[`, `]` and `L` go by the key's place,
  so any keyboard layout has them. A key pressed on one of the buttons over
  the sky is that button's; only undo, redo and Escape during a drag reach
  the panels from there.
- **Undo.** Every panel edit is one step, fifty deep, for this visit; the
  save bar's *Discard*, **Back to saved framing** and a framing saved
  elsewhere start the history afresh.

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
- **Schema 30.** Shot geometry and turns are optional fields a panel without
  them never writes, so nothing is rewritten: a schema 29 store opens
  byte for byte the same. The bump keeps a build that cannot read the new
  fields (panels refuse unknown fields) from opening a store that may hold
  them. Tests open a schema 29 store with drafts unchanged, round-trip shot
  geometry and turns, refuse shot geometry on an unlocked panel, and read
  every older shape as before.

## Target Scheduler mapping

What Target Scheduler sees does not change shape:

- one project per rig, as before, `isMosaic = 1` once it has more than one
  panel. The flag holds no geometry; with a Mosaic Completion weight it
  favours the panels that are furthest behind, which suits new panels beside
  a long-shot one;
- one target per panel the rig shoots, at the panel's center, J2000, named
  `{target} {panel id}` (`M31 p2`), with its own rotation: the framing's
  angle plus the panel's turn and north's lean at its center. A framed target
  that becomes a mosaic keeps its name and its row: activation reads it
  `unchanged`, and a locked panel with frames keeps its target's place and
  rotation;
- exposure plans per panel as before. Nothing is written that Target
  Scheduler cannot read, and optional columns are probed as before. The
  preview says each target's angle and what it is made of, such as "angle
  62.6° (framing 60.0°, north here +2.6°)", "angle 10.4° (framing 0.0°,
  turned +7.8°, north here +2.6°)" or "as shot, from solved frames". Every
  rig gets the same angles, rotator or not.

What PSF Guard keeps on its side:

- the layout (ids, places, turns, locks, shot geometry, minimum overlap) in
  the meta store's framing draft;
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
  warnings in its preview, never as a refusal, read across every framing
  that rigs shoot.

## Workflow and UI

1. **A framed target.** The Framing tab shows the plan's panel. If it has
   frames, it is drawn solid in the locked ink with a padlock where its
   frames say it was shot; a lone panel carries no name, and a mosaic's
   names are small. The list says "40 frames", **Unlock** frees it, and the
   center, angle and target search say the locked panels hold them.
2. **Make it a mosaic.** In *Mosaic panels*, choose *Whole mosaic* or a panel
   (click it on the sky or in the list), then a direction on the pad: E adds
   a column east, N a row north. *Ring around* surrounds it.
3. **Place by hand.** Pick *Panel* among the sky's drag modes and drag a
   panel. It settles at the minimum overlap; the shared strips show their
   share in green, or in the warning colour when short. The + on each side
   and corner of the chosen panel adds a panel there.
4. **Turn a panel.** Choose it and drag its turn handle, press `[` or `]`,
   or type a turn; the editor gives its camera angle here.
5. **Mistakes.** A drop or a turn short of the rule springs back and says
   why; Alt places it anyway, dashed in the warning colour and listed under
   the panel list. Undo and Redo sit beside the list.
6. **Save and activate.** The save bar lists what changed ("panels p2, p3
   added", "panel p4 turned", "r1c1 locked"); activation creates the new
   targets, each at its own angle, and leaves the shot one alone.
7. **A rig on its own.** *Frame separately* or *Edit* on a rig's card opens
   its own panels in the same tools. Picking another rig to size a shared
   mosaic of more than one panel, when its field differs, asks first: frame
   it separately, or use its field for the shared mosaic anyway. Its panels
   need not touch each other; the cover across framings is what is checked.

States are drawn from data attributes (`data-panel-state='locked' |
'invalid'`, `data-selected`, `data-shot`, `data-framing`, `data-rig-index`)
and ARIA (`aria-pressed`, `aria-invalid`) with tokens scoped on `.framing`:
`--framing-panel-locked`, `--framing-panel-chosen`, `--framing-panel-invalid`
and `--framing-overlap`; a rig's own panels take its `--framing-rig-N`
colour as `--framing-panel`.

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
- **Overlap of the axis-aligned boxes of turned panels.** Cheap, but a turned
  panel's box overstates what it covers, so a thin seam would pass. The
  polygon intersection costs little at these counts.
- **Per-panel angle as an absolute position angle.** A turn relative to the
  framing keeps meaning when the whole mosaic turns, and 0 is the common case
  that writes nothing.
- **Shot geometry stored only on the server side.** The browser needs it to
  draw and check overlaps live, and the lock check needs it in the draft; the
  server's own reading on save keeps it honest.

## Built and proposed

Built, first round: the layout model and its checks in the core and the
browser; schema 29 and older bodies; locks by hand and by frames, held by the
server; extending beside a panel or the whole mosaic, ring and fill;
dragging with snapping, shared strips and refusal; keyboard and undo; the
add handles; activation into separate targets with layout warnings; the
Library reading Director panels that form no grid; frames of a linked
project counted against their panels before activation.

Built, second round: overlap read from polygon outlines in the core and the
browser; shot panels drawn from their frames (solves, then headers, then the
plan) with the source on the sky, refreshed as solves land and checked by the
server on save; schema 30; each panel's camera angle with north's lean at
activation, read back on import, and kept for targets already shot; turned
panels with the turn handle, `[` and `]`, snapping, spring-back and undo,
with every rig given the same angles whatever turns its camera; the same
panel tools for a rig framed on its own.

Built, third round: another rig picked for a shared mosaic of more than one
panel asks before its field moves the panels; gaps, lone panels and groups
apart read across every framing on the sky, in the core, the browser and
activation.

Proposed, not built:

- a ghost of the panel a + or a direction would add, shown on hover;
- importing a free layout from a Target Scheduler project without
  Director's record (it still reads only grids).

## Open questions

1. ~~**Per-panel camera angle.** A rotator rig could shoot each panel at the
   angle that suits it. Worth it, or keep one angle per framing and frame such
   a rig on its own?~~ Answered: built. Each panel is shot at the framing's
   angle plus north's lean at its center, and may be turned on its own.
2. ~~**Mixed fields.** If the camera changes, should old locked panels keep
   their old field while new panels use the new one (a size per panel)?~~
   Answered: locked panels follow their original shots, not a new camera;
   new panels use the rig's current field.
3. ~~**Names.** Panels added since a grid are `p{n}` and their targets
   `{target} {panel id}`; should they read otherwise?~~ Answered: naming is
   fine.
4. ~~**Unlocking a shot panel.** Panels with frames cannot be unlocked or
   removed in the editor. Should there be a deliberate "retire this panel"
   that keeps its target and frames and frees the place?~~ Answered: a shot
   panel can be unlocked by hand and stays unlocked (`unlocked_by_hand`).
5. **`isMosaic` for irregular layouts.** Keep it on for every layout of more
   than one panel (as now), or offer a switch per plan?
6. ~~**Rigs without a rotator and north's lean.** Such a rig shoots every
   panel at its camera's angle unless someone turns it, so outer panels of a
   wide, northern mosaic would turn by −γ against the plane. Keep one angle
   for such a rig, or warn past a lean?~~ Answered: planning always sets each
   panel's rotation, and reaching it, by a rotator or by hand, belongs to
   execution. Every rig gets the same angles, and neither the editor nor the
   activation preview says how to reach them.
7. **Default minimum overlap.** Grids default to 20%. Is that right for free
   layouts, and should the minimum differ between edges and corners?
