import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { checkReleaseVersion } from './check-release-version.mjs';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

test('release versions and notes agree', async () => {
  const version = await checkReleaseVersion(repositoryRoot);
  assert.match(version, /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/);
  assert.equal(await checkReleaseVersion(repositoryRoot, `v${version}`), version);
});

test('rejects a tag that does not match the package version', async () => {
  await assert.rejects(
    checkReleaseVersion(repositoryRoot, 'v9.9.9'),
    /does not match v/,
  );
});
