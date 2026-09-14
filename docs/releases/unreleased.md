# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Quality analysis now flags **Rotation skew**: a frame, or a run of two,
  whose solved field rotation sits more than 2° from the rotation the rotator
  held around it, compared modulo a half turn so a meridian flip is not skew;
  a re-rotation held for three frames or more is framing, not skew, unless
  it returns to the earlier angle. A skewed
  frame is scored like a pointing jump, recommended for rejection, and left
  out of stack previews. The Sequence view's astrometry selection includes
  it, and the image grid can filter by any quality flag, rotation skew among
  them. Each frame's solved rotation and, when Target Scheduler planned one,
  the planned rotation and their difference are in the sequence results.

- Stack previews can keep themselves current. With **Rebuild stack previews
  on their own** turned on in Settings → Setups, a project whose previews were
  built once rebuilds the same channels, with the same settings, after new
  frames arrive by import, upload, or sync (5 minutes of settling, so a
  night's stream is stacked in batches) and after grades change (15 minutes,
  so an interactive pass settles first). Color previews follow their
  channels. A build you start takes precedence; the automatic one steps aside
  and comes back afterwards.

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

- Stack and color previews state integrated exposure in hours and minutes.
  A stack card and its inspector read "2h 5m" instead of "125m 12s", the
  stack panel adds up every remembered channel into a project total, and a
  color preview now shows each channel stack's integration and the total
  across its channels, which it never showed before.

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
