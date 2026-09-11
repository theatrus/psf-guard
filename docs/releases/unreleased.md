# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Inspect the calibration masters used by a mono or color stack from its
  **Masters** action, with session/channel provenance, display stretch,
  native-size zoom, rejection statistics, and unchanged FITS downloads.

- Optional native star masking for flat masters, controlled by **Mask stars
  in flats** in calibration settings. Masked builds retain original unmasked
  measurements, report low coverage, and refuse unsupported pixels without
  silently smoothing or filling them. No external star-removal tool is needed.

- A frame shot with a warm sensor is flagged **Sensor Temperature**, capped
  to a rejected score, and given an `[Auto]` reject recommendation: more
  than 10 °C warmer than its capture session, or more than 20 °C above the
  cooler's set point. A cooler dropout no longer slips into a stack because
  the stars still measured well.

- In a multi-night stack, lights from a session with no dark master now get
  hot-pixel suppression even when other sessions have darks; the calibration
  card says which sessions were filtered. Existing stack previews rebuild
  when next opened.

## Changed

## Fixed

- Sky-flat masters improve rejection of overlapping moving-star samples with
  robust median/MAD clipping after calibration and brightness normalization. Existing
  generated masters rebuild with the new algorithm when next needed. Stars
  need enough drift or dithering to leave most samples at each pixel clean;
  clipping does not guarantee a star-free master.
