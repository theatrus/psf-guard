# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Review Target Scheduler flat coverage in the calibration library and invalidate
  suspect runs with a reason. The NINA plugin applies these requests on grade
  pulls and applied reconciles, preserving changed or newly taken coverage and
  leaving calibration files untouched.

- Inspect the calibration masters used by a mono or color stack from its
  **Masters** action, with session/channel provenance, display stretch,
  native-size zoom, rejection statistics, and unchanged FITS downloads.

- Optional native star masking for flat masters, controlled by **Mask stars
  in flats** in calibration settings. Masked builds retain original unmasked
  measurements, report low coverage, and refuse unsupported pixels without
  silently smoothing or filling them. No external star-removal tool is needed.

## Changed

## Fixed

- Sky-flat masters improve rejection of overlapping moving-star samples with
  robust median/MAD clipping after calibration and brightness normalization. Existing
  generated masters rebuild with the new algorithm when next needed. Stars
  need enough drift or dithering to leave most samples at each pixel clean;
  clipping does not guarantee a star-free master.
