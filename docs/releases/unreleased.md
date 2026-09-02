# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

## Changed

## Fixed

- The Images tab's Status filter works again: choosing Accepted, Rejected, or
  Pending shows those images instead of an empty grid, the summary line names
  the choice, and links saved with the old numeric value keep filtering.
- Master darks, biases, and flats built by PixInsight or Siril now match and
  calibrate stacks. Their headers drop gain, offset, and temperature, so they
  were refused; they are now matched on what they do record, placed on the
  right scale, and used as-is. A new Calibration matching setting chooses
  whether such a master is preferred, a fallback, or ignored.
