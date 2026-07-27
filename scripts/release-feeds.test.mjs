import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import {
  DEFAULT_SUMMARY,
  buildReleaseFeeds,
  readReleaseSummary,
  summaryFromNotes,
} from './release-feeds.mjs';
import { writeUpdaterConfig } from './write-updater-config.mjs';

const execFileAsync = promisify(execFile);

test('labels macOS artifacts as Apple Silicon', async () => {
  const releaseWorkflow = await readFile(
    new URL('../.github/workflows/release.yml', import.meta.url),
    'utf8',
  );
  const ciWorkflow = await readFile(
    new URL('../.github/workflows/ci.yml', import.meta.url),
    'utf8',
  );

  assert.match(releaseWorkflow, /asset_name: psf-guard-macos-arm64/);
  assert.doesNotMatch(releaseWorkflow, /asset_name: psf-guard-macos-x64/);
  // CI publishes one macOS artifact, from the Tauri job. It carries the
  // bundle and target/release/psf-guard, so dropping the test job's separate
  // macOS upload cost no binary. What matters here is the label: these
  // runners are Apple Silicon, and calling the result x64 is the bug this
  // test exists to catch.
  assert.match(ciWorkflow, /name: psf-guard-tauri-macos-arm64/);
  assert.doesNotMatch(ciWorkflow, /name: psf-guard-.*macos-x64/);
});

test('normal and signed builds use website-first updater endpoints', async () => {
  const mainConfig = JSON.parse(await readFile(
    new URL('../tauri.conf.json', import.meta.url),
    'utf8',
  ));
  assert.deepEqual(mainConfig.plugins.updater.endpoints, [
    'https://updates.psf-guard.com/updater.json',
    'https://github.com/theatrus/psf-guard/releases/latest/download/updater.json',
  ]);
  assert.ok(mainConfig.plugins.updater.pubkey);

  const directory = await mkdtemp(path.join(os.tmpdir(), 'psf-guard-updater-'));
  const outputPath = path.join(directory, 'updater-config.json');
  await writeUpdaterConfig(outputPath);
  const buildConfig = JSON.parse(await readFile(outputPath, 'utf8'));
  assert.equal(buildConfig.bundle.createUpdaterArtifacts, true);
  assert.deepEqual(buildConfig.plugins.updater, mainConfig.plugins.updater);
});

test('updater config command writes its output', async (t) => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'psf-guard-updater-cli-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const outputPath = path.join(directory, 'updater-config.json');
  const scriptPath = fileURLToPath(new URL('./write-updater-config.mjs', import.meta.url));

  await execFileAsync(process.execPath, [scriptPath, outputPath]);

  const config = JSON.parse(await readFile(outputPath, 'utf8'));
  assert.equal(config.bundle.createUpdaterArtifacts, true);
});

test('builds signed updater and server notice feeds', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'psf-guard-feeds-'));
  const version = '1.2.3';
  const files = [
    `PSF-Guard-${version}-windows-x64-setup.exe`,
    `PSF-Guard-${version}-linux-x86_64.AppImage`,
    `PSF-Guard-${version}-macos-aarch64.app.tar.gz`,
  ];
  for (const fileName of files) {
    await writeFile(path.join(directory, fileName), 'signed payload\n');
    await writeFile(path.join(directory, `${fileName}.sig`), `sig-${fileName}\n`);
  }

  const feeds = await buildReleaseFeeds({
    artifactDirectory: directory,
    version,
    tag: `v${version}`,
    publishedAt: '2026-07-26T18:00:00Z',
    summary: 'Catalog updates.',
  });

  assert.equal(feeds.updater.version, version);
  assert.equal(
    feeds.updater.platforms['linux-x86_64'].url,
    'https://github.com/theatrus/psf-guard/releases/download/v1.2.3/PSF-Guard-1.2.3-linux-x86_64.AppImage',
  );
  assert.equal(
    feeds.updater.platforms['linux-x86_64'].signature,
    'sig-PSF-Guard-1.2.3-linux-x86_64.AppImage',
  );
  assert.deepEqual(feeds.notice, {
    schema_version: 1,
    version,
    release_url: 'https://github.com/theatrus/psf-guard/releases/tag/v1.2.3',
    summary: 'Catalog updates.',
    urgency: 'normal',
    minimum_supported_version: '0.5.0',
    published_at: '2026-07-26T18:00:00Z',
  });
});

test('rejects a mismatched release tag', async () => {
  await assert.rejects(buildReleaseFeeds({
    artifactDirectory: os.tmpdir(),
    version: '1.2.3',
    tag: 'v1.2.4',
    publishedAt: '2026-07-26T18:00:00Z',
  }), /does not match/);
});

test('summarises release notes with their first sentence', () => {
  const summary = summaryFromNotes([
    '# PSF Guard 0.6.4',
    '',
    'This patch release reorganizes Settings. It had grown into one long page',
    'holding three separate jobs, so reaching any one of them meant scrolling',
    'past the other two. They are now tabs:',
    '',
    '- **Databases** — the catalog list.',
  ].join('\n'));

  // One line in the banner, so one sentence — not the whole paragraph, and
  // not the dangling "They are now tabs:" that introduces the list.
  assert.equal(summary, 'This patch release reorganizes Settings.');
});

test('strips markup and joins wrapped prose', () => {
  const summary = summaryFromNotes([
    '# PSF Guard 1.0.0',
    '',
    'Adds **stacking** for [OSC frames](https://example.com/osc) and',
    '`narrowband` palettes.',
  ].join('\n'));

  assert.equal(summary, 'Adds stacking for OSC frames and narrowband palettes.');
});

test('keeps the default when the notes carry no prose', () => {
  assert.equal(summaryFromNotes('# PSF Guard 1.0.0\n\n- Only a list item.\n'), null);
  assert.equal(summaryFromNotes(''), null);
  assert.equal(summaryFromNotes('# Heading only\n'), null);
});

test('truncates a long opening sentence on a word boundary', () => {
  const long = `This release ${'improves things '.repeat(30)}everywhere.`;
  const summary = summaryFromNotes(`# PSF Guard 1.0.0\n\n${long}\n`);

  assert.ok(summary.length <= 201, `summary was ${summary.length} characters`);
  assert.ok(summary.endsWith('…'));
  assert.ok(!summary.includes('  '));
});

test('reads the summary shipped for a real tag', async () => {
  // Against the checked-in notes, so the extraction cannot drift from what
  // releases actually publish.
  assert.equal(
    await readReleaseSummary('v0.6.4'),
    'This patch release reorganizes Settings.',
  );
  assert.equal(
    await readReleaseSummary('v0.6.3'),
    'This patch release fixes the first-run catalog setup.',
  );
  assert.equal(
    await readReleaseSummary('v0.6.2'),
    'This patch release corrects the name of the stand-alone macOS CLI download.',
  );
});

test('falls back for a tag with no notes file', async () => {
  assert.equal(await readReleaseSummary('v99.99.99'), null);
  assert.equal(DEFAULT_SUMMARY, 'A new PSF Guard release is ready.');
});

test('the four-argument workflow invocation publishes a real summary', async (t) => {
  // This is the shape release.yml uses. It passes no summary, which is how
  // every notice up to 0.6.4 shipped the generic default line.
  const directory = await mkdtemp(path.join(os.tmpdir(), 'psf-guard-feeds-cli-'));
  t.after(() => rm(directory, { recursive: true, force: true }));

  const version = '0.6.4';
  for (const fileName of [
    `PSF-Guard-${version}-windows-x64-setup.exe`,
    `PSF-Guard-${version}-linux-x86_64.AppImage`,
    `PSF-Guard-${version}-macos-aarch64.app.tar.gz`,
  ]) {
    await writeFile(path.join(directory, fileName), 'payload\n');
    await writeFile(path.join(directory, `${fileName}.sig`), `sig-${fileName}\n`);
  }

  const scriptPath = fileURLToPath(new URL('./release-feeds.mjs', import.meta.url));
  await execFileAsync(process.execPath, [
    scriptPath,
    directory,
    version,
    `v${version}`,
    '2026-07-27T06:00:00Z',
  ]);

  const notice = JSON.parse(await readFile(path.join(directory, 'notice.json'), 'utf8'));
  assert.equal(notice.summary, 'This patch release reorganizes Settings.');
  assert.notEqual(notice.summary, DEFAULT_SUMMARY);

  const updater = JSON.parse(await readFile(path.join(directory, 'updater.json'), 'utf8'));
  assert.equal(updater.notes, notice.summary);
});
