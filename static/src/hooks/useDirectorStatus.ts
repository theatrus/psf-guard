import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';

export function useDirectorStatus() {
  return useQuery({
    queryKey: ['directorStatus'],
    queryFn: apiClient.getDirectorStatus,
    staleTime: 30_000,
    retry: false,
  });
}
