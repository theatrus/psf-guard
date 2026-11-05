# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- When a reverse proxy refuses a remote worker's upload as too large, the
  worker's log and the failed run show which file the proxy refused and
  which upload limit to raise, such as nginx's `client_max_body_size`. The
  workers guide shows nginx settings that pass full-size results.
