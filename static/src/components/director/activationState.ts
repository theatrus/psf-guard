import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { retryWhenBusy } from './retry';

/** What the rig databases hold against what is saved here: the last
 *  activation, and which saved parts (plan, framing) it has not sent yet.
 *  The queries share their cache keys with the editors and the activation
 *  panel, so the page asks the server once. */
export function useActivationState(projectId: string) {
  const last = useQuery({ queryKey: ['directorActivation', projectId], queryFn: () => apiClient.getDirectorActivation(projectId), retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false });
  const savedPlan = useQuery({ queryKey: ['directorPlan', projectId], queryFn: () => apiClient.getDirectorPlan(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const savedFraming = useQuery({ queryKey: ['directorFraming', projectId], queryFn: () => apiClient.getDirectorFramingDraft(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const planRevision = savedPlan.data?.plan?.revision;
  const framingRevision = savedFraming.data?.draft?.revision;
  // The survey, view width and compared rigs are the view's; only a change
  // to what activation writes leaves the rigs behind.
  const layoutRevision = savedFraming.data?.draft?.layout_revision ?? framingRevision;
  const behind = last.data ? [
    planRevision !== undefined && planRevision > last.data.plan_revision && 'plan',
    layoutRevision !== undefined && layoutRevision > last.data.framing_revision && 'framing',
  ].filter((part): part is string => !!part) : [];
  return { last, savedPlan, savedFraming, planRevision, framingRevision, behind };
}
