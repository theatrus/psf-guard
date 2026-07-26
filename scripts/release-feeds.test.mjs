import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { buildReleaseFeeds } from './release-feeds.mjs';
import { writeUpdaterConfig } from './write-updater-config.mjs';

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
