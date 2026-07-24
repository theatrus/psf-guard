import Database from 'better-sqlite3';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { installAstrometryFixture } from './fixtures/astrometry';
import { ensureAllFixtures } from './fixtures/loader';

const FITS_CARD_BYTES = 80;
const FITS_BLOCK_BYTES = 2880;

function fitsCard(contents: string): Buffer {
  const encoded = Buffer.from(contents, 'ascii');
  if (encoded.length > FITS_CARD_BYTES) {
    throw new Error(`FITS fixture card exceeds 80 bytes: ${contents}`);
  }
  return Buffer.from(contents.padEnd(FITS_CARD_BYTES, ' '), 'ascii');
}

/**
 * Add a small, standards-based linear ICRS TAN WCS to one copied fixture.
 * The release fixture has ample padding after END in its second header block,
 * so this edits header cards only and leaves the real pixel payload unchanged.
 */
function installEmbeddedWcs(filePath: string): void {
  const fd = fs.openSync(filePath, 'r+');
  try {
    const header = Buffer.alloc(FITS_BLOCK_BYTES * 4);
    const bytesRead = fs.readSync(fd, header, 0, header.length, 0);
    let endOffset = -1;
    for (let offset = 0; offset + FITS_CARD_BYTES <= bytesRead; offset += FITS_CARD_BYTES) {
      if (header.toString('ascii', offset, offset + 8).trim() === 'END') {
        endOffset = offset;
        break;
      }
    }
    if (endOffset < 0) {
      throw new Error(`FITS fixture has no END card: ${filePath}`);
    }

    const cards = [
      "CTYPE1  = 'RA---TAN'           / Undistorted tangent-plane RA axis",
      "CTYPE2  = 'DEC--TAN'           / Undistorted tangent-plane Dec axis",
      "CUNIT1  = 'deg'                / World-coordinate unit",
      "CUNIT2  = 'deg'                / World-coordinate unit",
      "RADESYS = 'ICRS'               / Celestial reference frame",
      'EQUINOX =               2000.0 / Reference equinox',
      'CRVAL1  =     130.107013851174 / [deg] WCS reference RA',
      'CRVAL2  =     19.6601508517091 / [deg] WCS reference Dec',
      'CRPIX1  =               4788.5 / FITS one-based reference pixel',
      'CRPIX2  =               3194.5 / FITS one-based reference pixel',
      'CD1_1   =  -0.0004160277777778 / [deg/pixel] Linear WCS matrix',
      'CD1_2   =                  0.0 / [deg/pixel] Linear WCS matrix',
      'CD2_1   =                  0.0 / [deg/pixel] Linear WCS matrix',
      'CD2_2   =   0.0004160277777778 / [deg/pixel] Linear WCS matrix',
      'END',
    ].map(fitsCard);
    const blockEnd = Math.ceil((endOffset + FITS_CARD_BYTES) / FITS_BLOCK_BYTES) * FITS_BLOCK_BYTES;
    if (endOffset + cards.length * FITS_CARD_BYTES > blockEnd) {
      throw new Error(`FITS fixture lacks header padding for embedded WCS: ${filePath}`);
    }
    cards.forEach((card, index) => card.copy(header, endOffset + index * FITS_CARD_BYTES));
    header.fill(0x20, endOffset + cards.length * FITS_CARD_BYTES, blockEnd);
    fs.writeSync(fd, header, 0, blockEnd, 0);
  } finally {
    fs.closeSync(fd);
  }
}

/**
 * Wipe and recreate the per-PID tmp directory used by the e2e suite. Inside
 * it we drop a fresh SQLite fixture file mimicking the N.I.N.A. scheduler
 * schema (matching `tests/integration_sequence_analysis.rs::create_test_schema`)
 * pre-populated with two projects:
 *
 *   - "Project Alpha" / target "Alpha M44" → 3 images (the B-filter
 *     sequence 0028/0029/0030, all on the same night).
 *   - "Project Beta"  / target "Beta Field" → 1 image (the later 0104).
 *
 * Real FITS files for each row are pulled from the manifest under
 * `fixtures/manifest.json` (downloaded on demand into the user's cache
 * directory; ~117 MB each, so they live outside the repo). If a download
 * fails or no manifest entry is reachable, specs that rely on preview
 * rendering will fail loudly.
 */
export default async function globalSetup() {
  const tmpBase =
    process.env.PSF_GUARD_E2E_TMP ??
    path.join(os.tmpdir(), `psf-guard-e2e-${process.pid}`);

  fs.rmSync(tmpBase, { recursive: true, force: true });
  fs.mkdirSync(tmpBase, { recursive: true });
  fs.mkdirSync(path.join(tmpBase, 'cache'), { recursive: true });
  const imagesDir = path.join(tmpBase, 'images');
  fs.mkdirSync(imagesDir, { recursive: true });

  // Install a tiny real SEIZAOB4 catalog for the astrometry contract specs.
  // The indexed file is mostly empty buckets, so its checked-in gzip/base64
  // representation stays small while exercising Seiza's actual mapped
  // catalog reader instead of mocking the capability response.
  const objectsPath = installAstrometryFixture(tmpBase);

  // Resolve / download the real FITS fixtures up front. Each is hashed
  // against the manifest before we trust it.
  const fixturePaths = await ensureAllFixtures();
  const [alpha1, alpha2, alpha3, beta1] = Object.keys(fixturePaths).sort();

  // Copy fixtures into the per-PID images directory so multiple parallel
  // test runs (different PIDs) can't race on the same files.
  for (const name of [alpha1, alpha2, alpha3, beta1]) {
    fs.copyFileSync(fixturePaths[name], path.join(imagesDir, name));
  }
  installEmbeddedWcs(path.join(imagesDir, alpha1));

  const dbPath = path.join(tmpBase, 'scheduler.sqlite');
  const db = new Database(dbPath);
  db.exec(`
    CREATE TABLE project (
      Id INTEGER PRIMARY KEY,
      profileId TEXT,
      name TEXT NOT NULL,
      description TEXT
    );
    CREATE TABLE target (
      Id INTEGER PRIMARY KEY,
      projectId INTEGER NOT NULL,
      name TEXT NOT NULL,
      active INTEGER NOT NULL DEFAULT 1,
      ra REAL,
      dec REAL
    );
    CREATE TABLE acquiredimage (
      Id INTEGER PRIMARY KEY,
      projectId INTEGER NOT NULL,
      targetId INTEGER NOT NULL,
      acquireddate INTEGER,
      filtername TEXT NOT NULL,
      gradingStatus INTEGER NOT NULL DEFAULT 0,
      metadata TEXT NOT NULL DEFAULT '{}',
      rejectreason TEXT,
      profileId TEXT
    );
    CREATE TABLE exposuretemplate (
      Id INTEGER PRIMARY KEY,
      filtername TEXT NOT NULL
    );
    CREATE TABLE exposureplan (
      Id INTEGER PRIMARY KEY,
      targetid INTEGER NOT NULL,
      exposureTemplateId INTEGER NOT NULL,
      desired INTEGER NOT NULL DEFAULT 0,
      acquired INTEGER NOT NULL DEFAULT 0,
      accepted INTEGER NOT NULL DEFAULT 0
    );

    INSERT INTO project (Id, profileId, name, description) VALUES
      (1, 'default', 'Project Alpha', '3-image B-filter sequence'),
      (2, 'default', 'Project Beta',  '1 longer-exposure target');

    INSERT INTO target (Id, projectId, name, active, ra, dec) VALUES
      (1, 1, 'Alpha M44',  1, 8.6738009234116,  19.6601508517091),
      (2, 2, 'Beta Field', 1, 15.3333333333333, -10.0);
  `);

  // Use the filenames' embedded ISO date prefix for acquireddate so the
  // server's date-aware path-resolution heuristics line up. The exact
  // values are arbitrary — what matters is consistent ordering within
  // each target sequence.
  const insertImg = db.prepare(
    `INSERT INTO acquiredimage
       (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata)
       VALUES (?, ?, ?, ?, ?, ?, ?)`
  );
  const baseSeqTs = Math.floor(Date.UTC(2026, 3, 16, 22, 25, 0) / 1000); // 2026-04-16 22:25 UTC
  const betaTs = Math.floor(Date.UTC(2026, 3, 17, 0, 6, 0) / 1000);

  insertImg.run(1, 1, 1, baseSeqTs + 0, 'B', 0, JSON.stringify({ FileName: alpha1 }));
  insertImg.run(2, 1, 1, baseSeqTs + 66, 'B', 1, JSON.stringify({ FileName: alpha2 }));
  insertImg.run(3, 1, 1, baseSeqTs + 132, 'B', 0, JSON.stringify({ FileName: alpha3 }));
  insertImg.run(4, 2, 2, betaTs, 'B', 0, JSON.stringify({ FileName: beta1 }));

  db.close();

  // Surface the resolved paths to specs.
  process.env.PSF_GUARD_E2E_TMP = tmpBase;
  process.env.PSF_GUARD_E2E_DB = dbPath;
  process.env.PSF_GUARD_E2E_IMAGES = imagesDir;
  process.env.PSF_GUARD_E2E_OBJECTS = objectsPath;
  process.env.PSF_GUARD_E2E_ALPHA1 = alpha1;
  process.env.PSF_GUARD_E2E_ALPHA2 = alpha2;
  process.env.PSF_GUARD_E2E_ALPHA3 = alpha3;
  process.env.PSF_GUARD_E2E_BETA1 = beta1;
}
