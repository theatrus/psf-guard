# App and server updates

PSF Guard publishes two small JSON feeds with different jobs:

- `notice.json` tells browser/server and desktop users that a newer release is
  ready. It links to the release notes and can mark an update as recommended
  or required.
- `updater.json` lets the Tauri desktop app verify, download, and install a
  signed package.

Both checks read `updates.psf-guard.com` first, then the matching file attached
to the latest GitHub release. The notice check keeps the newer valid version
and keeps the website copy when both describe the same release. Tauri uses
GitHub when the website updater feed is unavailable. Browser/server mode never
downloads or installs an executable.

## Public files

Publish these files at:

- `https://updates.psf-guard.com/notice.json`
- `https://updates.psf-guard.com/updater.json`

The update host must allow cross-origin `GET` requests for `notice.json` so a
PSF Guard server on another host can read it. Use `Content-Type:
application/json` and a short cache lifetime. The site publishing job can copy
the two public files from the GitHub release later; it does not need the signing
key.

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

`urgency` may be `normal`, `recommended`, or `required`. The UI also treats the
notice as required when the installed version is older than
`minimum_supported_version`. This changes the notice style and text; it never
forces an install.

## Release flow

A `v*` tag runs `.github/workflows/release.yml`. The job:

1. checks that the tag matches `tauri.conf.json`;
2. builds the normal CLI and desktop packages;
3. signs stable Tauri updater payloads for Windows, macOS, and Linux;
4. builds `updater.json` from the three signatures;
5. builds `notice.json`; and
6. attaches both feeds and the signed payloads to the GitHub release.

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
