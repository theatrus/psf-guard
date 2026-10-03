# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

## Changed

## Fixed

- Frames with plenty of stars no longer score near zero because one
  measurement found none (issue #616). When the quality scan finds no stars
  where N.I.N.A. counted 20 or more, the scan is ignored for that frame
  rather than capping it, and a frame with no stars anywhere is no longer
  called a localized occlusion. A target's frames are now compared on one
  kind of star count, mostly the scan's, rather than a mix of the scan's and
  N.I.N.A.'s, which had dragged down whole nights on rigs where the two
  differ. Max HFR and Min stars still judge each frame on its own count. A
  scan taken from a file that has changed since is measured again.

