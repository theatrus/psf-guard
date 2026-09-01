# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Remote image receivers can match an existing catalog folder tree or use a
  preset/custom target, date, frame type, filter, camera, and exposure layout,
  while keeping the existing flat layout as the default.

## Changed

## Fixed

- Remote scheduler sync and export now wait for transient SQLite locks instead
  of failing immediately while N.I.N.A. or another server task is using the
  catalog.
- Remote uploads now retain a durable path and content identity, restore
  missing files on an identical retry, and avoid attaching same-named frames
  to the wrong scheduler target.
- Replacing an uploaded image now invalidates its rendered, quality,
  astrometry, and satellite evidence, including star and HFR values PSF Guard
  previously wrote back, without scanning the full cache tree.
