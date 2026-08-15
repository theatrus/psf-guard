# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- How hard each kind of evidence lowers a quality score is now adjustable.
  The **Penalties** control in the Sequence view scales the score hit from
  satellite trails, pointing failures, and temporal anomalies between 0%
  (ignore), 100% (the calibrated default), and 200% (double). The choice is
  remembered and applies to every scoring surface, so grid badges, the
  Sequence view, and the detail panel always agree. The zero-star cap does
  not scale. See
  [screening](https://github.com/theatrus/psf-guard/blob/main/docs/SCREENING.md).

## Fixed

- Stacking a target whose calibration library holds flats from several
  sessions no longer fails with "building master flat". Flat selection now
  keeps one coherent session — frames captured within a day of the
  nearest-matched flat at a compatible sensor temperature — instead of
  mixing, say, a fresh cooled set with an uncooled set from months earlier.
  When a master still cannot be built, the stack proceeds without it and
  the calibration warning names the master that was skipped and the exact
  reason; calibration errors now carry their full cause instead of a bare
  "building master flat". See [calibration
  libraries](https://github.com/theatrus/psf-guard/blob/main/docs/CALIBRATION_LIBRARY.md).
