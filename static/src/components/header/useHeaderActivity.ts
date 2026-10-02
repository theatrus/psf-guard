import { useEffect, useRef, useState } from 'react';
import { useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type {
  CacheRefreshProgress,
  QualityBackfillStatus,
  SpatialScanStatus,
  WbppActivity,
} from '../../api/types';
import { useAllDatabases } from '../../hooks/useDatabases';
import { useStackActivity } from '../../hooks/useStackActivity';
import { activityItems, summarize, type DatabaseActivity } from './activityItems';

export const WBPP_ACTIVITY_QUERY_KEY = ['wbpp-activity'] as const;

/** How long the chip says "Done" after the last job finishes. */
export const FINISHED_MS = 2500;

export interface FinishedNote {
  /** A quality scan that just finished reported frame errors. */
  errors: boolean;
  message?: string;
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
  const queryClient = useQueryClient();
  const { active: stacks, scheduled } = useStackActivity();
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
  const items = activityItems(perDb, stacks, wbpp.data, scheduled);
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
  const [scanError, setScanError] = useState<FinishedNote | null>(null);
  const scanErrorRef = useRef(scanError);
  scanErrorRef.current = scanError;
  useEffect(() => {
    const now = scanningIds ? scanningIds.split('|') : [];
    const ended = previousScanning.current.filter((id) => !now.includes(id));
    previousScanning.current = now;
    for (const id of ended) {
      const db = perDb.find((entry) => entry.dbId === id);
      const progress = db?.scan?.progress;
      if (db && progress && !progress.running && progress.errors > 0) {
        const note = {
          errors: true,
          message: `${db.dbName}: ${progress.errors} frame${progress.errors === 1 ? '' : 's'} failed${
            progress.last_error ? ` — ${progress.last_error}` : ''
          }`,
        };
        // The finish effect below may run in this same commit, before the
        // state lands, when the scan was the last job.
        scanErrorRef.current = note;
        setScanError(note);
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- read when the scanning set changes, which is when a scan ends.
  }, [scanningIds]);

  // When the whole queue empties, the chip says so for a moment, with any
  // scan errors it has been holding; then it leaves and forgets them.
  const [finished, setFinished] = useState<FinishedNote | null>(null);
  const finishedTimer = useRef<number | null>(null);
  // Running and lined-up work; a refresh still settling is not, so
  // skipping one does not say "Done".
  const workCount = items.filter((item) => item.kind !== 'automatic').length;
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
    setFinished(scanErrorRef.current ?? { errors: false });
    finishedTimer.current = window.setTimeout(() => {
      setFinished(null);
      setScanError(null);
      finishedTimer.current = null;
    }, FINISHED_MS);
  }, [workCount]);
  useEffect(() => () => {
    if (finishedTimer.current != null) window.clearTimeout(finishedTimer.current);
  }, []);

  return { items, summary, finished, scanError };
}
