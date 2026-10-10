# App and server updates

PSF Guard publishes two small JSON feeds with different jobs:

- `notice.json` tells browser/server and desktop users that a newer release is
  ready. It links to the release notes and can mark an update as recommended
  or required.
- `updater.json` lets the Tauri desktop app verify, download, and install a
  signed package.

The PSF Guard server reads `updates.psf-guard.com` first, then the matching file
attached to the latest GitHub release. It keeps the newer valid notice and
keeps the website copy when both describe the same release. The process caches
that result for 24 hours, refreshes once at startup and once per day, and serves
the cached result to every browser. Reloading the UI never fetches either public
feed. Tauri uses GitHub when the website updater feed is unavailable.
Browser/server mode never downloads or installs an executable.

## Open browser tabs

A browser tab can outlive the server build that served it. The Vite build
stamps `index.html` with `<meta name="psf-guard-build">`, a hash of the page
and every file the build wrote, and the server names that build in an
`X-PSF-Guard-Build` header on every reply. When an API reply names a build
other than the page's, the UI shows **PSF Guard was updated** with a
**Reload now** button, and reloads by itself the next time the user changes
views (Library, Images, Sequence, Stacks, Sky, Planning). Moving inside a view,
such as from the grid to an image, never reloads. A page with unsaved plan
edits asks first, as it does for any other way of leaving. A tab reloads by
itself once per server build, so a cache that keeps serving the old page
cannot make it reload on every change of view. The desktop app's page ships
inside the app and never takes part.

The server sends `index.html` and every other file outside `assets/` with
`Cache-Control: no-cache`, so a reload always checks for a new page. Vite names
the files in `assets/` by their content, so those are cached for a year. A
request for an `assets/` file the server does not hold, which only a page from
an older build makes, gets a 404 rather than `index.html`.

## Public files

Publish these files at:

- `https://updates.psf-guard.com/notice.json`
- `https://updates.psf-guard.com/updater.json`

Use `Content-Type: application/json` and a short cache lifetime. The site
publishing job can copy the two public files from the GitHub release later; it
does not need the signing key.

The notice schema is:

```json
{
  "schema_version": 1,
  "version": "0.6.0",
  "release_url": "https://github.com/theatrus/psf-guard/releases/tag/v0.6.0",
  "summary": "Improves catalog review and stack previews.",
  "urgency": "normal",
  "minimum_supported_version": "0.5.0",
  "published_at": "2026-07-26T18:00:00Z"
}
```

`summary` is the first sentence of `docs/releases/<tag>.md`, shown on one line
in the update banner. It falls back to "A new PSF Guard release is ready."
when those notes open with something other than prose.

`urgency` may be `normal`, `recommended`, or `required`. The UI also treats the
notice as required when the installed version is older than
`minimum_supported_version`. This changes the notice style and text; it never
forces an install.

## Release flow

A `v*` tag runs `.github/workflows/release.yml`. The job:

1. signs a small probe on each runner to check the GitHub signing secrets;
2. checks that the tag matches `tauri.conf.json`;
3. builds the normal CLI and desktop packages;
4. signs stable Tauri updater payloads for Windows, macOS, and Linux;
5. requires a non-empty signature beside each payload;
6. builds `updater.json` from the three signatures;
7. builds `notice.json`; and
8. attaches both feeds and the signed payloads to the GitHub release.

Tauri does not sign `updater.json` itself. The manifest carries one signature
for each platform payload. The app checks that signature with the public key
embedded in `tauri.conf.json` before it installs the downloaded file.

The workflow does not publish to `updates.psf-guard.com`. Keep that release
sync in a separate job or repository.

Run the feed tests with:

```bash
cd static
npm run test:release
```

## Signing key

Released apps contain only the public key from `tauri.conf.json`. The private
key and its password are stored in the local macOS Keychain under:

- `PSF Guard Tauri Updater Private Key`
- `PSF Guard Tauri Updater Password`

The release workflow reads GitHub Actions secrets named
`TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Copy the
Keychain values into those secrets without printing them:

```bash
security find-generic-password -a "$USER" \
  -s "PSF Guard Tauri Updater Private Key" -w |
  gh secret set TAURI_SIGNING_PRIVATE_KEY

security find-generic-password -a "$USER" \
  -s "PSF Guard Tauri Updater Password" -w |
  gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD
```

Do not place either private value in the repository, a release asset, or the
website sync job.
