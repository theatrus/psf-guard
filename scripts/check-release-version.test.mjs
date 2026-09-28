import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { checkReleaseVersion } from './check-release-version.mjs';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

test('release versions and notes agree', async () => {
  const version = await checkReleaseVersion(repositoryRoot);
  assert.match(version, /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/);
  // On a release branch the version's notes exist and a tag check passes.
  // On main the version is the next release and its notes are still
  // unreleased.md, so a tag check must refuse until they are cut.
  if (existsSync(path.join(repositoryRoot, 'docs/releases', `v${version}.md`))) {
    assert.equal(await checkReleaseVersion(repositoryRoot, `v${version}`), version);
  } else {
    await assert.rejects(checkReleaseVersion(repositoryRoot, `v${version}`), /Release notes are missing/);
  }
});

test('rejects a tag that does not match the package version', async () => {
  await assert.rejects(
    checkReleaseVersion(repositoryRoot, 'v9.9.9'),
    /does not match v/,
  );
});
