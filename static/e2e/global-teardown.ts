import * as os from 'os';
import * as path from 'path';
import { removeRunDirectories } from './tmp-dirs';

/**
 * Remove this run's tmp directories.
 *
 * There was no teardown at all, so each run left roughly 1.8 GB of copied
 * FITS fixtures behind under a PID-named directory that nothing ever
 * revisited. Global setup only ever wiped the directory it was about to use.
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

  removeRunDirectories(tmpBase);
}
