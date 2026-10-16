import { useCallback, useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { FlatClockRun } from '../../api/types';

/**
 * Each rig's clock, from the library summary the Calibration page already
 * reads, so a frame's or master's time reads on the clock its rig's frames
 * record, as flat groups' times do. Unknown rigs read on the viewer's.
 */
export function useRigClocks(dbId: string): (rig?: string | null) => FlatClockRun[] | undefined {
  const summary = useQuery({
    queryKey: ['db', dbId, 'calibrations'],
    queryFn: () => apiClient.getCalibrationLibrary(dbId),
    enabled: !!dbId,
  });
  const clocks = useMemo(
    () => new Map((summary.data?.rigs ?? []).map((rig) => [rig.rig_uuid, rig.clock ?? []])),
    [summary.data?.rigs],
  );
  return useCallback((rig?: string | null) => (rig ? clocks.get(rig) : undefined), [clocks]);
}
