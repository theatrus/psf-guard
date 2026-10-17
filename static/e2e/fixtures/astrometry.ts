import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import * as zlib from 'zlib';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/**
 * Install the tiny real SEIZAOB4 fixture and an isolated partial-bundle
 * registry. Playwright config calls this before the web server starts; global
 * setup calls it again after resetting the remainder of the per-run fixture.
 */
export function installAstrometryFixture(tmpBase: string): string {
  const astrometryDir = path.join(tmpBase, 'astrometry');
  fs.mkdirSync(astrometryDir, { recursive: true });
  const encodedCatalog = fs
    .readFileSync(path.join(__dirname, 'objects.bin.gz.b64'), 'utf8')
    .trim();
  const objectsPath = path.join(astrometryDir, 'objects.bin');
  fs.writeFileSync(
    objectsPath,
    zlib.gunzipSync(Buffer.from(encodedCatalog, 'base64'))
  );
  const encodedStars = fs
    .readFileSync(path.join(__dirname, 'stars.bin.gz.b64'), 'utf8')
    .trim();
  fs.writeFileSync(
    path.join(astrometryDir, 'stars-lite-tycho2.bin'),
    zlib.gunzipSync(Buffer.from(encodedStars, 'base64'))
  );

  fs.writeFileSync(
    path.join(tmpBase, 'registry.json'),
    `${JSON.stringify(
      {
        schema_version: 2,
        databases: [],
        astrometry: { data_dir: astrometryDir },
      },
      null,
      2
    )}\n`
  );
  return objectsPath;
}

/** The version `Cargo.lock` pins for a crate, as `build.rs` reports it. */
export function lockedCrateVersion(name: string): string {
  const lock = fs.readFileSync(path.join(__dirname, '../../../Cargo.lock'), 'utf8');
  const match = lock.match(new RegExp(`\\[\\[package\\]\\]\\r?\\nname = "${name}"\\r?\\nversion = "([^"]+)"`));
  if (!match) throw new Error(`${name} is not in Cargo.lock`);
  return match[1];
}
