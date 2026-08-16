# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Remote image intake now accepts bias, dark, dark-flat, and flat frames as
  well as lights. Calibration uploads enter PSF Guard's calibration library
  without creating Target Scheduler image rows, and identical retries remain
  idempotent.
- How hard each kind of evidence lowers a quality score is now adjustable.
  The **Penalties** control in the Sequence view scales the score hit from
  satellite trails, pointing failures, and temporal anomalies between 0%
  (ignore), 100% (the calibrated default), and 200% (double). The choice is
  remembered and applies to every scoring surface, so grid badges, the
  Sequence view, and the detail panel always agree. The zero-star cap does
  not scale. See
  [screening](https://github.com/theatrus/psf-guard/blob/main/docs/SCREENING.md).
- Stack previews gained a **Calibration** control: **Auto** (the default)
  applies the safe masters, **Force on** applies every master that can be
  built — including a flat with no bias or dark master, which Auto holds
  back — and **Off** stacks the raw frames. The choice is remembered, and a
  preview built under a different mode is marked out of date. See [stack
  previews](https://github.com/theatrus/psf-guard/blob/main/docs/STACKING_PREVIEWS.md).
- Hot pixels are suppressed where they actually do damage. Flat masters get
  a spatial defect pass after integration — a defective pixel repeats in
  every flat, survives statistical rejection, and used to divide a
  fixed-pattern artifact into every light (a 26-megapixel test flat carried
  nearly ten thousand of them). Stacks with no dark master run the same
  impulse filter over each calibrated light. Dark masters keep their hot
  pixels, which subtract the lights' own. Existing flat masters rebuild
  once. See [calibration
  libraries](https://github.com/theatrus/psf-guard/blob/main/docs/CALIBRATION_LIBRARY.md).
- Multi-night stacks now calibrate each night with its own masters. A stack
  group whose lights match different calibration sets — per-night flats
  above all — used to stack uncalibrated with a warning; it now partitions
  into sessions, builds each session's masters, and swaps them in as it
  integrates. The stack card shows the session count. See [calibration
  libraries](https://github.com/theatrus/psf-guard/blob/main/docs/CALIBRATION_LIBRARY.md).
- The stack **Calibration** control can now be overridden per channel: each
  target-and-filter card has its own select, so one channel with a bad flat
  can stack raw while the rest calibrate. Overrides are remembered and the
  card's out-of-date marker follows the mode the channel actually uses.
- A flats-only calibration library now flat-corrects instead of stacking
  uncorrected. PSF Guard fits the bias pedestal from the lights themselves
  (background against flat response), corroborates it against the camera's
  recorded offset when the mapping is known, subtracts it, and applies the
  flat — the stack card states the fitted value. When the fit cannot be
  trusted, the stack still proceeds without the flat as before. See
  [calibration
  libraries](https://github.com/theatrus/psf-guard/blob/main/docs/CALIBRATION_LIBRARY.md).

- A remote server can now export onto its own drive. Configure a **server
  export directory** per database (Settings, or `export_dir` in the
  registry); the Overview's Export action then runs as a background job with
  progress, placing lights, matching calibration frames, and the WBPP runner
  into a per-scope folder there — cloning files (reflink) where the
  filesystem supports it, copying where it does not. No database-management
  grant needed: consent was given when the directory was configured.

## Fixed

- Remote image upload can now attach image bytes after scheduler sync has
  already registered the same light, instead of rejecting that normal order
  of operations as a duplicate.
- Remote scheduler previews can now run as background jobs, so large Target
  Scheduler catalogs no longer fail merely because preview planning takes
  longer than an HTTP reverse proxy timeout. Updated clients request this mode
  and poll the token-scoped job until the preview is ready. Capabilities also
  advertise image upload only when that database has enabled its separate
  upload gate.
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
