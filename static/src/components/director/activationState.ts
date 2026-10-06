import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { shootingRigs } from './planModel';
import { retryWhenBusy } from './retry';

const listed = (words: string[]) => words.length <= 1 ? words.join('') : `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;

/** What the rig databases hold against what is saved here: the last
 *  activation, which saved parts (plan, framing) it has not sent yet, and
 *  which rigs shooting the saved plan it left out (one skipped with a
 *  warning, or added since). The queries share their cache keys with the
 *  editors and the activation panel, so the page asks the server once. */
export function useActivationState(projectId: string) {
  const last = useQuery({ queryKey: ['directorActivation', projectId], queryFn: () => apiClient.getDirectorActivation(projectId), retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false });
  const savedPlan = useQuery({ queryKey: ['directorPlan', projectId], queryFn: () => apiClient.getDirectorPlan(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const savedFraming = useQuery({ queryKey: ['directorFraming', projectId], queryFn: () => apiClient.getDirectorFramingDraft(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const profiles = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const planRevision = savedPlan.data?.plan?.revision;
  const framingRevision = savedFraming.data?.draft?.revision;
  // The survey, view width and compared rigs are the view's; only a change
  // to what activation writes leaves the rigs behind.
  const layoutRevision = savedFraming.data?.draft?.layout_revision ?? framingRevision;
  const parts = last.data ? [
    planRevision !== undefined && planRevision > last.data.plan_revision && 'plan',
    layoutRevision !== undefined && layoutRevision > last.data.framing_revision && 'framing',
  ].filter((part): part is string => !!part) : [];
  // The revisions can match while a rig has nothing: activation skipped it.
  const activated = new Set(last.data?.rigs.map(rig => rig.rig_id) ?? []);
  const missing = last.data
    ? shootingRigs(savedPlan.data?.plan).filter(id => !activated.has(id)).map(id => profiles.data?.find(rig => rig.rig.id === id)?.catalog_name ?? 'a rig')
    : [];
  // Everything the rigs lack: the saved parts, then the rigs left out.
  const behind = [...parts, ...missing];
  const behindText = [
    parts.length > 0 && `Saved ${listed(parts)} not on the rigs yet`,
    missing.length > 0 && `${listed(missing)} not activated yet`,
  ].filter(Boolean).join('; ');
  return { last, savedPlan, savedFraming, planRevision, framingRevision, behind, behindText };
}

/** Whether the page should ask for an activation: someone may write, the
 *  server writes rig databases, the saved plan shoots something, and the
 *  rigs lack the saved plan, its layout or a rig that shoots it. The save
 *  bar comes first, so the page also waits for unsaved edits. */
export function useActivationDue(projectId: string, canWrite: boolean, unsaved: number): boolean {
  const { last, savedPlan, behind } = useActivationState(projectId);
  const status = useDirectorStatus();
  const shoots = shootingRigs(savedPlan.data?.plan).length > 0;
  return canWrite && !!status.data?.database_management && unsaved === 0 && last.isSuccess && shoots && (!last.data || behind.length > 0);
}
