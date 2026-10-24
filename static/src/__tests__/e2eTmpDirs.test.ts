import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  removeRunDirectories,
  removeRunDirectoriesQuietly,
  runDirectories,
  sweepAbandonedRunDirectories,
} from '../../e2e/tmp-dirs';

/**
 * The e2e run directories hold ~1.8 GB of FITS fixtures each. Nothing removed
 * them, so a tmpfs /tmp filled up and runs began dying mid-copy. These cover
 * the rule that matters: a directory is reclaimed only once its owning
 * process is gone.
 */

const created: string[] = [];

function makeRunDir(pid: number, suffix = ''): string {
  const directory = path.join(os.tmpdir(), `psf-guard-e2e-${pid}${suffix}`);
  fs.mkdirSync(path.join(directory, 'images'), { recursive: true });
  fs.writeFileSync(path.join(directory, 'images', 'frame.fits'), 'x');
  created.push(directory);
  return directory;
}

afterEach(() => {
  vi.restoreAllMocks();
  while (created.length > 0) {
    fs.rmSync(created.pop() as string, { recursive: true, force: true });
  }
});

describe('e2e tmp directories', () => {
  it('reclaims a directory whose process has exited', () => {
    // A PID that cannot be running: process.kill reports ESRCH for it.
    const dead = makeRunDir(999_000_001);
    const deadSync = makeRunDir(999_000_001, '-sync');

    const removed = sweepAbandonedRunDirectories(
      path.join(os.tmpdir(), 'psf-guard-e2e-999000002')
    );

    expect(fs.existsSync(dead)).toBe(false);
    expect(fs.existsSync(deadSync)).toBe(false);
    expect(removed).toEqual(expect.arrayContaining([dead, deadSync]));
  });

  it('leaves a running suite alone', () => {
    // This test process is unquestionably alive.
    const live = makeRunDir(process.pid);

    sweepAbandonedRunDirectories(
      path.join(os.tmpdir(), 'psf-guard-e2e-999000003')
    );

    expect(fs.existsSync(live)).toBe(true);
  });

  it('never removes the directory the current run is about to use', () => {
    // Belt and braces: even if the PID looks dead, the run that is starting
    // owns these two and global setup is about to populate them.
    const current = makeRunDir(999_000_004);
    const currentSync = makeRunDir(999_000_004, '-sync');

    const removed = sweepAbandonedRunDirectories(current);

    expect(fs.existsSync(current)).toBe(true);
    expect(fs.existsSync(currentSync)).toBe(true);
    expect(removed).not.toContain(current);
  });

  it('ignores directories that are not run directories', () => {
    const stray = path.join(os.tmpdir(), 'psf-guard-e2e-fixtures');
    fs.mkdirSync(stray, { recursive: true });
    created.push(stray);

    sweepAbandonedRunDirectories(
      path.join(os.tmpdir(), 'psf-guard-e2e-999000005')
    );

    // The shared fixture cache has no PID and must survive; re-downloading
    // it costs ~470 MB.
    expect(fs.existsSync(stray)).toBe(true);
  });

  it('treats a process it cannot signal as alive', () => {
    const foreign = makeRunDir(999_000_006);
    vi.spyOn(process, 'kill').mockImplementation(() => {
      const error = new Error('operation not permitted') as NodeJS.ErrnoException;
      error.code = 'EPERM';
      throw error;
    });

    sweepAbandonedRunDirectories(
      path.join(os.tmpdir(), 'psf-guard-e2e-999000007')
    );

    expect(fs.existsSync(foreign)).toBe(true);
  });

  it('removes both directories a run owns', () => {
    const base = makeRunDir(999_000_008);
    const sync = makeRunDir(999_000_008, '-sync');

    expect(runDirectories(base)).toEqual([base, sync]);
    removeRunDirectories(base);

    expect(fs.existsSync(base)).toBe(false);
    expect(fs.existsSync(sync)).toBe(false);
  });

  it('teardown reports a directory it cannot remove instead of failing the run', () => {
    const base = makeRunDir(999_000_009);
    makeRunDir(999_000_009, '-sync');
    const busy = `ENOTEMPTY, Directory not empty: '${base}'`;
    const removed: string[] = [];

    const left = removeRunDirectoriesQuietly(base, (directory) => {
      if (directory === base) throw new Error(busy);
      fs.rmSync(directory, { recursive: true, force: true });
      removed.push(directory);
    });

    // The busy directory is reported, and its sibling is still removed.
    expect(left).toEqual([`${base}: ${busy}`]);
    expect(removed).toEqual([`${base}-sync`]);
    expect(fs.existsSync(`${base}-sync`)).toBe(false);
  });

  it('teardown removes both directories when nothing is busy', () => {
    const base = makeRunDir(999_000_010);
    makeRunDir(999_000_010, '-sync');
    expect(removeRunDirectoriesQuietly(base)).toEqual([]);
    expect(fs.existsSync(base)).toBe(false);
    expect(fs.existsSync(`${base}-sync`)).toBe(false);
  });
});
