# Release guide

This guide covers a normal PSF Guard desktop, CLI, package, and server release.
Prepare the release in a pull request. Do not tag until that exact pull request
has merged and all required checks have passed.

## 1. Check the release boundary

1. Fetch `origin/main` and the tags.
2. Read the full change list from the last release:

   ```bash
   git log --oneline vPREVIOUS..origin/main
   git diff --stat vPREVIOUS..origin/main
   ```

3. Choose the next semantic version. Use a patch release for compatible fixes,
   a minor release for compatible features, and a major release for breaking
   user-facing or data-format changes.
4. Check open pull requests for a change that must ship with the release.
5. Confirm that GitHub Actions lists the Apple signing secrets and the two
   updater secrets: `TAURI_SIGNING_PRIVATE_KEY` and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Check names only; never print values.

## 2. Prepare the release pull request

Create an isolated worktree from current `origin/main`. Update:

- `Cargo.toml` package version;
- `Cargo.lock` root package version;
- `tauri.conf.json` app version;
- `packaging/rpm/psf-guard.spec` version and changelog; and
- `docs/releases/vVERSION.md` release highlights.

Search for the old version after the edit. Test fixtures and dependency versions
may still use the same number; inspect each match instead of replacing all of
them.

The release workflow prepends the matching `docs/releases/vVERSION.md` file to
GitHub's generated change list. A missing file makes the release job fail.

Open the notes with a plain sentence that says what the release does. That
first sentence becomes the summary in `notice.json`, which is the single line
every installed copy sees in its update banner, so write it for someone who
has not read the rest of the file. Lead with a list or a heading and the
banner falls back to "A new PSF Guard release is ready."

## 3. Run local gates

Run the same broad checks used by CI:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo build --release --locked --bin psf-guard-cli
node scripts/check-release-version.mjs vVERSION

cd static
npm ci
npm run lint
npx vitest run
npm run build
npm run test:release
```

Use Node 24, which is the version in CI. Parse `.github/workflows/ci.yml` and
`.github/workflows/release.yml`, then run `git diff --check`. Build a local
Tauri bundle when app, updater, packaging, or signing code changed.

Commit only the release files and any release-blocking fix found during the
audit. Push the branch and open a pull request. Record local checks in its body.

## 4. Merge and tag the reviewed commit

Immediately before merging, record the pull request head SHA and inspect
`mergeStateStatus` and every required check. If the head changed, review the
new diff and wait for checks on that SHA.

```bash
gh pr view PR --json headRefOid,mergeStateStatus,statusCheckRollup
gh pr checks PR
gh pr merge PR --squash --match-head-commit REVIEWED_HEAD_SHA
```

After merge:

1. fetch `origin/main` again;
2. record the merge SHA and confirm it contains the reviewed head;
3. confirm `Cargo.toml`, `Cargo.lock`, `tauri.conf.json`, and the RPM spec all
   carry the same version;
4. create an annotated `vVERSION` tag on that merge SHA; and
5. push only that tag.

```bash
git fetch origin
git tag -a vVERSION MERGE_SHA -m "PSF Guard VERSION"
git push origin vVERSION
```

Never move a published release tag. Fix a bad public release with a new version.

## 5. Watch the hosted release

The tag starts `.github/workflows/release.yml` and the RPM workflow. Watch all
jobs for the exact tag SHA. The release workflow:

1. signs a probe on each runner;
2. builds the CLI and desktop packages;
3. requires each Tauri updater payload and its `.sig` file;
4. uploads platform assets;
5. creates `updater.json` from the three payload signatures; and
6. uploads `updater.json` and `notice.json` to the GitHub release.

Tauri signs updater payloads, not the JSON manifest. The manifest carries the
signature that the installed app checks with its embedded public key.

Do not call the release complete because CI is green. Wait until GitHub shows a
public, non-draft release and all expected assets.

```bash
gh run list --workflow release.yml --branch vVERSION
gh run watch RUN_ID --exit-status
gh release view vVERSION
```

## 6. Verify public artifacts

Download the release into a new temporary directory. Check:

- the tag and release target the recorded merge SHA;
- all Windows, macOS, Linux, CLI, RPM, `updater.json`, and `notice.json` assets
  exist and are non-empty;
- every URL and signature in `updater.json` matches its public release asset;
- the GitHub `releases/latest/download/updater.json` fallback works;
- the CLI reports the new version on each platform available to you; and
- package and app metadata carry the new version.

```bash
mkdir /tmp/psf-guard-vVERSION
gh release download vVERSION --dir /tmp/psf-guard-vVERSION
```

On macOS, verify the DMG, notarization ticket, Gatekeeper result, deep code
signature, architectures, and bundled app version:

```bash
hdiutil verify PSF.Guard_VERSION_*.dmg
xcrun stapler validate PSF.Guard_VERSION_*.dmg
spctl --assess --type open --context context:primary-signature -v *.dmg
codesign --verify --deep --strict --verbose=2 /path/to/PSF\ Guard.app
```

Keep the downloaded files until the release record and update feeds have both
been checked.

## 7. Publish and check the website feeds

The PSF Guard repository does not write to `updates.psf-guard.com`. Run the
separate website sync after the GitHub assets pass verification. Copy
`updater.json` and `notice.json` without changing their contents.

Check both public URLs, then start or refresh a PSF Guard server. The server
checks the notice feed at startup and caches it for 24 hours. A desktop app on
the previous version should offer the signed update. A browser should show the
notice but must not offer an install action.

Record the tag SHA, workflow run, release URL, downloaded checks, and website
feed checks in the release issue or operator notes.
