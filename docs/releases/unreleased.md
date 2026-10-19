# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Changes

- When Seiza chooses a stack's reference frame and the plate solves show
  frames at more than one rotator angle, the reference now comes from the
  largest group. Before, a reference from a small group could reject every
  frame of a large one for rotation, and a color composite then showed blank
  corners where that channel was rotated against the others. The new **Orientation group
  margin** in **Settings › Stacking** sets how far inside Seiza's 10° limit
  a frame must sit to count toward a group; the default is 1°. Stacks
  already built keep their reference until their frames change or you click
  **Rebuild**.
- While a database copies WBPP masters to its process directory, queued WBPP
  runs for other databases now start when PixInsight is free. Before, every
  queued run waited for the copy to finish. Queued runs for the copying
  database still wait, and keep their place in the queue.

## Fixes

- When grading leaves a channel with fewer than two usable frames, its card
  no longer shows the stack built from the frames you rejected, and a color
  composite stops offering that stack as a source. A channel whose files are
  missing, or that has no matching calibration, keeps its stack.
- A color preview you build while its target's channels are still queued or
  building now waits for those builds and uses their new stacks. It used to
  fail because no stacks were finished yet, or start from the old stacks and
  then fail when a channel was rebuilt before it ran. The color card shows
  **Waiting for the channel builds to finish** meanwhile, and the
  composition survives a server restart.
