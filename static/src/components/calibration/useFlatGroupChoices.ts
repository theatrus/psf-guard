import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { FlatClockRun, FlatGroupOverride, FlatSplit } from '../../api/types';
import { useAccess } from '../../auth/access';
import { flatGroupsKey } from './useFlatGroups';
import { windowLabel } from './flatGroupLabels';

/** A split, join or reset of one family's groups. */
export type GroupOverrideRequest = Parameters<typeof apiClient.setFlatGroupOverride>[1];
/** A side, group or reset for some lights. */
export type LightChoiceRequest = Parameters<typeof apiClient.setFlatLightChoice>[1];

/**
 * Whether this person may change flat group choices: an editor, on a
 * server that allows database management (the desktop app always does).
 * Everyone sees the choices; only then may they change them.
 */
export function useFlatChoiceAccess(): boolean {
  const { canWrite } = useAccess();
  const serverInfo = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000,
  });
  return canWrite && !!serverInfo.data?.allow_database_management;
}

/** What a choice changes on screen: the groups, what they feed, each
 *  light's account and the project's marks. */
function invalidate(queryClient: ReturnType<typeof useQueryClient>, dbId: string) {
  void queryClient.invalidateQueries({ queryKey: flatGroupsKey(dbId) });
  void queryClient.invalidateQueries({ queryKey: ['db', dbId, 'image-calibration'] });
  void queryClient.invalidateQueries({ queryKey: ['db', dbId, 'calibration-marks'] });
  void queryClient.invalidateQueries({ queryKey: ['db', dbId, 'calibration-health'] });
}

/** Split, join or reset one family's groups. */
export function useFlatGroupOverride(dbId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: GroupOverrideRequest) => apiClient.setFlatGroupOverride(dbId, request),
    onSuccess: () => invalidate(queryClient, dbId),
  });
}

/** Put lights on a side of a change or in a group, or reset that. */
export function useFlatLightChoice(dbId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: LightChoiceRequest) => apiClient.setFlatLightChoice(dbId, request),
    onSuccess: () => invalidate(queryClient, dbId),
  });
}

/** How a person changed an edge, in a few words; null when nobody did. */
export function handLabel(split: FlatSplit): string | null {
  if (split.by_hand === 'split') return 'Split by you';
  if (split.by_hand === 'joined') return 'Joined by you';
  if (split.choice?.choice === 'split') return 'Split by you · measured too';
  return null;
}

/** A camera-side change a split or join stands against, in a sentence,
 *  its window on the rig's `clock`. */
export function conflictText(choice: FlatGroupOverride, clock?: readonly FlatClockRun[]): string | null {
  const conflict = choice.conflict;
  if (!conflict) return null;
  const seen = conflict.filters.length > 0 ? `seen in ${conflict.filters.join(', ')}` : 'seen on the camera side';
  return choice.choice === 'join'
    ? `A camera-side change (${conflict.what}, ${seen}, ${windowLabel(conflict.start, conflict.end, clock)}) also falls here: these flats pool, and its lights still take their side of the change. Set a light's side to choose otherwise.`
    : `It overlaps a camera-side change (${conflict.what}, ${seen}, ${windowLabel(conflict.start, conflict.end, clock)}): both apply.`;
}
