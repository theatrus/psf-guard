import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { DirectorActivationRig } from '../../api/directorTypes';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { shootingRigs } from './planModel';
import { retryWhenBusy } from './retry';

/** Whether an activation would write anything in this rig's database: a
 *  row it creates, takes over, updates or turns off. */
export const writesRows = (rig: DirectorActivationRig) => rig.changes.some(change => change.action !== 'unchanged' && change.action !== 'keep');

/** A rig whose database already holds everything an activation would
 *  write there. */
const holdsPlan = (rig: DirectorActivationRig) => rig.profile_id !== null && rig.changes.length > 0 && !writesRows(rig);

const listed = (words: string[]) => words.length <= 1 ? words.join('') : `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;

/** What the rig databases hold against what is saved here: the last
 *  activation, which saved parts (plan, framing) it has not sent yet, which
 *  rigs shooting the saved plan it left out (one skipped with a warning, or
 *  added since), and which rigs' rows differ from the plan though the record
 *  is up to date. The queries share their cache keys with the editors and
 *  the activation panel, so the page asks the server once. */
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
  const shooting = shootingRigs(savedPlan.data?.plan);
  const left = last.data ? shooting.filter(id => !activated.has(id)) : [];
  // The record can lag rows that already match: a plan taken in from Target
  // Scheduler, or rows that came by Sync. Rows can also differ under a record
  // that is up to date: N.I.N.A. changed them, or a newer build activates
  // more (it turns off a plan another exposure length replaced). So the
  // check runs whatever the record says. It runs an activation on a copy of
  // each rig's planning tables, so it never writes or locks them.
  const needsCheck = last.isSuccess && savedPlan.isSuccess && savedFraming.isSuccess && shooting.length > 0;
  const check = useQuery({ queryKey: ['directorActivationCheck', projectId, planRevision, layoutRevision, last.data?.revision ?? 0], queryFn: () => apiClient.checkDirectorActivation(projectId), enabled: needsCheck, retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false, staleTime: 30_000 });
  const holding = new Set((check.data?.rigs ?? []).filter(holdsPlan).map(rig => rig.rig.id));
  // Every rig shooting the plan holds it, and no rig turned off has a change waiting.
  const matches = needsCheck && check.isSuccess && shooting.every(id => holding.has(id)) && (check.data?.rigs ?? []).every(holdsPlan);
  const checking = needsCheck && check.isPending;
  const missingIds = matches ? [] : left.filter(id => !holding.has(id));
  const missing = missingIds.map(id => profiles.data?.find(rig => rig.rig.id === id)?.catalog_name ?? 'a rig');
  // Rigs whose rows an activation would change: any row created, taken
  // over, updated or turned off.
  const changed = new Set((check.data?.rigs ?? []).filter(writesRows).map(rig => rig.rig.id));
  // Those the record calls up to date.
  const differing = matches || !last.data || parts.length > 0 ? []
    : (check.data?.rigs ?? []).filter(rig => changed.has(rig.rig.id) && !missingIds.includes(rig.rig.id)).map(rig => rig.catalog_name);
  // Everything the rigs lack: the saved parts, the rigs left out, then the
  // rigs whose rows differ.
  const behind = matches ? [] : [...parts, ...missing, ...differing];
  const behindText = [
    parts.length > 0 && `Saved ${listed(parts)} not on the rigs yet`,
    missing.length > 0 && `${listed(missing)} not activated yet`,
    differing.length > 0 && `${listed(differing)} ${differing.length === 1 ? 'differs' : 'differ'} from the plan`,
  ].filter(Boolean).join('; ');
  return { last, savedPlan, savedFraming, planRevision, framingRevision, behind, behindText, matches, holding, changed, checking };
}

/** Whether the page should ask for an activation: someone may write, the
 *  server writes rig databases, the saved plan shoots something, and the
 *  rigs lack the saved plan, its layout or a rig that shoots it, or their
 *  rows differ from it. The save bar comes first, so the page also waits for
 *  unsaved edits, and it waits for the check rather than ask and then take
 *  it back. */
export function useActivationDue(projectId: string, canWrite: boolean, unsaved: number): boolean {
  const { last, savedPlan, behind, matches, checking } = useActivationState(projectId);
  const status = useDirectorStatus();
  const shoots = shootingRigs(savedPlan.data?.plan).length > 0;
  return canWrite && !!status.data?.database_management && unsaved === 0 && last.isSuccess && shoots
    && (!last.data || behind.length > 0) && !matches && !checking;
}
