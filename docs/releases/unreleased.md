# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- A **Sky** view maps everything every database has pointed at on one
  all-sky chart, equatorial or galactic, with each target drawn as its field,
  coloured by filter mix and brighter with more hours. Hover for the target's
  story, click to open it in Images. A night-by-night timeline per rig, with
  the Moon's phase and an all-rigs total, scrubs or replays the map through
  the nights, and the stat band totals hours, frames, targets, nights, and
  sky area. Constellations and bright stars sit behind the fields; scroll to
  zoom in, drag to spin the sky or turn it as a globe, and each target's
  latest stack preview appears inside its field,
  placed by its plate solve. Save PNG makes a poster with the numbers on it.

## Changed

## Fixed

- Import no longer files another instrument's frames into a catalog. Once
  a catalog has recorded its rig, a light whose telescope matches none of
  its rigs is listed by rig with an example file and left out; the preview
  and the CLI say so, and **Include frames from other rigs** (or
  `--accept-other-rigs`) takes them after a real scope change.
  A remote upload from another rig is refused with the same explanation.
  A SpaceCat61 night had been swept into an Askar 107PHQ catalog this way
  and read as two targets exposed at once.
