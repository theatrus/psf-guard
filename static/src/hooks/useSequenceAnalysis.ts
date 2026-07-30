import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  ImageQualityResult,
  ProjectSequenceAnalysisRequest,
  SequenceAnalysisRequest,
} from '../api/types';
import { useState, useCallback } from 'react';

export function useSequenceAnalysis(dbId: string | null | undefined) {
  const [request, setRequest] = useState<SequenceAnalysisRequest | null>(null);

  const query = useQuery({
    queryKey: ['db', dbId, 'sequence-analysis', request?.target_id, request?.filter_name],
    queryFn: () => apiClient.analyzeSequence(dbId!, request!),
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
  return useQuery({
    queryKey: ['db', dbId, 'image-quality', imageId],
    queryFn: () => apiClient.getImageQuality(dbId!, imageId!),
    enabled: !!dbId && !!imageId,
    staleTime: 60000,
  });
}

/**
 * Load Sequence quality for a Grid scope in one request. Project scope is
 * handled on the server so a page never issues one analysis request per card.
 */
export function useGridQuality(
  dbId: string | null | undefined,
  projectId: number | null | undefined,
  targetId: number | null | undefined,
  filterName?: string,
) {
  const request: SequenceAnalysisRequest | ProjectSequenceAnalysisRequest | null = targetId != null
    ? { target_id: targetId, filter_name: filterName }
    : projectId != null
      ? { project_id: projectId, filter_name: filterName }
      : null;
  const queryKey = targetId != null
    ? ['db', dbId, 'sequence-analysis', targetId, filterName]
    : ['db', dbId, 'sequence-analysis', 'project', projectId, filterName];
  const query = useQuery({
    queryKey,
    queryFn: () => apiClient.analyzeSequence(dbId!, request!),
    enabled: !!dbId && request !== null,
    staleTime: 60000,
  });

  const qualityByImage = new Map<number, ImageQualityResult>();
  for (const sequence of query.data?.sequences ?? []) {
    for (const quality of sequence.images) {
      qualityByImage.set(quality.image_id, quality);
    }
  }

  return { qualityByImage, isLoading: query.isLoading, error: query.error };
}
