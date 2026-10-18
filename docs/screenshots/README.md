# README screenshot sources

The main README reuses these two documentation-site captures as of October 18,
2026. This edit copies the files without changes and makes no new capture. Keep
the filenames and recorded states together when reshooting. The shared
environment below describes the original three-image pass; this directory
uses its mosaic and calibration images.

Source repository: `theatrus/psf-guard-docs`, commit
`9e9391f19efd3d88104d66ed7800f52d23ac00ed`. The original
[capture record](https://github.com/theatrus/psf-guard-docs/blob/9e9391f19efd3d88104d66ed7800f52d23ac00ed/assets/screenshot-captures.md)
contains the same shared environment and per-image entries below. Both copies
match the recorded SHA-256; neither was resized, edited, or annotated.

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
