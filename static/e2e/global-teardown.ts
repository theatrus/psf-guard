import * as os from 'os';
import * as path from 'path';
import { removeRunDirectoriesQuietly } from './tmp-dirs';

/**
 * Remove this run's tmp directories.
 *
 * There was no teardown at all, so each run left roughly 1.8 GB of copied
 * FITS fixtures behind under a PID-named directory that nothing ever
 * revisited. Global setup only ever wiped the directory it was about to use.
 *
 * The server under test can still be writing into them as teardown runs, so
 * a removal that fails is reported and left for a later run's sweep rather
 * than failing a run whose tests all finished.
 *
 * Keep the directories when PSF_GUARD_E2E_KEEP_TMP is set — a failing run is
 * much easier to diagnose with its catalogs and caches still on disk. The
 * sweep in global setup collects them on a later run either way.
 */
export default async function globalTeardown() {
  if (process.env.PSF_GUARD_E2E_KEEP_TMP) {
    return;
  }

  const tmpBase =
    process.env.PSF_GUARD_E2E_TMP ??
    path.join(os.tmpdir(), `psf-guard-e2e-${process.pid}`);

  for (const left of removeRunDirectoriesQuietly(tmpBase)) {
    console.warn(`e2e teardown left a directory behind; a later run removes it. ${left}`);
  }
}
