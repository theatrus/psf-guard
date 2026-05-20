import Database from 'better-sqlite3';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

/**
 * Wipe and recreate the per-PID tmp directory used by the e2e suite. Inside
 * it we drop a fresh SQLite fixture file mimicking the N.I.N.A. scheduler
 * schema (matching `tests/integration_sequence_analysis.rs::create_test_schema`)
 * pre-populated with a single project / target / image so the merged
 * Overview has something to show once the DB is registered.
 *
 * The registry file is intentionally NOT created here — webServer launches
 * `psf-guard server` with `--registry <path>` and the server creates it on
 * demand. Specs that need a pre-registered DB do so via the HTTP API.
 */
export default async function globalSetup() {
  const tmpBase =
    process.env.PSF_GUARD_E2E_TMP ??
    path.join(os.tmpdir(), `psf-guard-e2e-${process.pid}`);

  fs.rmSync(tmpBase, { recursive: true, force: true });
  fs.mkdirSync(tmpBase, { recursive: true });
  fs.mkdirSync(path.join(tmpBase, 'cache'), { recursive: true });
  fs.mkdirSync(path.join(tmpBase, 'images'), { recursive: true });

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

    INSERT INTO project (Id, profileId, name, description)
      VALUES (1, 'default', 'Imaging Rig', 'e2e fixture project');
    INSERT INTO target (Id, projectId, name, active, ra, dec)
      VALUES (1, 1, 'M42 e2e', 1, 83.82, -5.39);
    INSERT INTO acquiredimage
      (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata)
      VALUES (1, 1, 1, 1700000000, 'L', 0, '{"FileName": "image_001.fits"}');
  `);
  db.close();

  // Surface the resolved paths to specs.
  process.env.PSF_GUARD_E2E_TMP = tmpBase;
  process.env.PSF_GUARD_E2E_DB = dbPath;
  process.env.PSF_GUARD_E2E_IMAGES = path.join(tmpBase, 'images');
}
