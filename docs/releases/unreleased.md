# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes
- Keep dewy hours out of night plans. A site's forecast now includes
  temperature, dew point and humidity. In **Settings › Presets › Sites**,
  enter a limit in **No imaging within**: hours whose air comes that many °C
  or closer to its dew point no longer count as clear. In the Nights grid,
  a drop marks nights the dew limit cut short. A night's timeline shows a
  dew band under the cloud band, and the legend lists the hours inside the
  limit. Like cloud, dew changes planning estimates, not the
  rig's safety controls.

- When a reverse proxy refuses a remote worker's upload as too large, the
  worker and the failed run now say so and point to the proxy's upload
  limit, such as nginx's `client_max_body_size`. The workers guide shows
  nginx settings that pass full-size results.
