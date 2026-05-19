import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { SequenceAnalysisRequest } from '../api/types';
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
