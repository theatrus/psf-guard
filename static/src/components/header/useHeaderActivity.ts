import { useEffect, useMemo, useRef, useState } from 'react';
import { useQueries, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type {
  CacheRefreshProgress,
  QualityBackfillStatus,
  SpatialScanStatus,
} from '../../api/types';
import { useAllDatabases } from '../../hooks/useDatabases';
import { useStackActivity } from '../../hooks/useStackActivity';
import { activityItems, summarize, type DatabaseActivity } from './activityItems';

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
  const { active: stacks } = useStackActivity();

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
  const items = activityItems(perDb, stacks);
  const summary = summarize(items);

  // A database whose work just finished has new images, metrics and grades
  // for every view, so its queries are refreshed as a whole.
  const busyIds = perDb.filter(busy).map((db) => db.dbId).sort().join('|');
  const previousBusy = useRef<string[]>([]);
  const scanErrors = useMemo(
    () => new Map(perDb.map((db) => [db.dbId, db.scan?.progress])),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- rebuilt when the busy set changes, which is when it is read.
    [busyIds]
  );
  const [finished, setFinished] = useState<FinishedNote | null>(null);
  const finishedTimer = useRef<number | null>(null);
  useEffect(() => {
    const now = busyIds ? busyIds.split('|') : [];
    const done = previousBusy.current.filter((id) => !now.includes(id));
    previousBusy.current = now;
    for (const id of done) queryClient.invalidateQueries({ queryKey: ['db', id] });
    if (done.length === 0) return;
    const failed = done
      .map((id) => scanErrors.get(id))
      .find((progress) => progress && !progress.running && progress.errors > 0);
    if (finishedTimer.current != null) window.clearTimeout(finishedTimer.current);
    setFinished({
      errors: !!failed,
      message: failed
        ? `${failed.errors} frame${failed.errors === 1 ? '' : 's'} failed${failed.last_error ? `: ${failed.last_error}` : ''}`
        : undefined,
    });
    finishedTimer.current = window.setTimeout(() => {
      setFinished(null);
      finishedTimer.current = null;
    }, FINISHED_MS);
  }, [busyIds, queryClient, scanErrors]);
  useEffect(() => () => {
    if (finishedTimer.current != null) window.clearTimeout(finishedTimer.current);
  }, []);

  // Stack builds have no per-database finish here: their panels refresh
  // themselves. A finished build still earns the "Done" note.
  const stackCount = stacks.length;
  const previousStacks = useRef(stackCount);
  useEffect(() => {
    const before = previousStacks.current;
    previousStacks.current = stackCount;
    if (before > 0 && stackCount === 0 && !busyIds) {
      if (finishedTimer.current != null) window.clearTimeout(finishedTimer.current);
      setFinished((current) => current ?? { errors: false });
      finishedTimer.current = window.setTimeout(() => {
        setFinished(null);
        finishedTimer.current = null;
      }, FINISHED_MS);
    }
  }, [stackCount, busyIds]);

  return { items, summary, finished: summary.count > 0 ? null : finished };
}
