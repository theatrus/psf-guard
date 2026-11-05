# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- Calibration coverage now judges a light's flats by the master flat a stack uses. A matching master from the light's own night no longer shows *can't be calibrated* because raw flats are older. Stacks keep those lights, and exports carry that master instead of the older raw flats.
