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

## Fixes

- When grading leaves a channel with fewer than two usable frames, its card
  no longer shows the stack built from the frames you rejected, and a color
  composite stops offering that stack as a source. A channel whose files are
  missing, or that has no matching calibration, keeps its stack.
