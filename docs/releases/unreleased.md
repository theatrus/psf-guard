# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- A **Sky** view maps everything every database has pointed at on one
  all-sky chart, equatorial or galactic, with each target drawn as its
  field, coloured by filter mix and brighter with more hours. Hover for the
  target's story, click to open it in Images. A night-by-night timeline per
  rig, with the Moon's phase and an all-rigs total, scrubs or replays the
  map through the nights, from a start night you choose, and the stat band
  totals hours, frames, targets, nights, the sky covered once with overlaps
  counted once, the fields added together, the pixels the sky resolves into
  at the finest scale that reached each patch, and the longest night of any
  one rig. Each catalog's nights split at the quietest hour of its own day,
  so a rig imaging past noon UTC is not cut in two. Stars to magnitude 5.5
  and the constellations sit behind the fields; scroll to zoom in, drag to
  spin the sky or turn it as a globe, and each target's latest stack preview
  appears inside its field, placed by its plate solve. Save PNG makes a
  poster with the numbers on it.

## Changed

## Fixed

- The Sky view remembers its frame, shape, turn, zoom, and cuts for the
  browser session, so coming back from another view lands on the same sky,
  and its coordinate labels stay the same size on screen instead of growing
  with the zoom.

- The Overview loads in well under a second per catalog again. Its counts,
  spans, and filter lists read each image row beside its metadata, so a
  large catalog on a network share cost several seconds per request; four
  wider indexes now answer them without touching the table (26,570 page
  reads became 226 on a twelve-thousand-frame catalog), the grid's
  newest-first page no longer sorts the whole table, and each catalog keeps
  a larger page cache. The indexes are created once when the catalog is next
  opened writable; a read-only catalog works as before.
- The flat Sky map no longer slides a fixed picture around when zoomed in.
  It is the sky seen from inside the sphere: a drag turns it sideways and
  up or down at every zoom, zoom is about the centre of the view, and the
  projection follows the turn, so a field near the pole is no longer sheared
  when you zoom onto it.

- Import no longer files another instrument's frames into a catalog. Once
  a catalog has recorded its rig, a light whose telescope matches none of
  its rigs is listed by rig with an example file and left out; the preview
  and the CLI say so, and **Include frames from other rigs** (or
  `--accept-other-rigs`) takes them after a real scope change.
  A remote upload from another rig is refused with the same explanation.
  A SpaceCat61 night had been swept into an Askar 107PHQ catalog this way
  and read as two targets exposed at once.
