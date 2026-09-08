# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Merge alternate target names in Overview and move selected exposures to an
  existing or new target or project from Grid, with a preview before Apply.
  Image files and grades stay unchanged, and Target Scheduler plan counts
  follow the new grouping.

## Changed

## Fixed

- PixInsight master bias files with a blank `FILTER` header now work during
  stacking. The cached master preserves the undefined value without requiring
  edits to the original FITS or XISF file.
