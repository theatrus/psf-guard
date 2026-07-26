import { readFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

function capture(text, pattern, label) {
  const match = text.match(pattern);
  if (!match) throw new Error(`Could not read ${label}.`);
  return match[1];
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
  const notesPath = path.join(repositoryRoot, 'docs/releases', `${tag}.md`);
  const notes = await stat(notesPath);
  if (!notes.isFile() || notes.size === 0) {
    throw new Error(`Release notes are missing or empty: ${notesPath}`);
  }
  return cargoVersion;
}

async function main() {
  const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const version = await checkReleaseVersion(repositoryRoot, process.argv[2]);
  process.stdout.write(`${version}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) await main();
