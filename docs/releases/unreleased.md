# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Remote image receivers can match an existing catalog folder tree or use a
  preset/custom target, date, frame type, filter, camera, and exposure layout,
  while keeping the existing flat layout as the default.
- Reject limits like N.I.N.A. subframe selection: set Max HFR and Min stars
  in the Sequence view's Scoring control (or `screen-fits --max-hfr` /
  `--min-stars`) and any frame over the HFR limit or under the star limit
  is recommended for rejection, whatever the rest of the sequence looks
  like.
- Satellite-trail checking can be turned off: set the satellite penalty to
  0% (or pass `screen-fits --ignore-satellites`) and trails no longer lower
  scores or drive reject recommendations, while still showing as warnings.

## Changed

## Fixed

- Calibration matching now tells readout modes apart on cameras whose
  driver names the mode (N.I.N.A. writes `READOUTM = 'High Gain Mode'` rather
  than a number). Before, every frame from such a rig recorded no readout
  mode and a High Gain dark could serve an Extend Fullwell light. The name is
  kept on import, shown in the calibration library, and read off existing
  frames' headers the first time an older library is opened.
- Stack previews now remember the scoring settings used to admit their frames.
  Changing a penalty or absolute reject limit marks the preview out of date,
  gives the rebuild its own cache identity, and prevents it from resuming a
  checkpoint whose frame decisions came from the old settings.
- Remote scheduler sync and export now wait for transient SQLite locks instead
  of failing immediately while N.I.N.A. or another server task is using the
  catalog.
- Remote uploads now retain a durable path and content identity, restore
  missing files on an identical retry, and avoid attaching same-named frames
  to the wrong scheduler target.
- Replacing an uploaded image now invalidates its rendered, quality,
  astrometry, and satellite evidence, including star and HFR values PSF Guard
  previously wrote back, without scanning the full cache tree.
