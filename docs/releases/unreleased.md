# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Changes

- A quality scan you start while another scan runs on the same database
  now waits its turn and starts on its own, instead of not starting. Scans
  you start run before the database-wide quality analysis, which pauses its
  current target for them and scans that target again afterwards.
- The background jobs list in the header now shows calibration grading
  while it measures new calibration frames, and calibration health checks
  while they work through your projects.
- The background jobs list also shows when the Calibration page reads
  light headers for a project, with how many it has read, and when it
  matches lights to flat groups.
- The background jobs list also shows a catalog's own upkeep while it runs:
  building PSF Guard's query indexes, reading the readout modes of older
  calibration frames, and pairing calibrated copies with their lights. A
  database you add or change gets its query indexes and readout modes at
  once, rather than at the next server start.
- On a server started with preview pre-generation, the background jobs list
  shows **Generating previews** for a database while it writes previews,
  with how many images it has done. **Stop** ends it for that database
  until the server restarts. While a job you started runs, pre-generation
  now waits and continues where it was, instead of skipping that round.
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

- A build you start stops a running automatic build, which goes back in the
  queue behind your work and resumes from its checkpoint, instead of being
  cancelled and rescheduled. Automatic builds always wait behind the builds
  you queued, and the header lists them as queued automatic builds.
- Automatic reprocessing after a refresh no longer waits for every other
  build to finish: it waits behind your work and behind builds of its own
  target only.
- A queued stack build or color preview reads its frames, grades and
  calibration again when its turn comes. Grades you change while a build
  waits are used, and when the frames changed, the card follows the new
  build.
- Sync, import and export previews now run as soon as you ask, and a newer
  preview replaces one still running. In **Data Transfer** and **Remote PSF
  Guard**, you can change the options while a preview runs: the
  running preview stops and you can preview again at once. Before, the form
  stayed locked until the preview finished, and a second import preview was
  refused while the first ran. An import you start also replaces an import
  preview that is still running.
- The background jobs list in the header now shows previews, imports,
  exports to the export folder, artifact searches and planning sent to a
  remote site while they run. Previews, imports, exports, artifact searches
  and star catalog installations have **Stop**. Stopping an import rolls it
  back. Stopping an export keeps the files already placed, and the next
  export skips them.
- An artifact search now waits its turn in the stack queue like a build,
  ahead of automatic work.

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
