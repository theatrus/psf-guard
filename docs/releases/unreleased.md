# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- You can pause a database's quality analysis, preview pre-generation, flat
  measuring and frame-site reading from the header's jobs list. **Pause**
  keeps the work done so far, and the job stays in the list as **Paused**
  until you choose **Resume** or the server restarts. Quality scans you start
  still run while the analysis is paused, and Settings shows the paused
  analysis with **Resume**. Over MCP, use `pause_job` and `resume_job`.
