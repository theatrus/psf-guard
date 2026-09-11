# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- A per-project **Separate exposure groups** option keeps substantially different
  exposure lengths apart in the image grid and stack previews. Short and long
  integrations retain independent stack results and calibration choices.
  RGB, LRGB, and narrowband cards pair matching exposure bands automatically;
  incomplete bands show their missing channels. **Custom combination** keeps
  manual source selection available for mixed exposures and saved variants.

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

- PSF Guard now checks a flat master against the flats that made it. Spots
  that fade across the run (dew drying off the sensor window) used to survive
  clipping and put a bright bead into every light wherever the sky is bright.
  Frames that disagree with the rest of the set stay out; a set that disagrees
  with itself gives way to the next one, or serves with a warning when there
  is no other. Flat masters built before this rebuild once; bias and dark
  masters do not.

- Sky-flat masters improve rejection of overlapping moving-star samples with
  robust median/MAD clipping after calibration and brightness normalization. Existing
  generated masters rebuild with the new algorithm when next needed. Stars
  need enough drift or dithering to leave most samples at each pixel clean;
  clipping does not guarantee a star-free master.
