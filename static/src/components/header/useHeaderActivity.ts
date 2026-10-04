import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type {
  CacheRefreshProgress,
  QualityBackfillStatus,
  SpatialScanStatus,
  WbppActivity,
} from '../../api/types';
import { useAllDatabases, useMergedTargets } from '../../hooks/useDatabases';
import { useStackActivity } from '../../hooks/useStackActivity';
import { activityItems, summarize, type DatabaseActivity, type ProjectOf } from './activityItems';

export const WBPP_ACTIVITY_QUERY_KEY = ['wbpp-activity'] as const;

/** How long the chip says "Done" after the last job finishes. */
export const FINISHED_MS = 2500;

export interface FinishedNote {
  /** A quality scan that just finished reported frame errors. */
  errors: boolean;
  message?: string;
}

/** Frame errors from a quality scan the header saw finish. It stays until
 *  dismissed, however long the rest of the queue runs. */
export interface ScanErrorNote {
  dbId: string;
  dbName: string;
  message: string;
  /** The sequence of the target the scan was on. */
  href?: string;
}

function busy(db: DatabaseActivity): boolean {
  return !!(db.refresh?.is_refreshing || db.scan?.progress.running || db.backfill?.progress.running);
}

/**
 * Every background job on the server, across databases: catalog refreshes,
 * quality scans and backfills, and stack builds, queued or running. The
 * queries share their keys with the views that start the work, so starting
 * a job anywhere shows here at once.
 */
export function useHeaderActivity() {
  const { data: databases = [] } = useAllDatabases();
  // Shares its cache with the header's target picker.
  const { data: targets } = useMergedTargets();
  const projectOf = useMemo<ProjectOf>(() => {
    const projects = new Map(
      (targets ?? []).map((target) => [`${target.db_id}:${target.id}`, target.project_id])
    );
    return (dbId, targetId) => projects.get(`${dbId}:${targetId}`);
  }, [targets]);
  const queryClient = useQueryClient();
  const { active: stacks } = useStackActivity();
  const wbpp = useQuery<WbppActivity>({
    queryKey: WBPP_ACTIVITY_QUERY_KEY,
    queryFn: apiClient.getWbppActivity,
    // WBPP runs for many minutes; a few seconds' lag is fine, and a quiet
    // server only needs a slow look for runs another client started.
    refetchInterval: (query) =>
      query.state.data && (query.state.data.running.length || query.state.data.queued.length)
        ? 2000
        : 10_000,
    refetchIntervalInBackground: false,
  });

  const refreshes = useQueries({
    queries: databases.map((db) => ({
      queryKey: ['db', db.id, 'cache-progress'] as const,
      queryFn: () => apiClient.getCacheProgress(db.id),
      // Fast only while a refresh runs; a quiet server needs a slow
      // heartbeat, not a request per second per database.
      refetchInterval: (query: { state: { data?: CacheRefreshProgress } }) =>
        query.state.data?.is_refreshing ? 1000 : 10_000,
      refetchIntervalInBackground: false,
    })),
  });
  const scans = useQueries({
    queries: databases.map((db) => ({
      queryKey: ['db', db.id, 'quality-scan'] as const,
      queryFn: () => apiClient.getSpatialScanStatus(db.id),
      refetchInterval: (query: { state: { data?: SpatialScanStatus } }) =>
        query.state.data?.progress.running ? 1000 : false,
      refetchIntervalInBackground: false,
    })),
  });
  const backfills = useQueries({
    queries: databases.map((db) => ({
      queryKey: ['db', db.id, 'quality-backfill'] as const,
      queryFn: () => apiClient.getQualityBackfillStatus(db.id),
      refetchInterval: (query: { state: { data?: QualityBackfillStatus } }) =>
        query.state.data?.progress.running ? 1000 : false,
      refetchIntervalInBackground: false,
    })),
  });

  const perDb: DatabaseActivity[] = databases.map((db, index) => ({
    dbId: db.id,
    dbName: db.name,
    refresh: refreshes[index]?.data,
    scan: scans[index]?.data,
    backfill: backfills[index]?.data,
  }));
  const items = activityItems(perDb, stacks, wbpp.data, projectOf);
  const summary = summarize(items);

  // A database whose work just finished has new images, metrics and grades
  // for every view, so its queries are refreshed as a whole.
  const busyIds = perDb.filter(busy).map((db) => db.dbId).sort().join('|');
  const previousBusy = useRef<string[]>([]);
  useEffect(() => {
    const now = busyIds ? busyIds.split('|') : [];
    const done = previousBusy.current.filter((id) => !now.includes(id));
    previousBusy.current = now;
    for (const id of done) queryClient.invalidateQueries({ queryKey: ['db', id] });
  }, [busyIds, queryClient]);

  // Frame errors belong to a scan this header saw running. The server keeps
  // the last scan's counts until the next one starts, so a database that
  // merely finished a refresh must not report yesterday's errors.
  const scanningIds = perDb
    .filter((db) => db.scan?.progress.running || db.backfill?.progress.running)
    .map((db) => db.dbId)
    .sort()
    .join('|');
  const previousScanning = useRef<string[]>([]);
  const [scanErrors, setScanErrors] = useState<ScanErrorNote[]>([]);
  const scanErrorsRef = useRef(scanErrors);
  scanErrorsRef.current = scanErrors;
  useEffect(() => {
    const now = scanningIds ? scanningIds.split('|') : [];
    const ended = previousScanning.current.filter((id) => !now.includes(id));
    previousScanning.current = now;
    let next = scanErrorsRef.current;
    for (const id of ended) {
      const db = perDb.find((entry) => entry.dbId === id);
      const progress = db?.scan?.progress;
      if (db && progress && !progress.running && progress.errors > 0) {
        const project =
          progress.target_id != null ? projectOf(db.dbId, progress.target_id) : undefined;
        const note: ScanErrorNote = {
          dbId: db.dbId,
          dbName: db.dbName,
          message: `${db.dbName}: ${progress.errors} frame${progress.errors === 1 ? '' : 's'} failed${
            progress.last_error ? ` — ${progress.last_error}` : ''
          }`,
          href:
            project != null
              ? `/sequence?${new URLSearchParams({
                  db: db.dbId,
                  project: String(project),
                  target: String(progress.target_id),
                }).toString()}`
              : undefined,
        };
        // A database's newer scan replaces its older note.
        next = [...next.filter((entry) => entry.dbId !== db.dbId), note];
      }
    }
    if (next !== scanErrorsRef.current) {
      // The finish effect below may run in this same commit, before the
      // state lands, when the scan was the last job.
      scanErrorsRef.current = next;
      setScanErrors(next);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- read when the scanning set changes, which is when a scan ends.
  }, [scanningIds]);

  const dismissScanError = useCallback((dbId: string) => {
    setScanErrors((current) => current.filter((entry) => entry.dbId !== dbId));
  }, []);

  // When the whole queue empties, the chip says "Done" for a moment and
  // leaves. Scan errors it holds keep it as "Finished with errors" until
  // each is dismissed.
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
  // The last error dismissed: nothing is left to say.
  useEffect(() => {
    if (scanErrors.length === 0) setFinished((current) => (current?.errors ? null : current));
  }, [scanErrors.length]);
  useEffect(() => () => {
    if (finishedTimer.current != null) window.clearTimeout(finishedTimer.current);
  }, []);

  return { items, summary, finished, scanErrors, dismissScanError };
}
