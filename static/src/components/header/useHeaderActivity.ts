import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { DatabaseJobs, JobsResponse } from '../../api/types';
import { JOBS_QUERY_KEY } from '../../api/jobs';
import { jobItem, summarize } from './activityItems';

export { JOBS_QUERY_KEY };

/** How often the header reads the job list while a job is listed, and while
 *  none is. A quiet server needs a slower look, for jobs another client or
 *  the server itself starts. */
export const JOBS_POLL_MS = { active: 1_000, idle: 5_000 } as const;

/** How long the chip shows "Done" after the last job finishes. */
export const FINISHED_MS = 2500;

export interface FinishedNote {
  /** A quality scan that just finished reported frame errors. */
  errors: boolean;
  message?: string;
}

/** Frame errors from a quality scan that the header saw finish. The note
 *  stays until a person dismisses it, however long other jobs run. */
export interface ScanErrorNote {
  dbId: string;
  dbName: string;
  message: string;
  /** The sequence of the target the scan was on. */
  href?: string;
}

/** The query keys a finished Target Scheduler sync or import changes. */
const TS_SYNC_KEYS = [
  ['databases'],
  ['db'],
  ['directorActivation'],
  ['directorActivationCheck'],
  ['directorPlanProgress'],
] as const;

function idsWhere(databases: DatabaseJobs[], test: (db: DatabaseJobs) => boolean): string {
  return databases.filter(test).map((db) => db.id).sort().join('|');
}

function split(ids: string): string[] {
  return ids ? ids.split('|') : [];
}

/**
 * Every background job on the server, across databases, from one route:
 * catalog refreshes, quality scans and analyses, calibration jobs, stack
 * builds and processing runs, queued or running, automatic work waiting
 * for a person's work, WBPP runs, transfers, syncs, backups and catalog
 * installs.
 */
export function useHeaderActivity() {
  const queryClient = useQueryClient();
  const jobs = useQuery<JobsResponse>({
    queryKey: JOBS_QUERY_KEY,
    queryFn: apiClient.getJobs,
    refetchInterval: (query) =>
      query.state.data?.jobs.length ? JOBS_POLL_MS.active : JOBS_POLL_MS.idle,
    refetchIntervalInBackground: false,
  });
  const rows = jobs.data?.jobs;
  const databases = useMemo(() => jobs.data?.databases ?? [], [jobs.data]);
  const items = useMemo(() => (rows ?? []).map(jobItem), [rows]);
  const summary = summarize(items);

  // When a catalog installation finishes, the installed catalogs have
  // changed: Settings and the color cards read the capabilities again.
  const installing = items.some((item) => item.kind === 'catalog');
  const wasInstalling = useRef(false);
  useEffect(() => {
    if (wasInstalling.current && !installing) {
      queryClient.invalidateQueries({ queryKey: ['astrometry', 'capabilities'] });
      queryClient.invalidateQueries({
        predicate: (query) => query.queryKey[0] === 'db' && query.queryKey.includes('stack-color'),
      });
    }
    wasInstalling.current = installing;
  }, [installing, queryClient]);

  // When a Target Scheduler sync or import stops running, the catalogs and
  // plans it wrote are read again.
  const syncing = items
    .filter((item) => item.control?.kind === 'ts-sync')
    .map((item) => item.key)
    .sort()
    .join('|');
  const previousSyncing = useRef<string[]>([]);
  useEffect(() => {
    const now = split(syncing);
    const ended = previousSyncing.current.some((key) => !now.includes(key));
    previousSyncing.current = now;
    if (!ended) return;
    for (const queryKey of TS_SYNC_KEYS) void queryClient.invalidateQueries({ queryKey });
  }, [syncing, queryClient]);

  // When a database's work finishes, every view has new images, metrics and
  // grades for it, so all of its queries are read again.
  const busyIds = idsWhere(databases, (db) => db.busy);
  const previousBusy = useRef<string[]>([]);
  useEffect(() => {
    const now = split(busyIds);
    const done = previousBusy.current.filter((id) => !now.includes(id));
    previousBusy.current = now;
    for (const id of done) queryClient.invalidateQueries({ queryKey: ['db', id] });
  }, [busyIds, queryClient]);

  // Frame errors belong to a scan this header saw running. The server keeps
  // the last scan's counts until the next scan starts, so a database that
  // only finished a refresh must not report an earlier scan's errors.
  const scanningIds = idsWhere(databases, (db) => db.scanning);
  const previousScanning = useRef<string[]>([]);
  const [scanErrors, setScanErrors] = useState<ScanErrorNote[]>([]);
  const scanErrorsRef = useRef(scanErrors);
  scanErrorsRef.current = scanErrors;
  useEffect(() => {
    const now = split(scanningIds);
    const ended = previousScanning.current.filter((id) => !now.includes(id));
    previousScanning.current = now;
    let next = scanErrorsRef.current;
    for (const id of ended) {
      const db = databases.find((entry) => entry.id === id);
      const scan = db?.scan;
      if (db && scan && scan.errors > 0) {
        const note: ScanErrorNote = {
          dbId: db.id,
          dbName: db.name,
          message: `${db.name}: ${scan.errors} frame${scan.errors === 1 ? '' : 's'} failed${
            scan.last_error ? ` — ${scan.last_error}` : ''
          }`,
          href:
            scan.project_id != null && scan.target_id != null
              ? `/sequence?${new URLSearchParams({
                  db: db.id,
                  project: String(scan.project_id),
                  target: String(scan.target_id),
                }).toString()}`
              : undefined,
        };
        // A database's newer scan replaces its older note.
        next = [...next.filter((entry) => entry.dbId !== db.id), note];
      }
    }
    if (next !== scanErrorsRef.current) {
      // The finish effect below may run in this same commit, before the
      // state update applies, when the scan was the last job.
      scanErrorsRef.current = next;
      setScanErrors(next);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- read when the scanning set changes, which is when a scan ends.
  }, [scanningIds]);

  const dismissScanError = useCallback((dbId: string) => {
    setScanErrors((current) => current.filter((entry) => entry.dbId !== dbId));
  }, []);

  // When the last job finishes, the chip shows "Done" for a moment and
  // then hides. While scan error notes remain, it shows "Finished with
  // errors" until each note is dismissed.
  const [finished, setFinished] = useState<FinishedNote | null>(null);
  const finishedTimer = useRef<number | null>(null);
  const workCount = items.length;
  const previousCount = useRef(workCount);
  useEffect(() => {
    const before = previousCount.current;
    previousCount.current = workCount;
    if (workCount > 0) {
      if (finishedTimer.current != null) {
        window.clearTimeout(finishedTimer.current);
        finishedTimer.current = null;
      }
      setFinished(null);
      return;
    }
    if (before === 0) return;
    if (scanErrorsRef.current.length > 0) {
      setFinished({ errors: true });
      return;
    }
    setFinished({ errors: false });
    finishedTimer.current = window.setTimeout(() => {
      setFinished(null);
      finishedTimer.current = null;
    }, FINISHED_MS);
  }, [workCount]);
  // When the last error note is dismissed, the chip has nothing to show.
  useEffect(() => {
    if (scanErrors.length === 0) setFinished((current) => (current?.errors ? null : current));
  }, [scanErrors.length]);
  useEffect(() => () => {
    if (finishedTimer.current != null) window.clearTimeout(finishedTimer.current);
  }, []);

  return { items, summary, finished, scanErrors, dismissScanError };
}
