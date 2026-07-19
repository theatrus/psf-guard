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
