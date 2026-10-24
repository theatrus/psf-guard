import { useEffect, useRef } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { sameScoring, SCORING_DEFAULTS, scoringFromFields, stackScoringSettings } from '../utils/scoring';
import { getDisplayPreferences } from './useDisplayPreferences';
import { SCORING_DEFAULTS_QUERY_KEY, useScoringDefaultsQuery } from './useScoringPreferences';

/**
 * Before the server kept one copy of the scoring defaults, each person kept
 * their own: with their account when signed in, else in this browser. The
 * first page to find the server without a copy saves the person's own
 * values there, once, when they differ from the built-in ones and the
 * person may change settings. Built-in values need no copy: the server
 * already uses them.
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
        if (cancelled || sameScoring(own, SCORING_DEFAULTS)) return;
        const saved = await apiClient.updateScoringDefaults(stackScoringSettings(own));
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
