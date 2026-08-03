# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

This release adds optional server accounts, cropped and gradient-corrected
color stacks, and a Sequence view that keeps its place as you grade.

## Added

- Optional server user accounts, with roles, a browser management UI, and a
  login page. See [authentication](../AUTHENTICATION.md).
- A **Stop** button on a queued or running stack build. The stop lands between
  frames, between channels, and between calibration masters. Nothing partial is
  published, and channels that already finished keep their previews.
- A **Stacking** indicator in the header for as long as any mono or color
  build is queued or running, in every view and across databases. Stack panels
  re-attach to a running job when they mount, so leaving the grid or reloading
  no longer hides work in flight.
- Source-frame artifact search: select a region of a stack and see the frames
  that contributed it, with the shape classified and localized quality findings
  highlighted.
- Automatic gradient correction for stack previews.
- An **Edges** picker on each color card: keep the blank edges left by
  registration, trim to the box the covered pixels span, or trim to the largest
  rectangle every channel covers in full. The default is unchanged. A completed
  card reports the kept size and, when one channel bounded the crop, names it.
- A thumbnail zoom control in Sequence, remembered across reloads.
- Quality analysis can be started from the image grid, and now appears only
  where it is needed.

## Changed

- Stack previews keep the rotation of their reference frame instead of being
  reprojected, and are turned half a turn when that frame faces the wrong way
  across a meridian flip. Which way is right is read from the reference frame's
  cached solve or embedded WCS, then its `PIERSIDE` header, then the stack's
  own exposure — so every channel of a target agrees rather than each following
  its own frames. The shared north-up, east-left grid is still available to an
  API caller through `north_up: true`, which is what a mosaic needs. Dropping
  the required plate solve also made a three-frame build about twenty times
  faster.
- A color composite crops before normalization, so every channel is scaled
  from the same patch of sky, and its FITS keeps the reference plate solution.
- Sequence navigation matches the image grid: the same arrow keys, the same
  Space selection, and a grid selection carried into Sequence.
- Sequence tabs wrap instead of scrolling sideways.
- Sequence quality scores renormalize when evidence is missing, so a session
  is not punished for a scan that has not run, and scores stay consistent
  across sessions and projects.
- A satellite trail warning needs pixel evidence, not just a predicted pass.
- Background grading uses the solved nebulosity of a field.
- The Overview states its grading progress and status more plainly, and the
  selection marker no longer sits over the quality score.
- The closed project picker names its database under the project name.

## Fixed

- Opening the Overview no longer loses the project you were working in.
- Color background extraction retries without catalog protection when a
  protected fit fails. The processing stack also lets you turn protection off.
- Closing an image detail returns to the view that opened it, at the position
  it was left, including a partly visible Sequence card.
- Background protection follows the stack's own geometry.
- Flagged Sequence thumbnails stay bright.
- Shift-click range selection keeps a stable anchor.

## Security

- A server bound to anything other than loopback now answers `401` to every
  API request until an operator adds a user, and refuses to start if it was
  also asked for database management. Previously a server with no accounts
  treated every caller as a full editor, and the default bind is `0.0.0.0`.
  Remote sync and image upload are unaffected — their bearer keys carry their
  own proof. `--allow-anonymous-access` restores the old behavior for a
  trusted network, and logs what it gives away.
