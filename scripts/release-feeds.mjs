import { readFile, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

export const NOTICE_SCHEMA_VERSION = 1;

export const DEFAULT_SUMMARY = 'A new PSF Guard release is ready.';

/** Longest summary the in-app notice can show without crowding its one line. */
const SUMMARY_LIMIT = 200;

/**
 * First sentence of a release-notes file, for the in-app update notice.
 *
 * The notice renders one line, so it wants a sentence rather than the whole
 * opening paragraph. Every docs/releases/vX.Y.Z.md so far opens with prose
 * that summarises the release in its first sentence — "This patch release
 * fixes the first-run catalog setup." — which is exactly what an installed
 * copy should be told.
 *
 * Returns null when there is no usable prose, so callers keep the default
 * rather than showing something mangled.
 */
export function summaryFromNotes(markdown) {
  const lines = markdown.split('\n');
  const paragraph = [];
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed === '') {
      if (paragraph.length > 0) break;
      continue;
    }
    // Skip the title and stop at anything that is not plain prose: a list,
    // quote, table, or fence means the prose paragraph is over.
    if (/^#/.test(trimmed)) {
      if (paragraph.length > 0) break;
      continue;
    }
    if (/^([-*+>|]|\d+\.|```)/.test(trimmed)) {
      if (paragraph.length > 0) break;
      continue;
    }
    paragraph.push(trimmed);
  }
  if (paragraph.length === 0) return null;

  const prose = paragraph
    .join(' ')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1') // links keep their text
    .replace(/[*_`]/g, '')
    .replace(/\s+/g, ' ')
    .trim();
  if (prose === '') return null;

  // First sentence. Require a few characters before the period so an
  // abbreviation does not end it early.
  const sentence = /^(.{12,}?[.!?])(?:\s|$)/.exec(prose);
  let summary = sentence ? sentence[1] : prose;

  if (summary.length > SUMMARY_LIMIT) {
    const clipped = summary.slice(0, SUMMARY_LIMIT);
    const lastSpace = clipped.lastIndexOf(' ');
    summary = `${(lastSpace > 0 ? clipped.slice(0, lastSpace) : clipped).replace(/[.,;:]$/, '')}…`;
  }
  return summary;
}

/**
 * Read the summary for a tag from its notes file. Missing or unreadable notes
 * are not fatal: the release itself already fails when the file is absent,
 * and the notice is better with a generic line than not published at all.
 */
export async function readReleaseSummary(tag) {
  try {
    const notes = await readFile(
      new URL(`../docs/releases/${tag}.md`, import.meta.url),
      'utf8',
    );
    return summaryFromNotes(notes);
  } catch {
    return null;
  }
}

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
  summary = DEFAULT_SUMMARY,
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
  // Without this the notice always read "A new PSF Guard release is ready.",
  // so the notes written for each release never reached anyone already
  // running PSF Guard. An explicit argument still wins.
  const resolvedSummary = summary || (await readReleaseSummary(tag)) || undefined;

  const feeds = await buildReleaseFeeds({
    artifactDirectory,
    version,
    tag,
    publishedAt,
    summary: resolvedSummary,
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
