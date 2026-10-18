import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../../api/client';
import type { DirectorActivationChange, DirectorActivationRemotePush, DirectorActivationReport, DirectorActivationRig } from '../../api/directorTypes';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { shootingRigs } from './planModel';
import { retryWhenBusy } from './retry';

/** Whether an activation would write anything in this rig's database: a
 *  row it creates, takes over, updates or turns off. */
export const writesRows = (rig: DirectorActivationRig) => rig.changes.some(change => change.action !== 'unchanged' && change.action !== 'keep')
  || !!rig.push?.ts_file && Object.values(rig.push.summary).some(count => count > 0);

/** A rig whose database already holds everything an activation would
 *  write there. */
const holdsPlan = (rig: DirectorActivationRig) => rig.profile_id !== null && rig.changes.length > 0 && !writesRows(rig);

const listed = (words: string[]) => words.length <= 1 ? words.join('') : `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;

/** A remote rig whose site has not had the last activation, with the
 *  rig's database name. */
export type UnsentPush = DirectorActivationRemotePush & { rig_name: string };

/** The sites these pushes go to, each once. */
export const sitesOf = (unsent: UnsentPush[]) => [...new Set(unsent.map(push => push.peer_name))];

/** The send button's label: "Send to observatory-pi again", or without
 *  "again" when no push was tried yet. */
export const sendLabel = (unsent: UnsentPush[]) => `Send to ${listed(sitesOf(unsent))}${unsent.some(push => push.attempted_at_ms) ? ' again' : ''}`;

/** What a rig's turn-offs come to, in a few words: "1 plan", "2 plans",
 *  "the project", "the project and 1 plan"; empty for none. */
export function turnedOff(changes: DirectorActivationChange[]): string {
  const off = changes.filter(change => change.action === 'disable');
  const plans = off.filter(change => change.kind !== 'project').length;
  return listed([
    off.some(change => change.kind === 'project') ? 'the project' : '',
    plans > 0 ? `${plans} plan${plans === 1 ? '' : 's'}` : '',
  ].filter(Boolean));
}

/** What a refused activation found different: the saved plan or framing,
 *  and the rigs whose rows the new preview writes otherwise. */
export function describeChange(before: DirectorActivationReport, after: DirectorActivationReport): string {
  const parts: string[] = [];
  if (before.plan_revision !== after.plan_revision) parts.push(`the saved plan (revision ${before.plan_revision} → ${after.plan_revision})`);
  if (before.framing_revision !== after.framing_revision) parts.push(`the framing (revision ${before.framing_revision} → ${after.framing_revision})`);
  // What an activation writes, as the server's digest reads it: a frame
  // taken since changes a count the preview quotes, never what is written.
  const rows = (rig: DirectorActivationRig | undefined) => JSON.stringify(rig
    ? rig.changes.map(change => [change.kind, change.action, change.name, withoutFrameCounts(change.detail)])
    : null);
  const ids = new Set([...before.rigs, ...after.rigs].map(rig => rig.rig.id));
  for (const id of ids) {
    const was = before.rigs.find(rig => rig.rig.id === id);
    const now = after.rigs.find(rig => rig.rig.id === id);
    const name = (now ?? was)!.catalog_name;
    // A rig the activation found locked or unwritable says why, in its words.
    const said = (now?.warnings ?? []).filter(warning => !(was?.warnings ?? []).includes(warning));
    if (said.length > 0) parts.push(`${name}: ${said.join(' ')}`);
    else if (JSON.stringify(was?.push?.ts_file) !== JSON.stringify(now?.push?.ts_file)) parts.push(`${name}'s Target Scheduler connection or planning`);
    else if (rows(was) !== rows(now)) parts.push(`${name}'s rows`);
  }
  return parts.length > 0 ? listed(parts) : 'something the preview cannot show';
}

/** A change's detail without the "(12 of 60 frames taken)" a new frame moves. */
const withoutFrameCounts = (detail: string) => detail.replace(/\([^()]*frames taken\)/g, '');

/** Whether every row activation would write in this rig turns one off. */
const onlyTurnsOff = (rig: DirectorActivationRig) => rig.changes.some(change => change.action === 'disable') && rig.changes.every(change => change.action === 'unchanged' || change.action === 'keep' || change.action === 'disable');

const failure = (error: unknown) => isAxiosError(error) ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'The check failed';

/** What the server needs before it can activate this plan at all (its 422
 *  answer: "Save a framing first."), or null when it can. */
export function notReadyReason(error: unknown): string | null {
  const answer = answerOf(error);
  return answer?.status === 422 ? answer.data?.error || failure(error) : null;
}

/** The server's answer behind a failed request, whether the API client
 *  passed the request's error on or wrapped it. */
export function answerOf(error: unknown): { status: number; data?: { error?: string | null; data?: unknown } } | undefined {
  const cause = isAxiosError(error) ? error : error instanceof Error && isAxiosError(error.cause) ? error.cause : undefined;
  return cause?.response;
}

/** What the rig databases hold against what is saved here: the last
 *  activation, which saved parts (plan, framing) it has not sent yet, which
 *  rigs shooting the saved plan it left out (one skipped with a warning, or
 *  added since), which rigs' rows differ from the plan though the record is
 *  up to date, and whether the rigs could be checked at all. The queries
 *  share their cache keys with the editors and the activation panel, so the
 *  page asks the server once. */
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
  // A record with the saved plan, framing and rigs answers at once; the
  // check runs behind it and speaks up only when it finds rows to write.
  // Without a record, or with one behind, the page waits for the check.
  const recordCurrent = !!last.data && parts.length === 0 && left.length === 0;
  const checking = needsCheck && check.isPending && !recordCurrent;
  const missingIds = matches ? [] : left.filter(id => !holding.has(id));
  const missing = missingIds.map(id => profiles.data?.find(rig => rig.rig.id === id)?.catalog_name ?? 'a rig');
  // Rigs whose rows an activation would change: any row created, taken
  // over, updated or turned off.
  const changed = new Set((check.data?.rigs ?? []).filter(writesRows).map(rig => rig.rig.id));
  // Those the record calls up to date: rigs where activation would only
  // turn plans off are said apart, since that is what to look out for.
  const differingRigs = matches || !last.data || parts.length > 0 ? []
    : (check.data?.rigs ?? []).filter(rig => changed.has(rig.rig.id) && !missingIds.includes(rig.rig.id));
  const differing = differingRigs.filter(rig => !onlyTurnsOff(rig)).map(rig => rig.catalog_name);
  const turningOff = differingRigs.filter(onlyTurnsOff).map(rig => `turns off ${turnedOff(rig.changes)} in ${rig.catalog_name}`);
  // Everything the rigs lack: the saved parts, the rigs left out, then the
  // rigs whose rows differ.
  const behind = matches ? [] : [...parts, ...missing, ...differingRigs.map(rig => rig.catalog_name)];
  // Remote rigs whose site has not had the last activation: the site was
  // offline at Apply, the push failed, or it has not run yet. The rows
  // here can match while the site lacks them.
  const unsent: UnsentPush[] = (last.data?.pushes ?? []).filter(push => !push.applied)
    .map(push => ({ ...push, rig_name: profiles.data?.find(rig => rig.rig.id === push.rig_id)?.catalog_name ?? 'a rig' }));
  const sites = sitesOf(unsent);
  const sentReason = `${listed(sites)} ${sites.length === 1 ? "hasn't" : "haven't"} received this activation`;
  // Why the plan needs activating, said once for the bar, the summary's
  // tooltip and the activation dialog.
  const planReasons = [
    !matches && parts.length > 0 && `saved ${listed(parts)} not activated yet`,
    missing.length > 0 && `${listed(missing)} not activated yet`,
    differing.length > 0 && `${listed(differing)} ${differing.length === 1 ? 'differs' : 'differ'} from the plan`,
    ...turningOff,
  ].filter(Boolean);
  // Only the sites that lack the last activation, for a bar that asks for
  // nothing else.
  const notSent = sentReason;
  const notSynced = !last.data ? 'Not activated yet'
    : planReasons.length === 0 && sites.length > 0 ? notSent
    : `Needs activating: ${[...planReasons, sites.length > 0 && sentReason].filter(Boolean).join('; ')}`;
  // A check that failed, or a rig it could not read (its database locked,
  // unreadable or not on this server, or a row it refuses): said, never
  // asked about, with the reason in `title`.
  const skipped = (check.data?.rigs ?? []).filter(rig => rig.changes.length === 0);
  // The plan cannot be activated yet: the check says what to do first.
  const notReady = check.isError ? notReadyReason(check.error) : null;
  const unchecked = !needsCheck || matches || notReady ? null
    : check.isError ? { text: "Couldn't check the rigs", title: failure(check.error) }
    : skipped.length > 0 ? { text: `Couldn't check ${listed(skipped.map(rig => rig.catalog_name || rig.rig.name))}`, title: skipped.flatMap(rig => rig.warnings).join(' ') }
    : null;
  /** One rig's line on the Rigs tab: activated or not, and why. */
  const rigSync = (rigId: string): { text: string; title?: string } => {
    const unread = skipped.find(rig => rig.rig.id === rigId);
    const report = check.data?.rigs.find(rig => rig.rig.id === rigId);
    // Rows that match here still wait for a site that lacks them.
    const waiting = unsent.find(push => push.rig_id === rigId);
    const synced = waiting ? { text: `behind · not sent to ${waiting.peer_name}`, title: waiting.error } : { text: 'activated' };
    if (holding.has(rigId)) return synced;
    if (unread) return { text: "couldn't check", title: unread.warnings.join(' ') };
    if (!activated.has(rigId)) return { text: 'behind · not activated yet' };
    if (report && changed.has(rigId)) return { text: onlyTurnsOff(report) ? `behind · activation turns off ${turnedOff(report.changes)}` : 'behind · differs from the plan' };
    if (parts.length > 0) return { text: `behind · saved ${listed(parts)} not activated yet` };
    return synced;
  };
  return { last, savedPlan, savedFraming, planRevision, framingRevision, behind, notSynced, notSent, unsent, matches, checking, unchecked, notReady, rigSync };
}

/** Send the last activation again to the remote sites that lack it. The
 *  server records how it went, so the state reads the record again. */
export function useSendAgain(projectId: string) {
  const client = useQueryClient();
  return useMutation({
    retry: false,
    mutationFn: () => apiClient.pushDirectorActivation(projectId),
    onSettled: () => client.invalidateQueries({ queryKey: ['directorActivation', projectId] }),
  });
}

/** What the bar at the top of the page asks for. `activate`: someone may
 *  write, the server writes rig databases, the saved plan shoots something,
 *  and the rigs lack the saved plan, its layout or a rig that shoots it, or
 *  their rows differ from it. `send`: a remote site lacks the last
 *  activation. The save bar comes first, so the page also waits for unsaved
 *  edits. Where the record cannot answer it waits for the check rather than
 *  ask and then take it back; where it can, it asks once the check finds
 *  rows to write. */
export function useActivationBar(projectId: string, canWrite: boolean, unsaved: number): { activate: boolean; send: boolean } {
  const { last, savedPlan, behind, matches, checking, unsent, notReady } = useActivationState(projectId);
  const status = useDirectorStatus();
  const shoots = shootingRigs(savedPlan.data?.plan).length > 0;
  const ready = canWrite && !!status.data?.database_management && unsaved === 0 && last.isSuccess;
  return {
    // Never for a plan the server would refuse: the summary says what to do.
    activate: ready && shoots && (!last.data || behind.length > 0) && !matches && !checking && !notReady,
    send: ready && unsent.length > 0,
  };
}

/** Whether the bar at the top of the page is up, for either reason. */
export function useActivationDue(projectId: string, canWrite: boolean, unsaved: number): boolean {
  const { activate, send } = useActivationBar(projectId, canWrite, unsaved);
  return activate || send;
}
