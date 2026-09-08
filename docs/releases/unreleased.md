# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Color-stack processing now offers RC-Astro tools per linear input channel,
  including saved tool steps and separate starless/stars FITS downloads.

- Merge alternate target names in Overview and move selected exposures to an
  existing or new target or project from Grid, with a preview before Apply.
  Image files and grades stay unchanged, and Target Scheduler plan counts
  follow the new grouping.

## Changed

## Fixed

- Stack previews now overlap frame preparation within the configured worker
  and memory budgets instead of silently preparing frames sequentially.
  Builds that fit serial processing but cannot fit a preparation queue keep
  working with one frame at a time. Batch logs expose execution mode, worker
  counts, memory fallback, and stage timings.

- Completed stacks revisit early admitted samples to reject bright transient
  trails, with pass/frame progress and consistent results after resuming a
  build. Low-coverage pixels and overlapping trails can still retain artifacts.

- Scheduler merge no longer duplicates frames that an import or remote upload
  had already added before the telescope's own rows arrived: the existing row
  is recognized by target, file name, and capture time, updated in place, and
  takes the telescope's identity, keeping any grade it already had.

- Applied stack processing survives reloads, restoring the cached result,
  RC-Astro versions, and editor settings. Reverting also survives reloads.

- PixInsight master bias files with a blank `FILTER` header now work during
  stacking. The cached master preserves the undefined value without requiring
  edits to the original FITS or XISF file.
- A calibration frame that was imported, then moved by a filer and imported
  again no longer fails every master with `duplicate calibration input`: the
  import updates the row's path instead of adding a twin, and stacking hands
  a file to the integrator once even when two rows point at it.
