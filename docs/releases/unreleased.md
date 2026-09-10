# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

## Changed

## Fixed

- Sky-flat masters reject overlapping moving-star samples with robust
  median/MAD clipping after calibration and brightness normalization. Existing
  generated masters rebuild with the new algorithm when next needed. Stars
  fixed on the same pixels still require drift or dithering during capture.
