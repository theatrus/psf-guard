# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Before upgrading its tables in a scheduler catalog, PSF Guard now copies the
  catalog beside itself (`…psf-guard-backup-v<schema>-<time>.sqlite`), keeps
  the three newest copies, and refuses to upgrade if the copy cannot be
  written.

## Changed

## Fixed

- Choosing a preset or custom folder layout for remote uploads now replaces
  the layout detected from the catalog instead of being filed as its
  fallback, and a date inside a detected folder name (`NIGHT_2025-12-14`)
  becomes `%NIGHT%` rather than pinning every upload to one night.

- Starting 0.9.2 on a large calibration library could take minutes with
  nothing logged: the schema upgrade read every calibration frame's header
  before the server was up. Upgrades no longer read frame files; readout-mode
  names are recorded after startup, in the background, with progress in the
  log.

- The Images tab's Status filter works again: choosing Accepted, Rejected, or
  Pending shows those images instead of an empty grid, the summary line names
  the choice, and links saved with the old numeric value keep filtering.
- Master darks, biases, and flats built by PixInsight or Siril now match and
  calibrate stacks. Their headers drop gain, offset, and temperature, so they
  were refused; they are now matched on what they do record, placed on the
  right scale, and used as-is. A new Calibration matching setting chooses
  whether such a master is preferred, a fallback, or ignored.
