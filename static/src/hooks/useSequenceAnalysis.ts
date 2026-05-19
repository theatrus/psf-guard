import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { SequenceAnalysisRequest } from '../api/types';
import { useState, useCallback } from 'react';

export function useSequenceAnalysis() {
  const [request, setRequest] = useState<SequenceAnalysisRequest | null>(null);

  const query = useQuery({
    queryKey: ['sequence-analysis', request?.target_id, request?.filter_name],
    queryFn: () => apiClient.analyzeSequence(request!),
    enabled: !!request?.target_id,
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

export function useImageQuality(imageId: number | undefined) {
  return useQuery({
    queryKey: ['image-quality', imageId],
    queryFn: () => apiClient.getImageQuality(imageId!),
    enabled: !!imageId,
    staleTime: 60000,
  });
}
