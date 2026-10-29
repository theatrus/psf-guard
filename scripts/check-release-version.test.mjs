import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { checkReleaseVersion, checkReleaseHighlights } from './check-release-version.mjs';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

test('release versions and notes agree', async () => {
  const version = await checkReleaseVersion(repositoryRoot);
  assert.match(version, /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/);
  // On a release branch the version's notes exist and a tag check passes.
  // On main the version is the next release and its notes are still
  // unreleased.md, so a tag check must refuse until they are cut. It may
  // refuse for the missing highlights first: an interim version has neither.
  if (existsSync(path.join(repositoryRoot, 'docs/releases', `v${version}.md`))) {
    assert.equal(await checkReleaseVersion(repositoryRoot, `v${version}`), version);
  } else {
    await assert.rejects(
      checkReleaseVersion(repositoryRoot, `v${version}`),
      /Release (notes|highlights) are missing/,
    );
  }
});

test('rejects a tag that does not match the package version', async () => {
  await assert.rejects(
    checkReleaseVersion(repositoryRoot, 'v9.9.9'),
    /does not match v/,
  );
});

const highlight = { title: 'Changed behavior', body: 'What to do next.' };
const release = { version: '1.2.0', deprecations: [], upgradeNotes: [], highlights: [highlight] };

test('tagged releases need highlights for their own version', () => {
  assert.doesNotThrow(() => checkReleaseHighlights([release], '1.2.0'));
  assert.throws(() => checkReleaseHighlights([release], '1.3.0'), /missing for 1.3.0/);
  assert.doesNotThrow(() => checkReleaseHighlights([release]));
});

test('every release explicitly lists deprecations and uses complete entries', () => {
  assert.throws(() => checkReleaseHighlights([{ ...release, deprecations: undefined }]), /deprecations must be an array/);
  assert.throws(() => checkReleaseHighlights([{ ...release, deprecations: [{ title: 'Removed' }] }]), /title and body/);
  assert.throws(() => checkReleaseHighlights([{ ...release, highlights: [] }]), /empty/);
  assert.throws(() => checkReleaseHighlights([release, release]), /Duplicate/);
  assert.throws(() => checkReleaseHighlights([{ ...release, version: 'bad' }]), /valid version/);
  assert.doesNotThrow(() => checkReleaseHighlights([{ ...release, highlights: [], deprecations: [highlight] }]));
});
