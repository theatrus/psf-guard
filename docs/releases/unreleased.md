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

- Stacking a target whose calibration library holds frames from several
  sessions no longer fails with "building master flat". The frames feeding
  each master now cluster by sensor temperature (the stacker's own 1 °C
  gate) and, for flats, into one capture session, with the nearest viable
  cluster building the master — instead of mixing, say, a fresh cooled set
  with an uncooled set from months earlier. When a master still cannot be
  built, the stack proceeds without it and the calibration warning names
  the master that was skipped and the exact reason; a master whose own
  input master failed is skipped rather than built wrong; a flat is no
  longer divided into lights that keep their pedestal (a flats-only
  library previously inverted the vignette into bright edges); and
  calibration errors carry their full cause instead of a bare "building
  master flat".
  See [calibration
  libraries](https://github.com/theatrus/psf-guard/blob/main/docs/CALIBRATION_LIBRARY.md).
