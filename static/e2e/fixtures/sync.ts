import Database from 'better-sqlite3';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const TS_SCHEMA_DIR = path.resolve(__dirname, '../../../src/ts_schema');

/**
 * Bearer keys the running server accepts for `/api/sync/v1/*`. A key is scoped
 * to one catalog, so a real telescope-to-review sync needs both: the exporting
 * side presents one, the receiving side the other. Long enough to clear the
 * registry's minimum, and fixed so specs can present them.
 */
export const TELESCOPE_TOKEN = 'e2e-remote-sync-telescope-token-01';
export const REVIEW_TOKEN = 'e2e-remote-sync-review-token-0001';

/** Registry slugs of the two catalogs the sync specs move data between. */
export const TELESCOPE_DB = 'e2e-telescope';
export const REVIEW_DB = 'e2e-review';

/**
 * Build a real Target Scheduler database from the SQL PSF Guard vendors.
 *
 * Hand-writing a cut-down schema here was tried and is a trap: the sync
 * engine, the importer, and the upload path each read columns the shorthand
 * did not have, and each failure looked like a product bug. Replaying the
 * vendored initial schema and migrations keeps one source of truth, so these
 * catalogs move with `src/ts_schema` instead of drifting from it.
 */
function applyRealSchema(db: InstanceType<typeof Database>): void {
  db.exec(fs.readFileSync(path.join(TS_SCHEMA_DIR, 'initial_schema.sql'), 'utf8'));
  const migrations = fs
    .readdirSync(path.join(TS_SCHEMA_DIR, 'migrate'))
    .filter((name) => name.endsWith('.sql'))
    .map((name) => Number(name.replace('.sql', '')))
    .sort((left, right) => left - right);
  for (const version of migrations) {
    db.exec(
      fs.readFileSync(path.join(TS_SCHEMA_DIR, 'migrate', `${version}.sql`), 'utf8')
    );
    db.pragma(`user_version = ${version}`);
  }
}

/** A night's work at the telescope, none of it reviewed yet. */
const TELESCOPE_ROWS = `
  INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid)
    VALUES (1,'default','Sync Nebula','fresh from the mount',1,5,0,0,'sync-project');
  INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid)
    VALUES (1,'Sync Nebula Core',1,5.5,-5.4,0,1,'sync-target');
  INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid)
    VALUES (1,'default','Ha 300','Ha',100,30,1,0,'sync-template');
  INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid)
    VALUES (1,'default',300,40,2,2,1,1,1,'sync-plan');
  INSERT INTO ruleweight (Id,name,weight,projectid) VALUES (1,'Priority',2.5,1);
  INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid)
    VALUES (1,1,1,1750000000,'Ha',1,'{"FileName":"sync-one.fits"}','default',1,'sync-image-one');
  INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid)
    VALUES (2,1,1,1750000600,'Ha',1,'{"FileName":"sync-two.fits"}','default',1,'sync-image-two');
`;

/**
 * The review copy already holds the first frame and has rejected it. A merge
 * must respect that; a grade push must be able to send it back.
 */
const REVIEW_ROWS = `
  INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid)
    VALUES (10,'default','Sync Nebula','under review',1,5,0,0,'sync-project');
  INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid)
    VALUES (10,'Sync Nebula Core',1,5.5,-5.4,0,10,'sync-target');
  INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid)
    VALUES (10,'default','Ha 300','Ha',100,30,1,0,'sync-template');
  INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid)
    VALUES (10,'default',300,40,1,1,10,10,1,'sync-plan');
  INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,exposureId,guid)
    VALUES (10,10,10,1750000000,'Ha',2,'{"FileName":"sync-one.fits"}','reviewed here','default',10,'sync-image-one');
`;

const FITS_CARD_BYTES = 80;
const FITS_BLOCK_BYTES = 2880;

/**
 * A small but real FITS light frame. The upload endpoint reads headers to
 * place the frame, so a stub of zeroes would not do; this carries the cards
 * the importer looks at and a 10x10 16-bit image.
 */
export function fitsLight(object: string, dateObs: string): Buffer {
  const cards = [
    'SIMPLE  =                    T',
    'BITPIX  =                   16',
    'NAXIS   =                    2',
    'NAXIS1  =                   10',
    'NAXIS2  =                   10',
    "IMAGETYP= 'LIGHT   '",
    `OBJECT  = '${object}'`,
    "FILTER  = 'Ha      '",
    `DATE-OBS= '${dateObs}'`,
    'EXPTIME =                300.0',
    'GAIN    =                  100',
    'OFFSET  =                   30',
    'END',
  ];
  const headerBlocks = Math.ceil(
    (cards.length * FITS_CARD_BYTES) / FITS_BLOCK_BYTES
  );
  const header = Buffer.alloc(headerBlocks * FITS_BLOCK_BYTES, 0x20);
  cards.forEach((card, index) =>
    header.write(card.padEnd(FITS_CARD_BYTES, ' '), index * FITS_CARD_BYTES, 'ascii')
  );
  // 10x10 big-endian 16-bit pixels, one block, zero-padded.
  const data = Buffer.alloc(FITS_BLOCK_BYTES);
  for (let pixel = 0; pixel < 100; pixel += 1) {
    data.writeInt16BE(100 + pixel, pixel * 2);
  }
  return Buffer.concat([header, data]);
}

/** Everything one of the two sync instances is started with. */
export interface SyncInstance {
  configPath: string;
  registryPath: string;
  cacheDir: string;
  databasePath: string;
}

export interface SyncFixture {
  /** Directory holding both catalogs and both server configs. */
  directory: string;
  telescope: SyncInstance;
  review: SyncInstance;
  /** Where the review instance writes frames it receives. */
  uploadDir: string;
}

/**
 * Install the fixture two PSF Guard instances need to sync with each other:
 * one scheduler catalog each, and one config file each opening it for remote
 * access.
 *
 * These get their own instances rather than sharing the main e2e server on
 * purpose. Specs that exercise the database CRUD UI reset that server's
 * database list to empty, which would take the sync catalogs with it — and a
 * re-added entry would come back without the grant its config file gave it,
 * since that is applied at startup.
 *
 * Everything lives beside the run directory rather than inside it, because
 * global setup wipes the run directory while these servers are already up and
 * still opening their catalogs by path for every sync. Playwright config
 * calls this before any server starts; a second call is a no-op.
 */
export function installSyncFixture(tmpBase: string): SyncFixture {
  const directory = `${tmpBase}-sync`;
  const uploadDir = path.join(directory, 'incoming');
  const instance = (name: string): SyncInstance => ({
    configPath: path.join(directory, `${name}.toml`),
    registryPath: path.join(directory, `${name}-registry.json`),
    cacheDir: path.join(directory, `${name}-cache`),
    databasePath: path.join(directory, `${name}.sqlite`),
  });
  const telescope = instance('telescope');
  const review = instance('review');
  if (fs.existsSync(telescope.databasePath)) {
    return { directory, telescope, review, uploadDir };
  }

  fs.rmSync(directory, { recursive: true, force: true });
  fs.mkdirSync(directory, { recursive: true });
  // The review instance's config also creates this at startup, but the
  // telescope has no upload grant and its registry still has to name a
  // directory that exists.
  fs.mkdirSync(uploadDir, { recursive: true });
  for (const [target, rows] of [
    [telescope, TELESCOPE_ROWS],
    [review, REVIEW_ROWS],
  ] as const) {
    const db = new Database(target.databasePath);
    applyRealSchema(db);
    db.exec(rows);
    db.close();
  }

  const writeRegistry = (
    target: SyncInstance,
    id: string,
    name: string,
    imageDir: string
  ) =>
    fs.writeFileSync(
      target.registryPath,
      `${JSON.stringify(
        {
          schema_version: 2,
          databases: [
            { id, name, db_path: target.databasePath, image_dirs: [imageDir] },
          ],
        },
        null,
        2
      )}\n`
    );
  writeRegistry(telescope, TELESCOPE_DB, 'E2E telescope', directory);
  writeRegistry(review, REVIEW_DB, 'E2E review copy', uploadDir);

  // A headless server has no Settings panel, so config blocks are how an
  // operator opens a catalog for remote access. One config per instance,
  // exactly as two machines would have.
  const reviewTokenFile = path.join(directory, 'review.token');
  fs.writeFileSync(reviewTokenFile, `${REVIEW_TOKEN}\n`);
  fs.writeFileSync(
    telescope.configPath,
    [
      '[server]',
      '',
      '[cache]',
      '',
      '[[remote_sync]]',
      `database = "${TELESCOPE_DB}"`,
      `token = "${TELESCOPE_TOKEN}"`,
      '',
    ].join('\n')
  );
  fs.writeFileSync(
    review.configPath,
    [
      '[server]',
      '',
      '[cache]',
      '',
      '[[remote_sync]]',
      `database = "${REVIEW_DB}"`,
      // token_file is the form a deployment actually uses: systemd
      // credentials, Docker secrets, and the like.
      `token_file = "${reviewTokenFile}"`,
      '',
      // The same key also lets the telescope ship frames here. Catalog rows
      // and pixels travel by different routes and are separate grants.
      '[[remote_upload]]',
      `database = "${REVIEW_DB}"`,
      `image_dir = "${uploadDir}"`,
      `token_file = "${reviewTokenFile}"`,
      '',
    ].join('\n')
  );

  return { directory, telescope, review, uploadDir };
}
