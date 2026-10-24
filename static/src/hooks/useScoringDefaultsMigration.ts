import { useEffect, useRef } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { scoringFromFields, stackScoringSettings } from '../utils/scoring';
import { getDisplayPreferences } from './useDisplayPreferences';
import { SCORING_DEFAULTS_QUERY_KEY, useScoringDefaultsQuery } from './useScoringPreferences';

/**
 * Before the server kept one copy of the scoring defaults, each person kept
 * their own: with their account when signed in, else in this browser. The
 * first page an editor opens with the server holding no copy saves that
 * editor's values there, built-in ones included, so the first editor
 * decides and a later editor's older values never replace them. The save
 * is conditional: when another page saved first, the server keeps that
 * copy and this page takes it.
 *
 * A signed-in person's values are read from their account's document, not
 * this browser's copy of it, which may not have arrived yet.
 */
export function useScoringDefaultsMigration(username: string | undefined, canWrite: boolean): void {
  const queryClient = useQueryClient();
  const { data } = useScoringDefaultsQuery();
  const attempted = useRef(false);
  const unstored = data !== undefined && !data.stored;

  useEffect(() => {
    if (!unstored || !canWrite || attempted.current) return;
    attempted.current = true;
    let cancelled = false;
    let finished = false;
    void (async () => {
      try {
        const own = username
          ? scoringFromFields((await apiClient.getMyPreferences()).preferences)
          : scoringFromFields(getDisplayPreferences());
        if (cancelled) return;
        const saved = await apiClient.updateScoringDefaults(stackScoringSettings(own), {
          onlyIfUnstored: true,
        });
        queryClient.setQueryData(SCORING_DEFAULTS_QUERY_KEY, saved);
      } catch {
        // The built-in values stay in use; Settings › Review can save them.
      } finally {
        finished = true;
      }
    })();
    return () => {
      cancelled = true;
      // Stopped part way, as when the person changed: the next run tries
      // again.
      if (!finished) attempted.current = false;
    };
  }, [unstored, canWrite, username, queryClient]);
}
