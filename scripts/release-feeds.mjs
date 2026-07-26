import { readFile, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

export const NOTICE_SCHEMA_VERSION = 1;

const platformAssets = (version) => ({
  'windows-x86_64': `PSF-Guard-${version}-windows-x64-setup.exe`,
  'linux-x86_64': `PSF-Guard-${version}-linux-x86_64.AppImage`,
  'darwin-aarch64': `PSF-Guard-${version}-macos-aarch64.app.tar.gz`,
});

function validVersion(version) {
  return /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version);
}

function releaseAssetUrl(repository, tag, fileName) {
  return `https://github.com/${repository}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(fileName)}`;
}

export async function buildReleaseFeeds({
  artifactDirectory,
  version,
  tag,
  repository = 'theatrus/psf-guard',
  publishedAt,
  summary = 'A new PSF Guard release is ready.',
  minimumSupportedVersion = '0.5.0',
}) {
  if (!validVersion(version)) throw new Error(`Invalid release version: ${version}`);
  if (tag !== `v${version}`) {
    throw new Error(`Release tag ${tag} does not match v${version}.`);
  }
  if (Number.isNaN(Date.parse(publishedAt))) {
    throw new Error(`Invalid release date: ${publishedAt}`);
  }

  const platforms = {};
  for (const [target, fileName] of Object.entries(platformAssets(version))) {
    const payload = await stat(path.join(artifactDirectory, fileName));
    if (!payload.isFile() || payload.size === 0) {
      throw new Error(`The updater payload ${fileName} is empty.`);
    }
    const signature = (
      await readFile(path.join(artifactDirectory, `${fileName}.sig`), 'utf8')
    ).trim();
    if (!signature) throw new Error(`The updater signature for ${fileName} is empty.`);
    platforms[target] = {
      signature,
      url: releaseAssetUrl(repository, tag, fileName),
    };
  }

  const releaseUrl = `https://github.com/${repository}/releases/tag/${encodeURIComponent(tag)}`;
  return {
    updater: { version, notes: summary, pub_date: publishedAt, platforms },
    notice: {
      schema_version: NOTICE_SCHEMA_VERSION,
      version,
      release_url: releaseUrl,
      summary,
      urgency: 'normal',
      minimum_supported_version: minimumSupportedVersion,
      published_at: publishedAt,
    },
  };
}

async function main() {
  const [artifactDirectory, version, tag, publishedAt, summary] = process.argv.slice(2);
  if (!artifactDirectory || !version || !tag || !publishedAt) {
    throw new Error(
      'Usage: node scripts/release-feeds.mjs ARTIFACT_DIR VERSION TAG PUBLISHED_AT [SUMMARY]',
    );
  }
  const feeds = await buildReleaseFeeds({
    artifactDirectory,
    version,
    tag,
    publishedAt,
    summary: summary || undefined,
  });
  await Promise.all([
    writeFile(
      path.join(artifactDirectory, 'updater.json'),
      `${JSON.stringify(feeds.updater, null, 2)}\n`,
    ),
    writeFile(
      path.join(artifactDirectory, 'notice.json'),
      `${JSON.stringify(feeds.notice, null, 2)}\n`,
    ),
  ]);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) await main();
