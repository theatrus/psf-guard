# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Every stack build now measures how its signal-to-noise ratio grows with
  depth and draws the curve on the channel card. For equal, known exposures it
  adds the square-root line perfect averaging would give and says whether the
  noise is still falling, how far short of ideal the run is, where the returns
  flattened, and about how many more frames another five or ten percent would
  take. The measurement is free: the build reads its own
  accumulator on the way past instead of stacking anything twice, and the
  curve is kept in the resume checkpoint, so a target stacked one night at a
  time ends with one curve over the whole season. A new **Order** control
  switches the reading: capture sequence follows the frames after a fixed
  high-quality reference, while quality order asks which frames are worth
  keeping. The chart folds away from its own heading, the way frame decisions
  and view processing do, and the choice is remembered — folded still shows the
  frame order and the verdict, so you lose the chart rather than the answer.
  `psf-guard stack-snr`
  runs the same analysis over a folder of frames without a catalog. See
  [stack previews](https://github.com/theatrus/psf-guard/blob/main/docs/STACKING_PREVIEWS.md).
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

- Every project gained a **Calibration** report: per kind, the matching
  frames and their capture sessions; per imaging night and filter, the flat
  session a stack would use, its age, and whether the night has its own
  flats — with warnings for missing kinds, nights without same-night flats,
  and stale flats. See [calibration
  libraries](https://github.com/theatrus/psf-guard/blob/main/docs/CALIBRATION_LIBRARY.md).
- Imports can be scoped. The import preview gained an **Import** selector
  (lights and calibration, lights only, or calibration only), per-folder
  checkboxes over the configured roots and two levels of subfolders, and an
  opt-in **skip processing artifacts** toggle that recognizes integration
  masters and PixInsight calibrated/registered intermediates by their own
  signatures. A folder inside a configured root no longer needs the
  database-management grant. The CLI grows matching `--lights-only`,
  `--calibration-only`, and `--skip-processed` flags. See
  [importing](https://github.com/theatrus/psf-guard/blob/main/docs/IMPORTING.md).
- A remote server can now export onto its own drive. Configure a **server
  export directory** per database (Settings, or `export_dir` in the
  registry); the Overview's Export action then runs as a background job with
  progress, placing lights, matching calibration frames, and the WBPP runner
  into a per-scope folder there — cloning files (reflink) where the
  filesystem supports it, copying where it does not. No database-management
  grant needed: consent was given when the directory was configured.

- Review ergonomics: a **Review** tab in Settings holds browser-local
  preferences, starting with whether grading moves to the next image
  (holding Shift while grading does the opposite for that one grade) and
  the two score-chip toggles. Shift+arrow extends the grid selection from
  the keyboard like Shift+click does with the mouse. A selected card now
  keeps its accepted/rejected border color, with selection shown as an
  outer ring and checkmark. Selecting many images no longer queues a
  full-size preview generation per selected frame — only the keyboard
  cursor's image preloads.

- The image detail view gained a **sensor tilt and aberration inspector**
  (press **I**): a 3x3 mosaic of 1:1 crops from each sensor region with
  per-region median HFR, eccentricity, star count, and mean star-elongation
  direction, plus ASTAP-style corner numbers — tilt (softest corner against
  sharpest) and field curvature (corners against center). Solved frames also
  show their **field rotation** (and a mirrored flag) in the Sky context
  panel.
- Data Transfer in Settings now lists every **staged preview** parked on
  the server — including pushes a remote N.I.N.A. client created and never
  applied — with its source, operation, change counts, and expiry, plus
  Apply, Refresh, and Discard actions, and a "What would change" list
  naming each staged change by its own name with exact field-level moves
  ("update project \"Caldwell 49\": state: 1 → 3") — every grade transition with its reason, and
  each project, target, plan, or image the transfer would insert or
  update (capped at 400 lines). Previously only the browser session
  that created a preview could see it, so a plugin's staged push sat
  invisible until it expired. Remote preview jobs also report a phase
  (materializing, then comparing) while they run.

- Remote clients now pair with a **one-time code** instead of a
  hand-copied API key. Settings gains "Pair a client" per database: a
  single-use code, good for an hour, that the N.I.N.A. plugin exchanges
  for its own credential and the catalog id in one step. Every pairing
  mints a separate credential, and the new **Paired clients** list revokes
  any one install without signing out the rest. Manually configured keys
  keep working.

## Changed

- A dark now suits a light when their exposures agree to a tenth of a percent
  or 0.05 seconds, whichever is the larger allowance. It was a flat 0.05
  seconds, which is a six-thousandth of a five-minute sub — tighter than a
  shutter can be trusted, and tighter than the rule Seiza applied to the same
  frames when it built the master. A 300.25-second dark now pairs with a
  300-second light where it previously did not.
- Checking that a calibration file on disk is still the frame the catalog
  recorded now compares the rotator angle for flats, as matching already did.
  A flat shot at a different angle is not the frame the record describes; an
  angle neither side wrote down still matches. That check also now needs
  positive evidence of identity — an agreeing camera name, or width and height
  that both sides record — so a catalog row that captured neither no longer
  verifies against any file.
- A FITS header that reads as "not a number" is treated as absent rather than
  as a reading. A light whose `CCD-TEMP` was unparseable used to match no dark
  at all; it now matches on the settings it did record, the same as a light
  from a rig with no temperature sensor.

## Fixed

- Scheduler rows can now find image files copied into a PSF Guard image root
  shortly after remote sync, without waiting for the directory cache to expire.
  PSF Guard waits up to ten minutes with backed-off checks, retries preview
  failures when a growing source changes, and keeps local paths out of errors.
  Remote path suffixes stay confined to configured roots; candidates must agree
  on capture time and filter, and ambiguous copies are refused instead of
  opening the first one found.
- A catalog written by an earlier PSF Guard is now upgraded the moment it is
  opened, instead of whenever some later write happened to notice. Stacking a
  project whose catalog predated rotator-aware flat matching failed outright
  with `no such column: rotation`; it now works. A read-only catalog still
  opens for ordinary reads and can reuse recorded cached calibration masters.
  If it needs a new master, the stack continues without that master and says
  why; if an unwritable older catalog is missing a required column, PSF Guard
  reports no calibration library rather than failing part way through a stack.
  See
  [calibration libraries](https://github.com/theatrus/psf-guard/blob/main/docs/CALIBRATION_LIBRARY.md).
- Calibration library details now load when the library contains frames
  instead of failing with an invalid-column error.
- The Overview page no longer times out on large catalogs. It used to ask the
  database for each target's date range and filter set one target at a time,
  holding the shared connection for the whole page; on a catalog of nineteen
  targets and ten thousand images that was thirteen seconds of scanning per
  load, long enough to time out and to stall every other request behind it.
  The same answers now come from one pass each, and the connection is released
  before the response is built.
- PSF Guard now adds two indexes to Target Scheduler's image table in the
  background at server startup. A busy catalog is left alone and retried at
  the next start. Target Scheduler ships no such indexes, so looking up one
  target's images read every row. Adding an index changes no data and no table,
  so Target Scheduler and N.I.N.A. keep working exactly as before.
- Progressive stack SNR now invalidates pre-curve checkpoints, resumes only an
  exact sequence prefix, verifies the checkpoint files agree, and retries
  transient frame-read failures from a clean stack. It measures row and column
  structure without turning a linear gradient into a noise floor and treats
  small uncertain rises as measurement scatter. Mixed or missing exposure
  lengths and inconsistent fitted trends keep their measured curve but no
  longer receive an invalid directional verdict or projection.
- Flat matching now respects the rotator. A flat only corrects lights
  shot at (nearly) the same rotator angle — vignetting from the optics
  ahead of the rotator turns relative to the sensor — so flats now match
  lights within 1° (wrap-aware), and a flat master never integrates
  frames from different angles. Frames without a recorded angle keep
  matching everything: rigs without rotators and flats catalogued before
  this change lose nothing. Re-import flats to record their angles.

- The project scheduler editor now exposes Target Scheduler's **flats
  handling** setting (off, every 1-7 sessions, target completion, or
  immediate), read and written with the same schema tolerance as the
  other project fields.

- The "comparing" phase of a pushed bundle no longer scans the thumbnail
  table once per image. The Target Scheduler schema has no index on
  `imagedata.acquiredimageid`, so a merge preview against a season-sized
  catalog ran thousands of full scans over the blob table — measured at
  163 seconds for a 6.5k-row push whose bundle carried no thumbnails at
  all. The comparison now makes one pass over each side and skips the
  thumbnail step entirely when the bundle has none.

- Staging a pushed bundle no longer commits once per row. A large grades
  push materialized each row in its own SQLite transaction — thousands of
  journal round-trips, minutes of "preview" — and a failure mid-bundle
  left a partial snapshot behind. The whole bundle now lands in one
  transaction on a scratch connection with no fsync cost, and a failure
  rolls back to an empty snapshot. Merge exports also stopped carrying
  thumbnail blobs by default: `include_thumbnails` opts in, so a season of
  previews no longer rides along with every catalog round trip.

- Grading now keeps Target Scheduler's exposure plans honest. Every path
  that changes a grade — the grid and detail views, undo and redo, the
  CLI regrade and screening commands, grade pushes, and catalog pulls —
  recomputes `exposureplan.accepted` from the images each plan actually
  has (linked by `exposureId`, schema v17+). Before, rejecting frames in
  PSF Guard left the telescope's counters where its own grader put them,
  so the scheduler under- or over-scheduled that filter. Catalog pulls
  now also keep the counter derived from the merged grades instead of
  copying the telescope's value.

- Star PSF fitting now actually fits. A sign error in the optimizer made
  every iteration step uphill, so every fitted PSF kept its initial guess —
  reported eccentricity was really the star's bounding-box aspect ratio and
  orientation was always zero. Star detail overlays and the tilt inspector
  now see converged models; quality scoring is unaffected (it uses
  N.I.N.A.'s own capture-time values).

- Editing a configured database now opens its settings directly beneath that
  database instead of below the full database list. The selected row and named
  editor stay visually connected, so paths, remote access, and export settings
  have a clear database context.

- Rejecting or accepting a large grid selection is now one request and one
  database transaction instead of two HTTP round trips per image, so
  grading thousands of frames finishes in about a second instead of
  minutes of "Processing". Undo and redo restore the whole selection in
  one request the same way.

- An idle grid no longer polls the server every second. The cache and
  quality status widgets ask fast only while a refresh, scan, or backfill
  is running, drop to a slow heartbeat otherwise, and stop entirely while
  the window is in the background.

- Remote sync export responses now carry an `X-Content-SHA256` header with
  the SHA-256 of the exact response body, so the N.I.N.A. plugin can verify
  a pulled bundle without re-serializing it. The in-bundle `payload_sha256`
  stays advisory: checking it requires reproducing the server's JSON writer
  byte for byte, which is why plugin pulls failed with "bundle digest
  missing or invalid".

- The grid and the Sequence view now show the same quality score for the
  same image. The Sequence view defaults to the all-sessions score, but the
  grid badge was built from per-session scores — a frame in a small session
  could score 1.0 against itself while scoring far lower against the whole
  filter. The grid (and the detail panel) now use the same basis the
  Sequence view displays, and the badge tooltip names it. When both bases
  smaller chips below the badge always carry both bases: a crescent marks
  the night-session score, stacked layers mark the all-sessions score,
  both colored on the badge's scale. The chips never appear or vanish
  between views; two icon checkboxes in the grid and Sequence toolbars
  turn each one off, remembered and shared across views.

- Filter names that differ only by case or whitespace ("Ha" beside "HA")
  no longer split one physical filter into separate quality-comparison
  groups, and filter selections match all variants. Display keeps the name
  as the camera wrote it.

- The same image can no longer show different quality scores in different
  views inside the desktop app. Analysis responses carried no cache policy,
  so the embedding webview could serve one view's request from its own HTTP
  cache while another view fetched fresh scores. JSON API responses now
  default to `Cache-Control: no-store`; previews and static assets keep
  their existing caching.

- Projects are no longer "renamed" to profile GUIDs. When a catalog's
  projects spanned two profiles — which an import could cause by filing new
  projects under the telescope's current-but-empty profile instead of the
  one owning the existing projects — every project name gained a raw GUID
  prefix. Imports now join the profile that owns the existing projects, and
  the display adds a short profile tag only when the same project name
  truly exists under more than one profile.

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
- A night's flats shot across several filters no longer costs you the flat
  master. Frames chosen for a master are now checked against the same rule
  the integrator applies before it will combine them, so a frame that would
  be refused is set aside and the master builds from the rest. Before, one
  OIII flat shot four minutes before a run of ten R flats was enough to
  abandon the R master outright, because everything about those frames
  except the filter agreed. The calibration warning names what was set
  aside and how many. Bias and dark frames are unaffected by filter, which
  only a flat records; a frame from another camera, gain or readout mode is
  still set aside for every kind.
  See [calibration
  libraries](https://github.com/theatrus/psf-guard/blob/main/docs/CALIBRATION_LIBRARY.md).
- Upgrading a catalog now reads the rotator angle off flats that predate the
  `rotation` column. Adding that column left every existing row empty, and an
  empty angle means "nobody wrote one down", which matches any angle at all —
  so a library filled before the column existed looked like one where the
  rotator had never moved, and flats from nights a degree or more apart were
  offered as one set. The files recorded the angle all along; now the catalog
  reads it. Flats only, headers only, and a flat whose file has moved away
  keeps its empty angle rather than stopping the upgrade.
- One calibration frame that disagrees with the rest no longer costs you the
  whole master. The integrator reads each frame's headers, and a frame whose
  metadata contradicts the reference is now left out and named in the
  calibration warning instead of stopping the build. This is the half
  selection cannot do: a catalog holds only what it recorded at import, so a
  flat shot at a rotator angle the catalog never wrote down looked compatible
  with every other flat until the files themselves were read. A set that
  falls below two usable frames is still reported rather than integrated,
  since a master built from one frame is that frame.
- A stack whose lights span several calibration sessions no longer dies at
  the first session boundary. Swapping in a later night's masters was judged
  against the stack's reference frame — a light from another night, already
  integrated, that the new masters would never touch — so a rotator that
  moved between nights killed the whole stack at the boundary even though
  every light had a matching flat. The swap is now judged by the frames it
  actually calibrates. A frame that genuinely does not match is turned away
  alone, and its reason now names the field and both readings — for example
  "rotation light=101.93deg master=104.24deg (2.31 deg apart, tolerance
  1.00)" — instead of a sentence that says only that something disagreed.
  In auto mode a calibration problem is never fatal: if a session's masters
  cannot be applied at all, its frames stack uncalibrated and the warning
  says so and why. Opening a catalog also no longer re-reads hundreds of
  flat headers looking for rotator angles it already knows are not there.
- Flats now match lights within 2 degrees of rotator angle rather than 1,
  absorbing a rotator that re-homes to the same framing with a little scatter
  between nights. The tolerance is also now yours to set: Settings → Setups →
  Calibration matching takes a value in degrees, applies it to the next stack
  without a restart, and an empty field returns to the default. The setting
  is server-wide and shared by the desktop and browser apps.
- Stacking OIII and SII no longer fails against calibration masters generated
  before mid-August. Two causes, both fixed. Validation treated a reading the
  master never recorded — telescope, focal length — as a disagreement, when
  an unrecorded reading proves nothing; masters now validate on what both
  sides actually recorded. And those old masters should have rebuilt
  themselves: the master cache records a generation number but reuse never
  checked it, so masters from before metadata preservation were served
  forever. Reuse now requires the current generation, and every older master
  rebuilds itself once, with full metadata. "Clear generated masters" in the
  Calibration Library remains the manual lever.
- Auto-mode calibration now keeps its promise everywhere: a stack is never
  abandoned over calibration. A plan that cannot be built, a reference frame
  whose masters are refused, and a mid-stack master swap that fails all
  degrade to stacking raw with a warning that says why; frames the stacker
  turns away for calibration are summarized in the same warning — count and
  reason — instead of hiding in the per-frame list. Forced calibration keeps
  hard errors.
- The tilt inspector now draws ASTAP's tilt figure: each corner's HFD becomes
  a vertex distance from center, so a flat field draws the dashed reference
  square and a tilted sensor a quadrilateral leaning toward its soft corner.
  The region statistics and the tilt/curvature verdict are computed by the
  server, so the same numbers the dialog shows are available to every API
  consumer, and a frame's analysis no longer depends on which browser
  computed it.
