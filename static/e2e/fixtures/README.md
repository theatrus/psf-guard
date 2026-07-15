# e2e FITS fixtures

The Playwright suite drives the real `psf-guard` server end-to-end, including
the preview / detail / sequence-analysis flows that load FITS files from
disk. Those FITS files are too large (~117 MB each) to commit, so we host
them as **release attachments on GitHub** and download them on demand.

The suite also contains `objects.bin.gz.b64`, a compressed one-object
`SEIZAOB3` catalog. Global setup expands it into the isolated astrometry data
directory and points the test registry at it. This keeps catalog capability
and validation tests offline while still exercising Seiza's real indexed-file
reader through the running PSF Guard server.

## Contents

- `manifest.json` — list of fixture filenames + sha256 + release tag. Update
  this whenever a new release tag is cut. The downloader rejects files that
  fail checksum verification.
- `loader.ts` — small library that consults the manifest, falls back to a
  local cache directory (default `~/.cache/psf-guard-e2e-fixtures/`), and
  downloads + verifies missing files.
- `objects.bin.gz.b64` — deterministic M 65 catalog used by the astrometry API
  end-to-end tests; it does not need to be uploaded with the FITS release.

## Local cache layout

```
~/.cache/psf-guard-e2e-fixtures/
  2026-04-16_22-25-11_B_-10.00_60.00s_0028.fits
  2026-04-16_22-26-17_B_-10.00_60.00s_0029.fits
  2026-04-16_22-27-23_B_-10.00_60.00s_0030.fits
  2026-04-17_00-06-56_B_-10.00_60.00s_0104.fits
```

Override with `PSF_GUARD_E2E_FIXTURE_CACHE` if you want them elsewhere.
Override `PSF_GUARD_E2E_FIXTURE_BASE` to point at a private mirror.

## Uploading a new fixture set

1. Drop the new FITS files into `~/.cache/psf-guard-e2e-fixtures/`.
2. Recompute checksums and edit `manifest.json` (fields: `name`, `sha256`,
   `size_mb`, `release_tag`, `base_url`).

   ```bash
   shasum -a 256 ~/.cache/psf-guard-e2e-fixtures/*.fits
   ```

3. Create a GitHub release (or update the existing one) named after the
   `release_tag` and attach the files:

   ```bash
   gh release create e2e-fixtures-v1 \
     ~/.cache/psf-guard-e2e-fixtures/*.fits \
     --title "e2e FITS fixtures v1" \
     --notes "Test data for static/e2e/. See static/e2e/fixtures/README.md."
   ```

4. Bump the manifest's `release_tag` + `base_url` to point at the new tag,
   commit, and push.

The CI job hits the same release URL via the manifest, with the
Playwright-browser cache step also doubling as a fixture cache.
