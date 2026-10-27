# Database navigation and shared Settings captures

These are unedited viewport captures from the browser regression tests. The
[capture record](captures.json) records the implementation commit, frontend
asset hashes, fresh backend hash, fixture states, dimensions and PNG hashes.
All earlier images in this directory have been regenerated.

The main suite uses disposable C925 and Ultra Cat databases. Planning
availability is stubbed off, while processing defaults, overrides and database
APIs use the real backend. The pairing-override example saves a shared default
of Off and a C925 override of On, then tests reset to inheritance. The Rig setup
and Sites images use the real Director server, saved optics and shared site
APIs. No live database or paired acquisition client is involved.

The gallery includes the overview, database files, Sync, New / import,
quality analysis, calibrated-copy defaults, a saved database override,
AstroBin filter maps, rig setup and shared Sites. Automatic import is shown
under **Database & files → Edit paths and import**. Those sample form edits
are discarded by the test, so no import schedule is saved.

Desktop captures use Chromium at 1280 × 900 CSS pixels, except Rig setup and
Sites at 1440 × 1000. Phone captures use WebKit at 390 × 844 CSS pixels,
except Rig setup and Sites, which use Chromium at that viewport. WebKit
PNGs have a 2× device pixel ratio. Quality analysis is scrolled into view
inside Settings; the header and phone section chooser remain visible.

To reshoot, build the frontend and backend from the same implementation,
then run these specs through `scripts/e2e-local.mjs`, one run at a time:

- Main suite: `static/e2e/rig-database-navigation.spec.ts`
- Director suite: `static/e2e-director/settings-context.spec.ts`

Copy the PNGs without editing them, update the capture record, and inspect
the final images before attaching them to a PR or using them in a guide.
