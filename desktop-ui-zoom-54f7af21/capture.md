# Desktop UI zoom review captures

Source: `54f7af211931b7ee0a18c3efd4d9945d20cff5ee`.
The browser test stubs the Tauri bridge and uses CSS zoom to show the scale change;
these images are not native app captures.

WebKit Desktop Safari, macOS 27.0.1, dark theme, reduced motion, 1440 × 1000 CSS
pixels at device scale 2. Route `/`. The `ui-zoom-test` disposable catalog has two
projects and four real FITS images. Display preferences select detailed Library
cards; release notes are acknowledged. Menu image: 110%, menu open. Settings
image: Databases open, 120% after Ctrl+=. No edits to catalog data. No crop,
redaction or image resizing. The pointer is absent. Exact dimensions and hashes
are in capture.json.

To reshoot, build the frontend and run from the repository root:

```bash
PSF_GUARD_E2E_BINARY=/path/to/server-wrapper scripts/e2e-local.mjs e2e/desktop-zoom.spec.ts
```

The wrapper execs the built server with its received arguments and adds
`--static-dir /absolute/path/to/static/dist`. The test sets the fixture state and
writes the two PNGs under static/test-results. A reshoot may change the fixture
temporary path shown in Settings. All test registries and caches are disposable.
