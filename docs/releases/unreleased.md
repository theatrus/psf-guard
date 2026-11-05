# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- When a reverse proxy refuses a remote worker's upload as too large, the
  worker's log and the failed run show which file the proxy refused and
  which upload limit to raise, such as nginx's `client_max_body_size`. The
  workers guide shows nginx settings that pass full-size results.
- See N.I.N.A. reported weather and the site's forecast separately under
  **Live rigs** and a rig's **Weather** tab. Director Sessions can opt in to
  weather holds between exposures through their existing PSF Guard connection,
  including a centrally configured paid forecast source and the site's dew limit.
  A forecast whose location differs from the rig's cannot clear a weather hold;
  the Weather tab explains a location mismatch or missing coordinates.
- Calibration coverage now judges a light's flats by the master flat a stack
  uses. A matching master from the light's own night no longer shows *can't be
  calibrated* because raw flats are older. Stacks keep those lights, and exports
  carry that master instead of the older raw flats.
