import { readFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

function capture(text, pattern, label) {
  const match = text.match(pattern);
  if (!match) throw new Error(`Could not read ${label}.`);
  return match[1];
}

export function checkReleaseHighlights(releases, requiredVersion) {
  if (!Array.isArray(releases) || releases.length === 0) {
    throw new Error('Release highlights must be a nonempty array.');
  }
  const versions = new Set();
  for (const release of releases) {
    if (!release || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(release.version ?? '')) {
      throw new Error('Release highlights need a valid version.');
    }
    if (versions.has(release.version)) throw new Error(`Duplicate release highlights: ${release.version}`);
    versions.add(release.version);
    let count = 0;
    for (const field of ['deprecations', 'upgradeNotes', 'highlights']) {
      if (!Array.isArray(release[field])) throw new Error(`${release.version}: ${field} must be an array.`);
      for (const item of release[field]) {
        if (!item || typeof item.title !== 'string' || !item.title.trim()
          || typeof item.body !== 'string' || !item.body.trim()) {
          throw new Error(`${release.version}: ${field} entries need a title and body.`);
        }
      }
      count += release[field].length;
    }
    if (!count) throw new Error(`${release.version}: release highlights are empty.`);
  }
  if (requiredVersion && !versions.has(requiredVersion)) {
    throw new Error(`Release highlights are missing for ${requiredVersion}.`);
  }
}

export async function checkReleaseVersion(repositoryRoot, expectedTag) {
  const [cargoToml, cargoLock, tauriConfig, rpmSpec] = await Promise.all([
    readFile(path.join(repositoryRoot, 'Cargo.toml'), 'utf8'),
    readFile(path.join(repositoryRoot, 'Cargo.lock'), 'utf8'),
    readFile(path.join(repositoryRoot, 'tauri.conf.json'), 'utf8'),
    readFile(path.join(repositoryRoot, 'packaging/rpm/psf-guard.spec'), 'utf8'),
  ]);

  const packageSection = capture(
    cargoToml,
    /^\[package\]\s*\n([\s\S]*?)(?=\n\[|$)/,
    'the Cargo package section',
  );
  const cargoVersion = capture(packageSection, /^version\s*=\s*"([^"]+)"/m, 'Cargo.toml version');
  const lockPackage = cargoLock
    .split('[[package]]')
    .find((block) => /^name = "psf-guard"$/m.test(block));
  if (!lockPackage) throw new Error('Could not find psf-guard in Cargo.lock.');
  const lockVersion = capture(lockPackage, /^version = "([^"]+)"$/m, 'Cargo.lock version');
  const tauriVersion = JSON.parse(tauriConfig).version;
  const rpmVersion = capture(rpmSpec, /^Version:\s*(\S+)$/m, 'RPM version');

  const versions = new Map([
    ['Cargo.toml', cargoVersion],
    ['Cargo.lock', lockVersion],
    ['tauri.conf.json', tauriVersion],
    ['RPM spec', rpmVersion],
  ]);
  const mismatches = [...versions].filter(([, version]) => version !== cargoVersion);
  if (mismatches.length > 0) {
    const detail = [...versions].map(([file, version]) => `${file}=${version}`).join(', ');
    throw new Error(`Release versions do not match: ${detail}`);
  }

  const tag = `v${cargoVersion}`;
  if (expectedTag && expectedTag !== tag) {
    throw new Error(`Release tag ${expectedTag} does not match ${tag}.`);
  }
  const highlights = JSON.parse(await readFile(
    path.join(repositoryRoot, 'static/src/updates/highlights.json'), 'utf8',
  ));
  checkReleaseHighlights(highlights, expectedTag ? cargoVersion : undefined);
  // A tagged release must ship its own notes. Between releases, main is
  // stamped with the next version while its notes still gather in
  // unreleased.md, so a check without a tag accepts either file.
  const notesPath = path.join(repositoryRoot, 'docs/releases', `${tag}.md`);
  const unreleasedPath = path.join(repositoryRoot, 'docs/releases', 'unreleased.md');
  const candidates = expectedTag ? [notesPath] : [notesPath, unreleasedPath];
  for (const candidate of candidates) {
    const notes = await stat(candidate).catch(() => null);
    if (notes?.isFile() && notes.size > 0) return cargoVersion;
  }
  throw new Error(
    expectedTag
      ? `Release notes are missing or empty: ${notesPath}`
      : `Release notes are missing or empty: ${notesPath} (or ${unreleasedPath} between releases)`,
  );
}

async function main() {
  const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const version = await checkReleaseVersion(repositoryRoot, process.argv[2]);
  process.stdout.write(`${version}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) await main();
