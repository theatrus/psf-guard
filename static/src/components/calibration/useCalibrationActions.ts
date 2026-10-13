import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';

export type ValidityChoice = 'both' | 'forward' | 'backward';
export type QualityChoice = 'use' | 'exclude' | 'clear';

/**
 * The library writes the calibration page offers: keep a frame out or use
 * it anyway, mark which lights frames serve, forget frames, clear masters.
 * Each refreshes everything the page shows of the library, and the
 * project health that follows from it.
 */
export function useCalibrationActions(dbId: string) {
  const queryClient = useQueryClient();
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ['db', dbId, 'calibrations'] });
    void queryClient.invalidateQueries({ queryKey: ['db', dbId, 'calibration-health'] });
  };
  const choose = useMutation({
    mutationFn: (input: { frameUuids: string[]; choice: QualityChoice }) =>
      apiClient.setCalibrationQualityOverride(dbId, input.frameUuids, input.choice),
    onSuccess: refresh,
  });
  const mark = useMutation({
    mutationFn: (input: { frameUuids: string[]; direction: ValidityChoice }) =>
      apiClient.setCalibrationValidity(dbId, input.frameUuids, input.direction),
    onSuccess: refresh,
  });
  const forget = useMutation({
    mutationFn: (frameUuids: string[]) =>
      frameUuids.length === 1
        ? apiClient.forgetCalibrationFrame(dbId, frameUuids[0])
        : apiClient.forgetCalibrationFrames(dbId, frameUuids),
    onSuccess: refresh,
  });
  const clearMasters = useMutation({
    mutationFn: () => apiClient.clearCalibrationMasters(dbId),
    onSuccess: refresh,
  });
  const error = choose.error || mark.error || forget.error || clearMasters.error;
  return {
    choose,
    mark,
    forget,
    clearMasters,
    pending: choose.isPending || mark.isPending || forget.isPending || clearMasters.isPending,
    error: error instanceof Error ? error.message : error ? String(error) : null,
  };
}
