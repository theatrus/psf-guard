import Database from 'better-sqlite3';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { ensureAllFixtures } from './fixtures/loader';

/**
 * Wipe and recreate the per-PID tmp directory used by the e2e suite. Inside
 * it we drop a fresh SQLite fixture file mimicking the N.I.N.A. scheduler
 * schema (matching `tests/integration_sequence_analysis.rs::create_test_schema`)
 * pre-populated with two projects:
 *
 *   - "Project Alpha" / target "Alpha M65" → 3 images (the B-filter
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

  // Resolve / download the real FITS fixtures up front. Each is hashed
  // against the manifest before we trust it.
  const fixturePaths = await ensureAllFixtures();
  const [alpha1, alpha2, alpha3, beta1] = Object.keys(fixturePaths).sort();

  // Copy fixtures into the per-PID images directory so multiple parallel
  // test runs (different PIDs) can't race on the same files.
  for (const name of [alpha1, alpha2, alpha3, beta1]) {
    fs.copyFileSync(fixturePaths[name], path.join(imagesDir, name));
  }

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

    INSERT INTO project (Id, profileId, name, description) VALUES
      (1, 'default', 'Project Alpha', '3-image B-filter sequence'),
      (2, 'default', 'Project Beta',  '1 longer-exposure target');

    INSERT INTO target (Id, projectId, name, active, ra, dec) VALUES
      (1, 1, 'Alpha M65',  1, 169.73,  13.09),
      (2, 2, 'Beta Field', 1, 230.0,  -10.0);
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
  process.env.PSF_GUARD_E2E_ALPHA1 = alpha1;
  process.env.PSF_GUARD_E2E_ALPHA2 = alpha2;
  process.env.PSF_GUARD_E2E_ALPHA3 = alpha3;
  process.env.PSF_GUARD_E2E_BETA1 = beta1;
}
