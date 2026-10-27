# Rig database Settings captures

These are unedited viewport captures from the browser regression tests. The
[capture record](captures.json) includes the frontend asset hashes, backend
binary hash, fixture state, dimensions and PNG hashes.

The parent list, Database & files and Sync captures use disposable catalogs
named C925 and Ultra Cat. Planning is stubbed off in that test so it also
covers access to files and sync without Planning. The Rig setup captures use
the Director test server and real saved optics in disposable catalogs;
Planning and the rig profile APIs are not mocked.

Desktop captures use Chromium at 1280 × 900 CSS pixels (Rig setup: 1440 ×
1000). Phone captures use WebKit at 390 × 844 CSS pixels, except Rig setup,
which uses Chromium at the same viewport. The WebKit PNGs have a 2× device
pixel ratio. No user data, paired equipment or live catalogs are involved.

To reshoot, build `static/dist`, then run these specs through
`scripts/e2e-local.mjs`, one run at a time:

- Main suite: `static/e2e/rig-database-navigation.spec.ts`
- Director suite: `static/e2e-director/settings-context.spec.ts`

Copy the PNGs without editing them and update the capture record. Inspect
all final images before attaching them to a PR or using them in a guide.
