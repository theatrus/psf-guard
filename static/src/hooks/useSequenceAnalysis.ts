import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  ImageQualityResult,
  DatabaseSequenceAnalysisRequest,
  ProjectSequenceAnalysisRequest,
  SequenceAnalysisRequest,
} from '../api/types';
import { useState, useCallback } from 'react';
import type { QualityScoreScope } from '../utils/qualityScore';
import {
  penaltyKeyOf,
  penaltyParamsOf,
  useScoringPreferences,
} from './useScoringPreferences';
import type { ScoringPreferences } from './useScoringPreferences';

/**
 * Query key for a target-scoped analysis. One builder shared by
 * useSequenceAnalysis and useScopedQuality: the Sequence view and the grid
 * badges cache-share only while their keys stay byte-identical, and the
 * penalty scales must be in the key so a preference change refetches.
 */
function targetAnalysisKey(
  dbId: string | null | undefined,
  targetId: number | undefined | null,
  filterName: string | undefined,
  penalties: ScoringPreferences,
) {
  return [
    'db', dbId, 'sequence-analysis', targetId, filterName,
    ...penaltyKeyOf(penalties),
  ];
}

export function useSequenceAnalysis(dbId: string | null | undefined) {
  const [request, setRequest] = useState<SequenceAnalysisRequest | null>(null);
  // Key and payload derive from the same snapshot: reading the store again
  // at fetch time could cache a response computed with new scales under a
  // key naming the old ones.
  const penalties = useScoringPreferences();

  const query = useQuery({
    queryKey: targetAnalysisKey(dbId, request?.target_id, request?.filter_name, penalties),
    queryFn: () =>
      apiClient.analyzeSequence(dbId!, { ...request!, ...penaltyParamsOf(penalties) }),
    enabled: !!dbId && !!request?.target_id,
    staleTime: 60000,
  });

  const analyze = useCallback((req: SequenceAnalysisRequest) => {
    setRequest(req);
  }, []);

  return {
    analyze,
    data: query.data,
    isLoading: query.isLoading && !!request,
    error: query.error,
    reset: () => setRequest(null),
  };
}

export function useImageQuality(dbId: string | null | undefined, imageId: number | undefined) {
  const penalties = useScoringPreferences();
  return useQuery({
    queryKey: ['db', dbId, 'image-quality', imageId, ...penaltyKeyOf(penalties)],
    queryFn: () => apiClient.getImageQuality(dbId!, imageId!, penaltyParamsOf(penalties)),
    enabled: !!dbId && !!imageId,
    staleTime: 60000,
  });
}

/**
 * Load Sequence quality for a Grid scope in one request. The server keeps
 * target/filter comparison groups separate even for project and database
 * scopes, so the page never issues one analysis request per card.
 */
export function useScopedQuality(
  dbId: string | null | undefined,
  projectId: number | null | undefined,
  targetId: number | null | undefined,
  filterName?: string,
) {
  const penalties = useScoringPreferences();
  const penaltyParams = penaltyParamsOf(penalties);
  const request:
    | SequenceAnalysisRequest
    | ProjectSequenceAnalysisRequest
    | DatabaseSequenceAnalysisRequest = targetId != null
      ? { target_id: targetId, filter_name: filterName, ...penaltyParams }
      : projectId != null
        ? { project_id: projectId, filter_name: filterName, ...penaltyParams }
        : { all_projects: true, filter_name: filterName, ...penaltyParams };
  const queryKey = targetId != null
    ? targetAnalysisKey(dbId, targetId, filterName, penalties)
    : projectId != null
      ? ['db', dbId, 'sequence-analysis', 'project', projectId, filterName, ...penaltyKeyOf(penalties)]
      : ['db', dbId, 'sequence-analysis', 'all-projects', filterName, ...penaltyKeyOf(penalties)];
  const query = useQuery({
    queryKey,
    queryFn: () => apiClient.analyzeSequence(dbId!, request),
    enabled: !!dbId,
    staleTime: 60000,
  });

  // Show the same score basis the Sequence view shows by default: the
  // all-sessions (target/filter) rollup for filters with several sessions,
  // the per-session score otherwise. Building the map from sessions alone
  // made the grid badge disagree with the Sequence view for any frame
  // whose small session normalized against itself. The rollup entry
  // carries only the score fields, so it overlays a copy of the session
  // entry — category, pointing, and overlays stay intact.
  const qualityByImage = new Map<number, ImageQualityResult>();
  const scopeByImage = new Map<number, QualityScoreScope>();
  const sessionsPerFilter = new Map<string, number>();
  for (const sequence of query.data?.sequences ?? []) {
    const key = `${sequence.target_id}:${sequence.filter_name}`;
    sessionsPerFilter.set(key, (sessionsPerFilter.get(key) ?? 0) + 1);
    for (const quality of sequence.images) {
      qualityByImage.set(quality.image_id, quality);
      scopeByImage.set(quality.image_id, 'capture_sequence');
    }
  }
  for (const rollup of query.data?.target_filter_rollups ?? []) {
    if ((sessionsPerFilter.get(`${rollup.target_id}:${rollup.filter_name}`) ?? 0) <= 1) {
      continue;
    }
    for (const score of rollup.images) {
      const session = qualityByImage.get(score.image_id);
      if (!session) continue;
      qualityByImage.set(score.image_id, {
        ...session,
        quality_score: score.quality_score,
        normalized_metrics: score.normalized_metrics,
        details: score.details,
      });
      scopeByImage.set(score.image_id, 'target_filter');
    }
  }

  return {
    qualityByImage,
    scopeByImage,
    isLoading: query.isLoading,
    error: query.error,
  };
}
