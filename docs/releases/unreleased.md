# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- Quality analysis flags frames shot at a different angle than the target's
  planned rotation. A frame whose solved rotation sits more than 5° from the
  plan is flagged Rotation skew with the reason **Rotation off plan**, its
  score is capped at 0.30, rejection is recommended, and stack previews leave
  it out. A meridian flip still matches the plan. This catches a whole night
  or a whole target shot at the wrong angle, which the existing rotation
  checks could not see. If a target's frames are flagged because the plan
  changed after they were shot, set the target's rotation to the angle you
  want.
