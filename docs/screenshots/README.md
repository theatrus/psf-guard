# README screenshot sources

The main README reuses documentation-site captures from October 17–18, 2026.
All files are exact copies, with no resizing or edits. Keep the files and
recorded states together when reshooting. This record preserves the original
mosaic and calibration entries, followed by the full README refresh.

Source repository: `theatrus/psf-guard-docs`, commit
`9e9391f19efd3d88104d66ed7800f52d23ac00ed`. The original
[capture record](https://github.com/theatrus/psf-guard-docs/blob/9e9391f19efd3d88104d66ed7800f52d23ac00ed/assets/screenshot-captures.md)
contains the same shared environment and per-image entries below. The original two copies
match their recorded SHA-256; neither was resized, edited, or annotated.

### Shared environment — October 17, 2026

- **Source:** signed-in Chrome browser at `https://psf-guard.atpn.co`.
  Server Information reports **0.11.0**. The captured UI has the features
  described for 0.12; the deployed commit and frontend/backend match are
  unknown. Do not treat the version label as a verified 0.12 release build.
- **Environment:** macOS 27.0.1; installed Chrome 155.0.8059.82; dark app theme;
  English UI. Viewport at capture: **1744 × 1142 CSS px**. Effective device
  pixel ratio: **1.8**. CSS-to-CDP capture coordinate ratio: **0.9**, consistent
  with 90% browser zoom on a 2× display; zoom was not read from Chrome's menu.
- **Capture method:** Chrome `Page.captureScreenshot`, JPEG quality 95,
  `fromSurface: true`, `captureBeyondViewport: false`, clip scale 1. Clip
  rectangles below use CDP device-independent pixels (DIP), not final pixels.
  Multiply the listed CSS crop by 0.9 to obtain the CDP clip. Saved files
  contain roughly 1.8 image pixels per CSS pixel. No resizing, annotations,
  compositing or redactions were applied. The cursor is absent from the files.
- **State:** existing live data only. No grades, group boundaries, light
  assignments, saved plans or activation state were changed. Background jobs
  were already running; none were started or stopped for these captures.
  Temporary viewport overrides were reset before capture. Mosaic overlay
  preferences were restored after capture.
- **Documentation check:** all three images loaded at the guide's 824 CSS px
  column width with their aspect ratios intact. Captions carry the key values;
  the lightbox and full-resolution link were checked for the flat comparison.
- **Reshoot:** use the same view dimensions if possible, but remeasure the crop
  after loading. UI changes, new frames and refreshed derived results can move
  controls or change counts. Inspect each final image before replacing it.


### planning-mosaic.jpg

#### 2026-10-17 18:00 PDT — initial capture

- **File:** 2936 × 1524 px, JPEG. SHA-256: `ec2c75fc35c76f9c963da0759e5393e2e70abf4acbc6b8ebd928568f7229f687`.
- **Also used by:** README.md;
- **Original use:** `docs/new-in-0.12.html#planning`; inspecting a captured panel
  while retaining its lock and measured overlap warnings.
- **Build/environment:** shared environment above. No site coordinates shown.
- **Source:** askar107phq, database `db-a01c581c`, project 21, Heart Mosaic;
  plan `051751d1-6b05-444e-9485-afbc6177dd5b`; panel `r1c2`, 336 frames.
- **URL:** https://psf-guard.atpn.co/#/plan?db=db-a01c581c&project=21&current=8620&q=Heart&plan=051751d1-6b05-444e-9485-afbc6177dd5b
- **UI state:** Framing; All targets; Panel drag mode; Snap on; r1c2 selected.
  DSS (offline), north up / east left; width reads 5.18° (stage reads 5.17°).
  Equatorial grid on; Constellations and Deep-sky marks off for readability;
  Stacks off. Sky center 02h32m38.3s, +61°21′37″. Four panels locked.
  Right controls scrolled to Mosaic panels, with selected-panel details open.
  Minimum overlap 20%; added-panel overlap 20%; Whole mosaic selected.
- **Observed state:** panel field 1.79° × 1.20° at 0.6°. Panel counts:
  r1c1 343, r1c2 336, r2c1 329, r2c2 331. Overlap warnings: 18.9%, 18.1%,
  18.8%, 19.4%, all below 20%. Not activated yet, outside this crop.
- **Reshoot steps:** open the URL, wait for the DSS image and footprints, turn
  off Constellations and Deep-sky marks, then select r1c2 once (clicking again
  deselects it). Scroll the right controls to show its lock and field. Return
  the main page to its top so all four footprints fit. Do not unlock, drag,
  save or activate. Restore the two overlay preferences after capture.
- **Composition:** CSS crop `(56, 280, 1632, 847)`; CDP clip
  `(50.4, 252, 1468.8, 762.3)`. Main page at top; `framing-controls`
  scrollTop about 1482.78 CSS px. Crop includes the sky, layer controls,
  panel controls, lock state and all four overlap warnings. The pointer was
  outside the crop; no hover tooltip or annotations were added.
- **Verification:** all four captured boundaries and padlocks are visible;
  r1c2 remains locked. Center overlap labels partly overlap in the app; the
  complete values remain readable in the warning list. Do not claim the saved
  overlap minimum was achieved or that this plan has been activated.

### Second capture session — October 17, 2026

The seven entries below use the shared build, OS, browser and JPEG capture
method above. Viewport was **1744 × 1204 CSS px**, measured in this session;
effective device pixel ratio remained **1.8** and CDP coordinates use the same
0.9 factor. Theme stayed dark. No browser viewport override was used.
All captures used existing data and cached stacks. No grades, builds, saved
processing settings or jobs were changed. Temporary scoring changes were reset;
stack card size returned to its original Fit/1600 value.

Local browser review checked the grader, screening and stacking guides and the
homepage. Guide overviews render at 824 CSS px; the scoring crop at 420 px;
the color card at 640 px; the homepage preview at 1112 px. Aspect ratios stayed
intact. The issue list, score chips, scoring controls and color card also
opened in the lightbox with their captions and full-resolution links.

### calibration-review.jpg

#### 2026-10-17 17:59 PDT — initial capture

- **File:** 2476 × 1152 px, JPEG. SHA-256: `321d3be101c0138e03a985719c3fa2c307e02369f5edc4961fc9791a190023d8`.
- **Also used by:** README.md;
- **Original use:** `docs/new-in-0.12.html#calibration`; grading a flat from its
  measured difference from the session, despite no clipped pixels.
- **Build/environment:** shared environment above. Frame time displays UTC−5.
- **Source:** starfront-redcat61, database `db-49b9f973`, project 2,
  Teddy Bear Nebula QMark; redcat61 / ZWO ASI2600MM Pro.
  Session `c86f7ba628d954df`, September 13, 2026.
  Frame `b28da39d-e31d-43f9-a226-944aa648e997`,
  `2026-09-14_07-05-02_OIII_-5.10_32.62s_0002.fits`.
- **URL:** https://psf-guard.atpn.co/#/calibration?db=db-49b9f973&project=2&current=1415&calProject=2&calSession=c86f7ba628d954df&calKind=flat&calGrade=poor&calFrame=b28da39d-e31d-43f9-a226-944aa648e997
- **UI state:** Frames → Sessions; project scope on; Type Flat; Grade Poor;
  other filters All/Any; Missing files and Duplicates off; Order As shot.
  Normal stretch, Compare with session off, Spots off, Full size off.
  Fit selected; viewer reads 85%. Frame details and measurement table visible.
- **Observed state:** Poor · uneven light; excluded from masters; illumination
  difference 13.9%, limit 5.0%. Frame/session: level 58.5%/50.5%; clipped
  0.000%/0.000%; noise 1.02%/1.13%; 0 spots and 9/10 large disks.
  Ten of ten session frames measured; frame in 13 masters.
- **Reshoot steps:** open the URL, wait for the session and preview to load,
  verify the filename and Poor reason, and click Fit. Keep the viewer controls
  and the entire details card in the crop. No data changes are needed.
- **Composition:** CSS crop `(344, 490, 1376, 640)` at the top of the page;
  CDP clip `(309.6, 441, 1238.4, 576)`. Viewer and detail card only; the session
  navigation is outside the crop. No hover tooltip. Actual ratio comparison
  is off; the table compares frame and session measurements.
- **Verification:** loaded preview was 1042 × 696; final JPEG checked for
  complete controls, filename, grade, measurements and limit. Subtle variation
  is visible in the flat; this is not proof of a particular dust or dew cause.

## README refresh — October 18, 2026

Replaced or removed every older UI screenshot linked from the README. The
14 copies below join the recorded mosaic and calibration images above.
This is a reuse pass, not a new capture session. All 16 displayed images have
capture records; the old image files remain where other guides use them.

Source repository: `theatrus/psf-guard-docs`, commit `ed4dc3849733946bd5f35d574d5f9f4542643589`.
The [original capture record](https://github.com/theatrus/psf-guard-docs/blob/ed4dc3849733946bd5f35d574d5f9f4542643589/assets/screenshot-captures.md)
contains these entries. The local [copy manifest](captures-readme-2026-10-18.json)
records source paths, commits and SHA-256 values. Each entry below preserves
the actual source, state, crop and reshoot steps. All copied images were
visually checked against their new README captions.

Live-server captures reported version 0.11.0 and an unknown deployed commit;
they show the current UI used by the docs, not a verified 0.12 release binary.
The two import/transfer captures used a verified 0.12.0 test server and
isolated catalogs. No live data changed during this reuse pass.

### Second capture session — October 17, 2026

The seven entries below use the shared build, OS, browser and JPEG capture
method above. Viewport was **1744 × 1204 CSS px**, measured in this session;
effective device pixel ratio remained **1.8** and CDP coordinates use the same
0.9 factor. Theme stayed dark. No browser viewport override was used.
All captures used existing data and cached stacks. No grades, builds, saved
processing settings or jobs were changed. Temporary scoring changes were reset;
stack card size returned to its original Fit/1600 value.

Local browser review checked the grader, screening and stacking guides and the
homepage. Guide overviews render at 824 CSS px; the scoring crop at 420 px;
the color card at 640 px; the homepage preview at 1112 px. Aspect ratios stayed
intact. The issue list, score chips, scoring controls and color card also
opened in the lightbox with their captions and full-resolution links.

### Full guide capture environment

Live captures use the shared server environment above; entries marked local fixture
use the isolated server described below. The live server reported 0.11.0; its deployed
commit and frontend/backend match remain unknown. Times below
are UTC. Job percentages and counts can change between inspection and capture.
The images show the final values; state notes record the inspected state.

Desktop captures use the normal viewport. The two phone captures use a temporary
375 × 812 CSS-pixel viewport with DPR 0.9; the override was reset afterward. PNG
files use lossless capture; JPEG files use quality 95. All crops are direct browser
captures. Some include the browser automation pointer highlight. No pixels were
retouched. Site coordinates were cleared in unsaved forms, then discarded.

Each entry includes the route, actual state, viewport, crop and file identity.
Open the route, use the named view or Settings section, restore the listed controls,
and wait for images and overlays to load before measuring the crop again. Settings
dialogs are not encoded in the URL: open Menu → Settings, then the section named
in the state. Unsaved examples must be discarded afterward. Do not start jobs or
change live catalog data to recreate a transient state.

All saved images were checked in contact sheets against the inspected UI. Tall
settings and narrow side panels use the guide's full-resolution lightbox for detail.
Specific differences from the planned shot are retained in the state notes and
the published captions. The machine-readable companion is
[captures-2026-10-17.json](https://github.com/theatrus/psf-guard-docs/blob/ed4dc3849733946bd5f35d574d5f9f4542643589/assets/captures-2026-10-17.json).

### assets/stack-foraxx-card.jpg

#### 2026-10-17 18:11 PDT — reshoot

- **File:** 1108 × 1306 px, JPEG. SHA-256: `344bac447ce065f240df3bd9ec7b947b613393479b45435cf718e1dba137d781`.
- **Used by:** `docs/stacking.html#color`.
- **Build/environment:** shared environment and second-session viewport above.
- **Source:** California Nebula, ultra cat, database `db-e075b4ec`, project/target 4. Foraxx SHO, 300 s exposure group. Color key `da718ffafabc7efe24623a1b2beefe4084c3d3eea029367dc30f677245d7fd35`, source revision `18df75db6d20fd1c-1e32e6` from its visible download link.
- **URL:** https://psf-guard.atpn.co/#/stacks?db=db-e075b4ec&project=4&target=4&current=561&cardsize=600
- **UI state:** Card size 600 px; separate exposure groups on; Accepted only off; Method Recommended; Calibration Auto; Order Capture sequence. Foraxx SHO already selected; Keep blank edges; completed. Pipeline phases and Processing stack collapsed. Page top. No settings changed except card size, restored to Fit/1600 after capture.
- **Observed state:** preview ready, 29/29 steps. Hα 181 frames / 15 h 5 m; OIII 577 / 48 h 5 m; SII 160 / 13 h 20 m; total 918 / 76 h 30 m. Processing summary: BG subtract, DC off, RC-Astro 6, Hα 1, OIII 1, SII 1, RGB 0. Auto MTF is selected for the three channels; deeper RC-Astro settings were not inspected.
- **Reshoot steps:** open Stacks and wait for cached metadata and previews; the initial loading state can briefly say Not built. Set card size 600, verify Foraxx SHO and the input counts, and crop the complete card. Do not build or change processing to recreate the image. The existing cache is a prerequisite.
- **Composition:** CSS `(872, 365, 616, 726)`; CDP `(784.8, 328.5, 554.4, 653.4)`. Complete card with a narrow margin. Source preview bitmap 2400 × 1601.
- **Verification:** palette, edges, loaded preview, completed status, processing summary, inputs and Masters button visible. Replaces an older Teddy Bear card whose capture state was not recorded. Blank registration edges remain, as selected.

## Owned catalog import flow — October 18, 2026

These eight captures use the merged TS import flow from PSF Guard PR #911 and
release preparation PR #912. They show real forms and real API results from an
isolated server, not the live observatory catalogs.

- **Build:** PSF Guard 0.12.0. Product source at `b6177fff` on merged main
  `1fef4d27`; capture spec and instructions at `6392a9a4`. The latter adds no
  product changes. The server embeds the same frontend as the captured source.
- **Environment:** macOS 27.0.1; Playwright Chromium 156.0.8078.4; viewport
  1440 × 1200 CSS px; device scale 2; browser zoom 100%; dark theme; reduced
  motion; en-US; America/Los_Angeles. No annotations, image resizing or redactions.
  The mouse pointer is absent. Focus follows the last action in the capture spec.
- **Source:** `static/e2e/global-setup.ts` creates two projects and four real FITS
  images. The capture spec copies the rows into the vendored TS schema with GUIDs
  `project-<Id>`, `target-<Id>` and `image-<Id>`. No random image data or live data.
  The source file is `/tmp/psf-guard-012-flow/TargetScheduler.sqlite`; images are
  `/tmp/psf-guard-012-flow/images`. On macOS saved paths resolve below `/private/tmp`.
- **URL:** all captures use `http://127.0.0.1:23652/`. Settings is modal state and
  is not restored by the URL. The version's What's new popup is acknowledged.
- **Capture:** element PNGs at device scale 2. Exact CSS crop rectangles, times,
  pixel sizes, checksums and state are in
  [captures-import-flow-2026-10-18.json](https://github.com/theatrus/psf-guard-docs/blob/ed4dc3849733946bd5f35d574d5f9f4542643589/assets/captures-import-flow-2026-10-18.json).
- **Reshoot:** check out the capture commit or a later source with the spec, build
  the server, and run from the PSF Guard repository root:

  ```bash
  PSF_GUARD_E2E_BINARY=/absolute/path/to/psf-guard \
  PSF_GUARD_E2E_TMP=/tmp/psf-guard-012-flow \
  PSF_GUARD_CAPTURE_DIR=/absolute/path/to/new-captures \
  scripts/e2e-local.mjs e2e/import-flow-captures.spec.ts --project=chromium
  ```

  Use an empty output folder. Global setup clears the disposable fixture path;
  teardown removes the test catalogs. The spec writes only there: it imports a
  TS snapshot, saves automatic sync off for the reviewed transfer, adds project
  and target 100 to the fixture source, previews and applies their merge, removes
  that catalog registration, creates a plugin catalog, and tests code generation
  after the last screenshot. No credentials appear in these images or records.
  Catalog UUIDs, preview expiry times and PNG checksums will change on a reshoot.
- **Verification:** inspected all eight PNGs for complete labels and controls.
  Local import, saved sync settings, reviewed Preview/Apply, plugin creation and
  pairing-code generation passed against the real server. No Windows N.I.N.A.
  Pair/Test connection was run. The images do not claim a paired client or a
  completed remote sync.

### library-compact.jpg

#### 2026-10-18T01:35:48.475Z — capture

- **File:** 3138 × 1260 px; SHA-256 `03e4159c97b5fbd281b94095c69cbd260540cf6550ffe5680630db3fb2b71093`.
- **Used by:** `index.html`, `docs/grader.html`, `docs/index.html`.
- **Purpose:** The compact Library lists projects across all databases, with counts and review status.
- **Build/environment:** shared live-server environment; viewport 1744 × 1142 CSS px; DPR 1.80.
- **Source URL:** https://psf-guard.atpn.co/#/?db=db-e075b4ec&project=12&target=12&current=10955&filterName=HA&grouping=filter&order=newest
- **State and reshoot:** All databases Library, Compact, newest first. Catalog 38,691 images, 25,562 accepted, 3,645 to review. Last 7 days rows through LDN798; counts are capture time observations.
- **Composition:** CSS crop (0, 0, 1744, 701); CDP clip (0.00, 0.00, 1569.60, 630.90), scale 1.

### grid.jpg

#### 2026-10-17 18:09 PDT — reshoot

- **File:** 3138 × 1674 px, JPEG. SHA-256: `acce8b543106e2402ce43f5e77e56745cac2c27b9e0c6b0fc60e4fac28a2becc`.
- **Used by:** `docs/grader.html#grid`.
- **Build/environment:** shared environment and second-session viewport above.
- **Source:** Iris Nebula, starfront-ultracat131, database `db-93617230`, project 1; target NGC 7023. Current frame 776, September 17 at 23:02:13.
- **URL:** https://psf-guard.atpn.co/#/grid?db=db-93617230&project=1&current=776
- **UI state:** Images, page top; Status, Filter, Flag and Calibration All; Dates Any; search empty; Size 300 px; Stats off. View: Group Date, Oldest first, Nights and hours, nights end at each rig's noon; night, all-sessions and calibration chips on; Separate exposure groups on. Zero selected; current frame has a cyan outline.
- **Observed state:** 822/822 images scored, 27 groups. First group September 17, L, 300 s: five Accepted frames, all with Sky Brightening warnings. Next group G, 120 s: 12 frames, 11 Accepted and one Rejected. Four visible G cards warn that their dark master has too few frames. Background jobs: 60%, one running, six waiting.
- **Reshoot steps:** open the URL; wait for Quality: 822 scored and the previews. Check View against the settings above, then close it. Keep the L row and first G row visible. The plan originally requested Group Filter; this capture uses Date to keep the verified score examples together.
- **Composition:** CSS crop `(0, 0, 1744, 930)`; CDP `(0, 0, 1569.6, 837)`. Header, toolbar and first two card rows. No tooltip.
- **Verification:** all visible previews loaded; score chips, grades and calibration icons readable. Scores and warnings do not replace the stored grades. Frame time zone was not exposed in this view; preserve the displayed timestamps when locating frames.

### sequence-session.jpg

#### 2026-10-18T01:30:24.522Z — capture

- **File:** 3138 × 1652 px; SHA-256 `707112f1981979c6056d9dd1e70c777d1a58c4656130849c9708cf06be6938ad`.
- **Used by:** `index.html`, `docs/astrometry-quality.html`, `docs/grader.html`, `docs/screening.html`.
- **Purpose:** Compare one capture session. These stored grades are Rejected; the scores still show the differences between frames.
- **Build/environment:** shared live-server environment; viewport 1744 × 1142 CSS px; DPR 1.80.
- **Source URL:** https://psf-guard.atpn.co/#/sequence?db=db-e075b4ec&project=12&target=12&current=10952&filterName=HA
- **State and reshoot:** Golf of Mexico ultra cat db-e075b4ec project12 target12, HA Jul9 2026 10:08 PM session57. Score cutoff50, By hand,0selected,150px,night/all/calibration chips. Three Soft Stars25 then Possible Obstruction62/60. All stored grades Rejected.
- **Composition:** CSS crop (0, 0, 1744, 918); CDP clip (0.00, 0.00, 1569.60, 826.20), scale 1.

### grid-phone-filters.png

#### 2026-10-18T01:35:25.271Z — capture

- **File:** 337 × 730 px; SHA-256 `242ff40c7954b9160b69164cce198c96cac4e0bf617765d4d8595729959cdec8`.
- **Used by:** `docs/grader.html`.
- **Purpose:** At phone width, expand Filters to reach the grid controls.
- **Build/environment:** shared live-server environment; viewport 375 × 812 CSS px; DPR 0.90.
- **Source URL:** https://psf-guard.atpn.co/#/grid?db=db-e075b4ec&project=12&target=12&current=10955&filterName=HA&grouping=filter&size=150&order=newest
- **State and reshoot:** 375 CSS px phone view (viewport override 338x731 DIP with 90% zoom; DPR 0.9). Golf of Mexico, Filters expanded, 150 px, newest first. All filters unchanged.
- **Composition:** CSS crop (0, 0, 375, 812); CDP clip (0.00, 0.00, 337.50, 730.80), scale 1.

### import-image-folders.png

#### 2026-10-18T07:06:51.809Z — initial capture

- **File:** SHA-256 `c6b7aa40fc58ffbdbc11c39931d835afd51f2dc2a29bdc4377b732078820186d`; 2048 × 1524 px; PNG.
- **Used by:** docs/importing.html — Create a catalog from images.
- **UI and observed state:** Image folders selected; Image archive name; real FITS fixture directory added; managed storage; background quality analysis off; not submitted.
- **Composition and reshoot:** follow the shared environment and capture spec above; CSS crop `{"x": 68, "y": 267.1875, "width": 1024, "height": 761.296875}`. No overlays, image processing or zoom/pan apply to these forms.

### annotated-hfr-labels.png

#### 2026-10-18T01:31:28.992Z — capture

- **File:** 3138 × 2054 px; SHA-256 `f262f33b6e5a21e7b7f93c429940599794a0455ba20e1fcc8637649414c3ea2a`.
- **Used by:** `docs/grader.html`.
- **Purpose:** The star overlay labels measured HFR values. These measurements can differ from imported capture metadata.
- **Build/environment:** shared live-server environment; viewport 1744 × 1142 CSS px; DPR 1.80.
- **Source URL:** https://psf-guard.atpn.co/#/detail/10955?db=db-e075b4ec&project=12&target=12&current=10955&filterName=HA&returnTo=sequence
- **State and reshoot:** Golf HA frame10955. S Stars on; annotation847 stars AvgHFR2.68 AvgFWHM5.29 versus imported HFR1.49/5340 stars. Fit65%. Image Information open, others folded. This frame is stored Rejected with possible obstruction, not a clean accepted example. Wait until loading overlay clears.
- **Composition:** CSS crop (0, 0, 1744, 1142); CDP clip (0.00, 0.00, 1569.60, 1027.80), scale 1.

### stacks-mosaic.jpg

#### 2026-10-18T01:42:42.667Z — capture

- **File:** 3138 × 1584 px; SHA-256 `aee3a571aa32ac26348b4a93ab672fbbf713aff2f3b79a02cb6b39547bfb1a7b`.
- **Used by:** `index.html`, `docs/grader.html`, `docs/stacking.html`.
- **Purpose:** Compare the four SHO panel stacks with their solved, north-up placement.
- **Build/environment:** shared live-server environment; viewport 1744 × 1142 CSS px; DPR 1.80.
- **Source URL:** https://psf-guard.atpn.co/#/stacks?db=db-a01c581c&project=21&current=8620&q=Heart&mosaic=1
- **State and reshoot:** Heart Mosaic4panels, existingSHOstacks, gridleft and solveplacednorthup right.550pxcards, all4panels solved. Automaticbuildscontinuingbelowcrop; none startedbyus.
- **Composition:** CSS crop (0, 0, 1744, 880); CDP clip (0.00, 0.00, 1569.60, 792.00), scale 1.

### stack-foraxx-california.jpg

#### 2026-10-17 18:12 PDT — initial capture; replaces stack-foraxx-teddy.jpg

- **File:** 2886 × 1806 px, JPEG. SHA-256: `1680caa3eb4ec1c5e234b36934db6fe1e259b49608300449bc152c48e6129ec1`.
- **Used by:** `index.html#stacking`.
- **Build/environment:** shared environment and second-session viewport above.
- **Source, build and processing:** same California Foraxx SHO cache and channel inputs as `stack-foraxx-card.jpg` above; source revision `18df75db6d20fd1c-1e32e6`.
- **URL:** https://psf-guard.atpn.co/#/stacks?db=db-e075b4ec&project=4&target=4&current=561&cardsize=600
- **UI state:** Inspect Foraxx SHO 300 s full size open; fit view at 13%; no manual pan. Keep blank edges. Full-resolution image reported 9576 × 6388, three channel stacks, 918 inputs, 76 h 30 m.
- **Reshoot steps:** prepare the card as above, click Inspect, and wait until the loading message disappears and dimensions show 9576 × 6388. Use Fit if needed. Crop the full dialog including its title, counts and zoom controls. Close inspector and restore card size afterwards. No build, download or processing change is needed.
- **Composition:** CSS `(70, 100, 1604, 1004)`; CDP `(63, 90, 1443.6, 903.6)`. Dialog only. Native capture, no image-only export or added labels.
- **Verification:** checked loaded nebula, title, counts, image dimensions and fit controls in final JPEG. Cached processing includes RC-Astro operations; this is not an untouched linear stack. Old Teddy Bear image predates the capture record.

### sky-live.jpg

#### 2026-10-18T01:43:00.439Z — capture

- **File:** 3138 × 1854 px; SHA-256 `52e8a57e2c1a18dd6e81fcbcf991eda88de5ade97ffbd3132b7988546c850e42`.
- **Used by:** `index.html`, `docs/rigs.html`, `docs/sky.html`.
- **Purpose:** Live lists the rigs and their latest reports. These five rigs have not reported yet.
- **Build/environment:** shared live-server environment; viewport 1744 × 1142 CSS px; DPR 1.80.
- **Source URL:** https://psf-guard.atpn.co/#/sky?db=db-a01c581c&project=21&current=8620&q=Heart&mosaic=1&live=1
- **State and reshoot:** SkycoverageEquatorialFlat,all5rigs/filters,183nightsAug6 2025–Oct16 2026,1699h38691frames72targets. Liveon;all5Neverseen/No reportyet,0online. No live pointings available.
- **Composition:** CSS crop (0, 0, 1744, 1030); CDP clip (0.00, 0.00, 1569.60, 927.00), scale 1.

### sky-context.jpg

#### 2026-10-18T01:45:49.388Z — capture

- **File:** 3066 × 1566 px; SHA-256 `5091e673493d5c0a989392890e072abfca8e426ee08802df9a63dfc645b663d0`.
- **Used by:** `index.html`, `docs/astrometry.html`.
- **Purpose:** The solved frame places catalog objects over the image and lists the expected objects in its field.
- **Build/environment:** shared live-server environment; viewport 1744 × 1142 CSS px; DPR 1.80.
- **Source URL:** https://psf-guard.atpn.co/#/detail/10955?db=db-e075b4ec&project=12&target=12&current=10955&filterName=HA&returnTo=sequence
- **State and reshoot:** Golf of Mexico HA frame 10955, Jul 9 2026 23:11, Rejected, hinted solve; Sky context expanded and sky overlay on. Fit 65%. 55 expected objects, center 20h56m49.8s +43d53m29s, scale 1.491 arcsec/px, rotation -179.7 deg, target in frame 91 arcsec from center. Image Information, Quality, Calibration, Satellite tracks folded. Crop image and sky context.
- **Composition:** CSS crop (24, 174, 1704, 870); CDP clip (21.60, 156.60, 1533.60, 783.00), scale 1.

### settings-catalogs.png

#### 2026-10-18T01:21:11.883Z — capture

- **File:** 1878 × 778 px; SHA-256 `d1f9b01ea27999b4d31dcf9e7ed43de1441d9a6b20a79261177d8a7505302ec4`.
- **Used by:** `docs/astrometry.html`.
- **Purpose:** Installed catalog packages support overlays, hinted solves and blind solves.
- **Build/environment:** shared live-server environment; viewport 1744 × 1142 CSS px; DPR 1.80.
- **Source URL:** https://psf-guard.atpn.co/#/grid?db=db-93617230&project=1&current=776
- **State and reshoot:** Settings > Catalogs; six packages installed. Blind solving recommended selected. Overlays, Hinted solve and Blind solve available. No catalog installation or validation run.
- **Composition:** CSS crop (210, 160, 1044, 433); CDP clip (189.00, 144.00, 939.60, 389.70), scale 1.

### satellite-pixel-match.png

#### 2026-10-18 02:36:39 UTC — replacement for the California overlay

- **File:** 3138 × 2166 px, PNG; SHA-256 `0683cd969d69006cb73282ac367045a8eb5f5ef37d235852897bf9bffce6b524`.
- **Used by:** `docs/satellites.html`, opening figure. Replaces the old
  `satellite-california-overlay.png`; the target is SNR G132.
- **Build/environment:** same live-server session as the full guide pass,
  `psf-guard.atpn.co`; version last observed as 0.11.0, deployed commit unknown.
  macOS 27.0.1, Chrome 155.0.8059.82, English, dark theme, 90% browser zoom.
  Viewport 1744 × 1204 CSS px; effective DPR 1.8.
- **Source:** starfront-ultracat131, database `db-93617230`, SNR G132, project 6,
  target 6, frame 1166, R, 300 s. Displayed capture time October 8, 2026,
  2:21:19 AM; displayed time zone was not independently checked. The numeric
  frame ID is verified; the source filename and GUID were not read.
- **URL:** https://psf-guard.atpn.co/#/detail/1166?db=db-93617230&project=6&current=1166&scoreScope=target-filter%3AR&target=6&returnTo=sequence
- **UI state:** Image Information, Quality analysis, Calibration and Sky context
  collapsed. Sky overlay off. Satellite tracks expanded, Track identifiers on.
  Fit zoom (65%), Color enabled. Cached result; no scan started.
- **Observed state:** Accepted grade, 99% quality score; five crossings, two
  high-risk predictions, one pixel match. BREEZE-M DEB (TANK) [40366] is marked
  pixel match, possible risk, 53%. The other four are prediction only. Element
  source Seiza mirror; Elements for 10/8/2026, 5:00:00 AM as displayed. The UI
  states that pixel corridor evidence supports one orbital candidate and that
  identity remains a candidate association. The score and grade are observations,
  not a new grading recommendation.
- **Reshoot:** open the URL; wait for the preview and cached satellite results.
  Turn off Sky overlay before collapsing Sky context. Collapse the other three
  panels, expand Satellite tracks, enable Track identifiers, then click Fit.
  Check that the green fitted path and all five candidate rows are visible.
  Do not press Identify satellite tracks or change the stored grade.
- **Composition:** full viewport; CSS crop (0, 0, 1744, 1204), CDP clip
  (0, 0, 1569.6, 1083.6), scale 1. Direct browser PNG; no retouching or redaction.
- **Verification:** inspected the saved image: target heading, green matched
  path, candidate label, counts and identity qualification are legible.

### export-dialog.png

#### 2026-10-18T01:36:05.172Z — capture

- **File:** 896 × 1242 px; SHA-256 `bc85cd615628f1d5475b97a57eb0d2b3001a41895160ba34ff69f7de1d8d981c`.
- **Used by:** `docs/exporting.html`.
- **Purpose:** Choose the frame selection, folder layout and output format before exporting.
- **Build/environment:** shared live-server environment; viewport 1744 × 1142 CSS px; DPR 1.80.
- **Source URL:** https://psf-guard.atpn.co/#/?db=db-e075b4ec&project=12&target=12&current=10955&filterName=HA&grouping=filter&order=newest&q=Golf+of+Mexico
- **State and reshoot:** Golf of Mexico project Export. Include ungraded on; Grouped by target; Zip of frames. No export started.
- **Composition:** CSS crop (623, 225, 498, 691); CDP clip (560.70, 202.50, 448.20, 621.90), scale 1.

### import-reviewed-transfer.png

#### 2026-10-18T07:06:51.470Z — initial capture

- **File:** SHA-256 `9d34a2fafe12a5543d2babfbb648dea15b522b1fa3f373accebe1c4cbfa52d54`; 2048 × 1120 px; PNG.
- **Used by:** docs/importing.html — Sync with the telescope database.
- **UI and observed state:** Automatic sync and writeback saved off. Merge catalogs: Observatory — Target Scheduler to Observatory. Include thumbnails on. Preview of new project 100 and target 100; Apply not clicked.
- **Composition and reshoot:** follow the shared environment and capture spec above; CSS crop `{"x": 68, "y": 172, "width": 1024, "height": 559.859375}`. No overlays, image processing or zoom/pan apply to these forms.
- **Catalog IDs:** Observatory: `db-969a90ea`.
