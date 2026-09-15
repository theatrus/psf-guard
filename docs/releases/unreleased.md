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

- Exports keep each night's flats apart. A light takes the calibration
  frames a stack would build from, its flats land in a `SESSION_<night>`
  folder, and in the WBPP layout the light's path names the same night. The
  runner turns WBPP's keyword grouping on so each night's lights calibrate
  with their own flats before the nights integrate together. Before, every
  matched flat of a filter shared one folder and WBPP integrated one master
  flat for all of them.
- The export dialog offers **Include ungraded lights**, on by default, so
  a night the grader has not judged yet exports the way the stack previews
  show it. Rejects stay excluded.
- Exports can **Link to the originals** (symbolic links, no space, may point
  at a network mount) or **Reference in place** (nothing copied; a
  `run-wbpp.js` PixInsight script names every frame where it is, below a
  server folder you map to what the machine running PixInsight calls it,
  such as `/mnt/nas/astro` to `P:\`, with no limit on how many frames, and
  each night's flats still matched to its lights). The script says which
  frames it could not find instead of leaving WBPP's dialog empty.
  In the browser, **Scripts only** downloads those scripts as a small zip
  instead of the frames. On the CLI: `--placement symlink` or
  `--placement reference --local-root <path> --remote-root <path>`.

- Stack and color previews state integrated exposure in hours and minutes.
  A stack card and its inspector read "2h 5m" instead of "125m 12s", the
  stack panel adds up every remembered channel into a project total, and a
  color preview now shows each channel stack's integration and the total
  across its channels, which it never showed before.
  A color preview composed before this release reads as out of date and
  rebuilds to pick up its hours; until then it shows frame counts only
  instead of "0s".

## Fixed

- Confirming an import after its preview now shows the import's own
  progress and result. The page had stopped polling when the preview
  finished, so the panel kept the preview and the footer stayed on
  "Importing…" until a reload.

- The Settings footer now reports when an import finishes instead of
  staying on "Importing…". A calibration-only import reads "Imported 12
  calibration frame(s)." rather than "0 light frame(s) — nothing new".

- The import preview now appears under the database it imports into, and
  the page scrolls to it, instead of at the bottom of Settings. A scan
  started from a database's calibration library dialog lands where you were
  looking.

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
