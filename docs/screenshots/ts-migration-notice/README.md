# Target Scheduler migration notice captures

These captures show the updated 0.12 notice with its neutral panel and accent
border. [captures.json](captures.json) records the source commit, image hashes,
viewports, crop, UI state and reshoot steps for all four images.

The guarded browser test used an isolated registry and cache. The server
reported 0.12.0 and served the freshly built frontend through `--static-dir`.
The Rust binary's source commit was not recorded. No live catalog changed.

The popup opens automatically, with no saved acknowledgment, over an empty
Library. Deprecations appear first; the preference is unchecked and dismiss
controls stay visible. Mobile uses 390 × 700 CSS px; desktop uses 1280 × 900,
with device scale 1 in Chromium and 2 in WebKit. The body scrolls on mobile. Images are full viewport
PNGs with no crop, resizing or annotations.

To reshoot, build the frontend at the recorded source or a later revision and
follow the wrapper and guarded test command in the JSON record. Run one local
Playwright session at a time. Inspect each image before replacing it, and keep
this record with the files. These are browser captures, not native app captures.

## Revised copy

The `*-plain-copy.png` captures show the approved wording, with separate
paragraphs for the upgrade plan and its benefit.
[captures-plain-copy.json](captures-plain-copy.json) records this reshoot's source,
state, viewports, crops, hashes and steps. The earlier images and their record
remain unchanged.
