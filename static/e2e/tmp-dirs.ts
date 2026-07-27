import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

/**
 * Lifecycle for the per-run tmp directories.
 *
 * A run works in `<tmpdir>/psf-guard-e2e-<pid>` plus a `-sync` sibling, and
 * they are not small: the FITS fixtures copied into each one come to roughly
 * 1.8 GB. Nothing removed them. Global setup wipes only the directory the
 * current run is about to use, and the name carries a PID, so every previous
 * run's directory survived. On a developer machine with a tmpfs `/tmp` that
 * ends in a full disk, and the failure is confusing:
 *
 *   Error: Unknown system error -122 ... copyfile '.../0030.fits'
 *
 * Teardown handles a run that finishes. The sweep handles one that does not —
 * interrupt Playwright and teardown never runs, which is how the directories
 * accumulated in the first place.
 */

const PREFIX = 'psf-guard-e2e-';
/** `psf-guard-e2e-1234` and its `psf-guard-e2e-1234-sync` sibling. */
const RUN_DIR = /^psf-guard-e2e-(\d+)(-sync)?$/;

/** Both directories a run owns, given its base. */
export function runDirectories(tmpBase: string): string[] {
  return [tmpBase, `${tmpBase}-sync`];
}

/** Remove this run's directories. Safe to call when they do not exist. */
export function removeRunDirectories(tmpBase: string): void {
  for (const directory of runDirectories(tmpBase)) {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

function processIsAlive(pid: number): boolean {
  try {
    // Signal 0 performs the permission and existence checks without
    // delivering anything.
    process.kill(pid, 0);
    return true;
  } catch (error) {
    // EPERM means the process exists but belongs to somebody else — alive as
    // far as we are concerned. Only ESRCH proves it is gone.
    return (error as NodeJS.ErrnoException).code === 'EPERM';
  }
}

/**
 * Delete run directories whose owning process has exited.
 *
 * A PID can be reused, in which case a dead run's directory looks live and is
 * left alone. That is the harmless direction: the next sweep gets it. Deleting
 * a directory out from under a running Playwright process is the one thing
 * this must never do, which is why liveness decides rather than age.
 *
 * Returns the paths removed, so setup can say what it reclaimed.
 */
export function sweepAbandonedRunDirectories(currentTmpBase: string): string[] {
  const root = os.tmpdir();
  const keep = new Set(runDirectories(path.resolve(currentTmpBase)));
  const removed: string[] = [];

  let entries: fs.Dirent[];
  try {
    entries = fs.readdirSync(root, { withFileTypes: true });
  } catch {
    return removed;
  }

  for (const entry of entries) {
    if (!entry.isDirectory() || !entry.name.startsWith(PREFIX)) continue;
    const match = RUN_DIR.exec(entry.name);
    if (!match) continue;

    const directory = path.join(root, entry.name);
    if (keep.has(path.resolve(directory))) continue;
    if (processIsAlive(Number(match[1]))) continue;

    try {
      fs.rmSync(directory, { recursive: true, force: true });
      removed.push(directory);
    } catch {
      // Another run may have removed it, or it may belong to a different
      // user. Neither is worth failing the suite over.
    }
  }

  return removed;
}
