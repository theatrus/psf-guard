import { useEffect, useRef } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { TS_SYNC_QUERY_KEY } from '../api/tsSyncTypes';

export function useTsSyncJobs(enabled: boolean) {
  const client = useQueryClient();
  const completed = useRef(new Map<string, number>());
  const query = useQuery({
    queryKey: TS_SYNC_QUERY_KEY,
    queryFn: apiClient.getTsSyncJobs,
    enabled,
    retry: false,
    refetchInterval: q => q.state.data?.some(job => job.running) ? 1000 : 10_000,
  });
  useEffect(() => {
    let changed = false;
    for (const job of query.data ?? []) {
      if (!job.running && completed.current.get(job.id) !== job.started_at) {
        completed.current.set(job.id, job.started_at);
        changed = true;
      }
    }
    if (changed) {
      void client.invalidateQueries({ queryKey: ['databases'] });
      void client.invalidateQueries({ queryKey: ['db'] });
    }
  }, [query.data, client]);
  return query;
}
