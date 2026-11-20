# Release guide

This guide covers a normal PSF Guard desktop, CLI, package, and server release.
Prepare the release in a pull request. Do not tag until that exact pull request
has merged and all required checks have passed.

A minor release comes from `main`. When `main` carries work that should not
ship yet, a patch release comes from that minor's release branch instead; see
[Patch releases from a release branch](#patch-releases-from-a-release-branch).
Between releases, `main` carries the next minor version; see
[Main's version between releases](#mains-version-between-releases).

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

Move the entries from `docs/releases/unreleased.md`, which collects each
user-visible change as it merges, into `docs/releases/vVERSION.md`. Read them against
`git log vPREVIOUS..origin/main` and fill in anything that landed without an
entry. If `vVERSION.md` already exists, merge the new entries into it instead
of replacing it. Keep adding changes to the same version until it is tagged,
and check for new entries again before tagging. Rewrite the opening sentence
for this version, drop the note about renaming the file, and reset
`unreleased.md` after transferring its entries.

Update the website's release notes and overview, the relevant user guides,
and the bundled in-app highlights in the same pass. Check new prose against
the writing guide, preserve UI labels, and remove claims that later changes
made obsolete.

Search for the old version after the edit. Test fixtures and dependency versions
may still use the same number; inspect each match instead of replacing all of
them.

The release workflow prepends the matching `docs/releases/vVERSION.md` file to
GitHub's generated change list. A missing file makes the release job fail.

Add an entry for the version at the start of
`static/src/updates/highlights.json` too.
The app uses it for **What's new** after an upgrade and from the menu. Keep
older entries so people who skip releases still see their warnings. Each
entry needs `version`, `deprecations`, `upgradeNotes` and `highlights`.
Each item has a short `title` and plain-text `body`. Use an empty array when a
section has nothing to say. Put removals and deprecated features in
`deprecations`, with the replacement and any deadline. Put required steps and
changed defaults in `upgradeNotes`. Choose a few main features for
`highlights`; aim for six to eight short items and leave the full account in
feature-grouped Markdown release notes. Combine entries that describe the same
feature, and remove older claims that later changes replaced. Keep deprecations
and upgrade steps ahead of feature highlights. Use absolute, versioned links to
repository guides: this Markdown also becomes the GitHub release body.
The release version check rejects a tag without matching highlights.

Open the notes with a plain sentence that says what the release does. That
first sentence becomes the summary in `notice.json`, which is the single line
every installed copy sees in its update banner, so write it for someone who
has not read the rest of the file. Lead with a list or a heading and the
banner falls back to "A new PSF Guard release is ready."

After that opening paragraph, add direct download links for the Windows setup
EXE and MSI, macOS DMG, and Linux AppImage and Debian package. Use a
**Download the desktop app** heading and a table so readers can choose without
searching GitHub's asset list:

| Computer | Link label | Asset |
| --- | --- | --- |
| Windows (64-bit) | Setup EXE — recommended | `PSF-Guard-VERSION-windows-x64-setup.exe` |
| Windows (64-bit) | MSI | `PSF.Guard_VERSION_x64_en-US.msi` |
| macOS (Apple Silicon) | Download for macOS (.dmg) | `PSF.Guard_VERSION_aarch64.dmg` |
| Linux (64-bit) | AppImage | `PSF-Guard-VERSION-linux-x86_64.AppImage` |
| Debian / Ubuntu (64-bit) | DEB package | `PSF.Guard_VERSION_amd64.deb` |

Link each label to the release's absolute, versioned asset URL. Keep MSI as a
Windows choice. Call the DMG the macOS download; do not label it "manual
install," which makes the normal download sound like a special case.

Below the table, explain that installed desktop apps offer **Install update**.
Readers do not need to download `.app.tar.gz`, `.sig`, `updater.json`, or
`notice.json` themselves: PSF Guard uses those files for update checks and
automatic updates. Put standalone CLI binaries and the Fedora RPM in a
separate server/command-line paragraph, linked to the
[installation guide](https://psf-guard.com/docs/install.html).

Check every download link against the actual release asset names before
publishing. Keep the source notes, published GitHub notes and website notes
in sync. Preserve GitHub's generated change list when editing a published
release body with `gh release edit --notes-file`.

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

Use Node 24.15 or newer in the 24.x LTS line, as in CI. Parse `.github/workflows/ci.yml` and
`.github/workflows/release.yml`, then run `git diff --check`. Build a local
Tauri bundle when app, updater, packaging, or signing code changed.

Read the generated summary as a complete sentence, not just a valid JSON
string. In 0.14, the sentence parser cut the summary at `N.I.N.A.`. The parser
now handles that name within a sentence, and the feed tests cover it. Check
both `notice.json.summary` and `updater.json.notes` after feed generation.

If a browser test fails after import, check whether the API still reports
loading before treating missing data as a product failure. Import can finish
before the file-cache refresh; the 0.14 planning-entry test now polls for a
successful project list. Wait for the required state, not a fixed delay.

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
2. record the merge SHA and compare its tree with the reviewed head using
   `git diff --exit-code REVIEWED_HEAD_SHA MERGE_SHA --` (a squash merge does
   not retain the head SHA as an ancestor);
3. confirm `Cargo.toml`, `Cargo.lock`, `tauri.conf.json`, and the RPM spec all
   carry the same version;
4. create an annotated `vVERSION` tag on that merge SHA; and
5. push only that tag.

```bash
git fetch origin
git tag -a vVERSION MERGE_SHA -m "PSF Guard VERSION"
git push origin vVERSION
```

Never move a published release tag or replace a published executable with a
different build. Ship binary fixes in a new version. Corrections to release text
can keep the version; see [Correct published release text](#correct-published-release-text).

## 5. Watch the hosted release

The tag starts the release, RPM and Docker workflows. Watch all jobs for the
exact tag SHA, including the security checks. The release workflow:

1. signs a probe on each runner;
2. builds the CLI and desktop packages;
3. requires each Tauri updater payload and its `.sig` file;
4. uploads platform assets;
5. creates `updater.json` from the three payload signatures; and
6. uploads `updater.json` and `notice.json` to the GitHub release.

Tauri signs updater payloads, not the JSON manifest. The manifest carries the
signature that the installed app checks with its embedded public key.

Publish one setup EXE and one AppImage, using the updater names for desktop
downloads too: `PSF-Guard-VERSION-windows-x64-setup.exe` and
`PSF-Guard-VERSION-linux-x86_64.AppImage`. Keep the Windows MSI, Linux Debian
and RPM packages, and standalone CLI binaries. On macOS, offer the DMG as the
desktop download and retain the `.app.tar.gz` for signed updates. Do not publish
an extra app ZIP, setup EXE, or AppImage. Keep all updater payloads, their
`.sig` files, `updater.json`, and `notice.json`.

Do not call the release complete because CI is green. Wait until GitHub shows a
public, non-draft release and all expected assets.

```bash
gh run list --workflow release.yml --branch vVERSION
gh run watch RUN_ID --exit-status
gh release view vVERSION
```

Check live job output before retrying a slow job. The macOS **Run tests** step
also compiles the test binaries; a long compile is not evidence of a failed
test.

### Recover from an Apple agreement failure

If notarization reports a missing or expired agreement, have the Apple
Developer Account Holder accept it at
[Apple Developer account](https://developer.apple.com/account). After they
confirm acceptance, wait for the failed workflow to finish, then rerun its
failed jobs:

```bash
gh run rerun RUN_ID --failed
```

Verify that notarization reports `Accepted` and that stapling succeeds. An
unsigned pull-request build cannot prove this; check the signed macOS job on
the merged commit and the release job. The 0.14 failure needed agreement
acceptance, not new signing credentials.

## 6. Verify public artifacts

Download the release into a new temporary directory. Check:

- the tag and release target the recorded merge SHA;
- all Windows, macOS, Linux, CLI, RPM, `updater.json`, and `notice.json` assets
  exist and are non-empty;
- every URL and signature in `updater.json` matches its public release asset;
- updater payload signatures verify with the public key embedded in
  `tauri.conf.json`, and downloaded file hashes match GitHub's asset digests;
- the GitHub `releases/latest/download/updater.json` fallback works;
- `ghcr.io/theatrus/psf-guard:latest` resolves to the same image index as the
  version tag (`docker buildx imagetools inspect` both, or compare the
  manifest digests through the registry API); `latest` moves only on a
  `vX.Y.Z` tag, `edge` follows main, and a wrong `latest` is repaired by
  running the Docker workflow by hand with `retag_latest` set to the version;
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

If an older workflow uploaded duplicate downloads, save copies and hashes
before removing them. Compare duplicate EXEs or AppImages with the retained
files, and confirm that no updater URL points to a file being removed. Keep
MSI and every signed updater payload. Fix the workflow too, so the next
release does not upload the duplicates again.

## 7. Check the website feeds and the demo

`updates.psf-guard.com` picks up `updater.json` and `notice.json` from the
published GitHub release through a webhook; nothing is copied by hand. After
the assets pass verification, confirm both public URLs serve the new version:

```bash
curl -s https://updates.psf-guard.com/notice.json
curl -s https://updates.psf-guard.com/updater.json
```

Check the full feed contents, including the summary, against the files on
GitHub. A matching version alone does not prove a text correction reached the
website. If either feed is stale, check the release webhook delivery and the
mirror's logs. An HTTP 202 response means the hook accepted the event, not
that deployment finished.

Then start an isolated PSF Guard server with an empty test registry, or restart
a server when its operator is ready. The server checks the notice feed at
startup and caches it for 24 hours. Reloading a browser does not refresh the
feed; a running server may show old text until its next check. A desktop app
on the previous version should offer the signed update. A browser should show the notice but must not
offer an install action.

`demo.psf-guard.com` rebuilds itself from the newest `vX.Y.Z` tag on the
same release webhook (flotswarm action `deploy-psf-guard`, host
`psf-guard-demo`), so it always runs what users download and never the main
tip. Each asset upload edits the release and sends the webhook again, so give
it time after the last asset upload, then confirm the version it runs:

```bash
curl -s https://demo.psf-guard.com/api/info
```

If the demo still reports the previous version, inspect the deployment log,
active build and service status before starting another build. During 0.14,
host restarts and queued builds delayed deployment; a later job waited for
Cargo's build lock. The existing build finished and the queued deployment
restarted the service. A fixed waiting period would not have verified that.

Record the tag SHA, workflow run, release URL, downloaded checks, and website
feed checks in the release issue or operator notes.

### Correct published release text

Editing the GitHub release body does not rebuild its JSON assets. To correct
the update banner without rebuilding the app:

1. Fix `docs/releases/vVERSION.md` and the matching published notes. Keep the
   opening summary short and readable on its own.
2. Save the current feeds, signed payloads and signatures. Regenerate the
   feeds from those artifacts with `scripts/release-feeds.mjs`, using the
   original version, tag and publication time.
3. Compare the JSON before uploading. A summary correction changes only
   `notice.json.summary` and `updater.json.notes`. Preserve payload URLs,
   signatures, dates and every other field.
4. Upload only the two corrected JSON assets with `gh release upload vVERSION
   updater.json notice.json --clobber`. Keep the published tag and binaries.
5. Wait for the mirror and compare both public feeds with GitHub again. Check
   the GitHub latest-release fallback too, then verify the text through a
   fresh server process with an empty test registry.

### Fixes from 0.14

- [Release PR #1120](https://github.com/theatrus/psf-guard/pull/1120)
  fixed the planning-entry test's loading-state race. The signed macOS retry
  passed after the Account Holder accepted Apple's agreement.
- [Release cleanup #1121](https://github.com/theatrus/psf-guard/pull/1121)
  removed the extra app ZIP and duplicate EXE/AppImage, kept MSI and updater
  files, and fixed summary parsing at `N.I.N.A.`. Tests exercise each
  platform's asset staging and preserve signed payload bytes.
- [Website notes #42](https://github.com/theatrus/psf-guard-docs/pull/42)
  corrected installer names and the release summary. The GitHub release's
  later download table uses **Download for macOS (.dmg)** and separates
  installer choices from updater files.
- The update site now picks up corrected release text for the current version.
  During 0.14, it kept serving the old summary after the GitHub files changed.
  Checking both copies confirmed the correction reached users.

## Patch releases from a release branch

Release fixes from `release/vX.Y` when `main` holds unreleased work that must
not ship in a patch. 0.12.4 was the first release made this way.

1. If `release/vX.Y` does not exist, create it from the last `vX.Y.Z` tag and
   push it.
2. Merge each fix to `main` first, through its own pull request. Then
   cherry-pick it onto a `release/vX.Y.Z` branch cut from `release/vX.Y`, and
   resolve conflicts against the release branch. Code that exists only on
   `main` stays out.
3. Tag workflows run from the files at the tagged commit. When `main` changed
   the runners or CI that the branch needs in order to build, bring those
   workflow files over too, without jobs for code the branch lacks.
4. Prepare the release as in section 2, in a pull request whose base is
   `release/vX.Y`. Write the fixes' entries straight into `vVERSION.md`, and
   leave the branch's `unreleased.md` reset.
5. Run the local gates, then merge, tag and verify as in sections 3 to 7. As
   for any release, Docker `latest`, `notice.json` and the demo move to the
   patch when it is the newest version.

The release branch's notes, highlights and RPM changelog entry do not reach
`main` on their own. When the next minor release is prepared on `main`, copy
`docs/releases/vX.Y.Z.md`, its `highlights.json` entry and its RPM changelog
entry from the release branch, and remove the entries in `main`'s
`unreleased.md` that the patch already shipped, so the minor's notes do not
announce them a second time.

## Main's version between releases

Once a release branch exists, stamp `main` with the next minor version as an
interim development version: `Cargo.toml`, the root package in `Cargo.lock`,
`tauri.conf.json`, and the RPM spec, whose changelog gets the line "Interim
development version on main; released builds come from release/* branches".
Add no release notes or highlights for it. 0.11.0 (#545) and 0.13.0 were
stamped this way.

Under the last release's number, every build from `main` reports each new
patch as an available update, though it already has the patch's fixes. The
end-to-end servers run with
`PSF_GUARD_UPDATE_NOTICE=off`, so a release does not change their pages
whatever `main`'s version is. The minor release keeps the interim number and
replaces the interim changelog line with its own entry.
