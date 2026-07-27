import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { buildReleaseFeeds } from './release-feeds.mjs';
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

test('the shipped CLI is never built with the tauri feature', async () => {
  const releaseWorkflow = await readFile(
    new URL('../.github/workflows/release.yml', import.meta.url),
    'utf8',
  );

  // psf-guard-cli must stay free of tauri: the feature flips the Windows
  // binary to the GUI subsystem, where a console process has no stdout to
  // write to, and drags GTK/WebKit into what is meant to be a plain CLI.
  //
  // The trap is that `cargo tauri build` reads its features from
  // tauri.conf.json and compiles EVERY bin target with them. Without an
  // explicit `--bin psf-guard` it rebuilds psf-guard-cli on top of the
  // tauri-free one built earlier in the job, and "Prepare release assets"
  // ships that. Every tauri build in the release must therefore name the
  // binary it is allowed to touch.
  const tauriBuilds = releaseWorkflow
    .split('\n')
    .filter((line) => /cargo tauri build|package-macos\.sh/.test(line))
    .filter((line) => !line.trimStart().startsWith('#'));

  assert.ok(
    tauriBuilds.length >= 3,
    `expected a tauri build per platform, found ${tauriBuilds.length}`,
  );
  for (const line of tauriBuilds) {
    assert.match(
      line,
      /-- --bin psf-guard$/,
      `tauri build must not be allowed to rebuild psf-guard-cli: ${line.trim()}`,
    );
  }

  // And the tauri-free build it protects has to actually run.
  assert.match(releaseWorkflow, /cargo build --release --locked --bin psf-guard-cli/);
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
