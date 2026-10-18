import { useEffect, useId, useRef, useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Download, Power, RefreshCw, Save, Telescope, TriangleAlert } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import type { DirectorActivationAction, DirectorActivationChange, DirectorActivationPush, DirectorActivationReport, DirectorActivationShot, DirectorTakenValues } from '../../api/directorTypes';
import { describeFailure, useDrafts } from './pageDraftsState';
import { answerOf, describeChange, notReadyReason, sendLabel, turnedOff, useActivationState, useSendAgain, writesRows } from './activationState';
import { objectiveName } from './planModel';
import './ActivationPanel.css';
import CollaborationActivation from './CollaborationActivation';
import { useAutoPreview } from './useAutoPreview';
import type { CollaborationVisit } from '../../api/collaborationTypes';

const message = (error: unknown) => isAxiosError(error) ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'The request failed';
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;

/** What each action does to a row, in words, most consequential first. */
const ACTIONS: Array<{ action: DirectorActivationAction; count: string; label: string }> = [
  { action: 'create', count: 'new', label: 'New' },
  { action: 'adopt', count: 'taken over', label: 'Taken over' },
  { action: 'update', count: 'updated', label: 'Updated' },
  { action: 'disable', count: 'turned off', label: 'Turned off' },
  { action: 'keep', count: 'left as is', label: 'Left as is' },
  { action: 'unchanged', count: 'unchanged', label: 'Unchanged' },
];

const KINDS: Array<{ kind: DirectorActivationChange['kind']; label: string }> = [
  { kind: 'project', label: 'Project' },
  { kind: 'target', label: 'Targets' },
  { kind: 'template', label: 'Exposure templates' },
  { kind: 'plan', label: 'Exposure plans' },
];

function counts(changes: DirectorActivationChange[], kind: DirectorActivationChange['kind']) {
  const of = changes.filter(c => c.kind === kind);
  const parts = ACTIONS
    .map(({ action, count }) => [of.filter(c => c.action === action).length, count] as const)
    .filter(([n]) => n > 0)
    .map(([n, count]) => `${n} ${count}`);
  return parts.length ? parts.join(', ') : 'none';
}

type ReportRig = DirectorActivationReport['rigs'][number];

/** Whether a preview would change rows Target Scheduler already holds for
 *  this rig: its project is there, and some row is more than unchanged. */
function differsFromTargetScheduler(rig: ReportRig): boolean {
  return rig.changes.some(change => change.kind === 'project' && change.action !== 'create') && writesRows(rig);
}

/** Taking a rig's Target Scheduler values: the action, and what it did. */
interface Take {
  run: () => void;
  pending: boolean;
  result?: DirectorTakenValues;
}

/** What the last preview or apply did in one rig's database: a line of
 *  counts, its warnings, what Target Scheduler shoots afterwards, what it
 *  turns off and on, where its rows go, and every other row it touches.
 *  Where a preview would overwrite Target Scheduler's rows, the plan can
 *  take them instead. */
export function RigActivation({ rig, applied, take, objectiveLabel = () => '', showDeliveryError = true }: { rig: ReportRig; applied: boolean; take?: Take; objectiveLabel?: (id: string) => string; showDeliveryError?: boolean }) {
  const summary = KINDS
    .filter(({ kind }) => rig.changes.some(change => change.kind === kind))
    .map(({ kind, label }) => `${label}: ${counts(rig.changes, kind)}`);
  const off = turnedOff(rig.changes);
  const on = (rig.shoots ?? []).filter(shot => shot.turned_on).length;
  return <div className="activation-rig" aria-label={`${rig.catalog_name} activation`}>
    <p className="activation-rig-summary">
      <strong>{applied ? rig.push?.ts_file && !rig.push.applied ? 'Activated locally' : 'Activated' : 'Preview'}</strong>{rig.applied && <> <Check size={14} aria-label="activated" /></>}
      {off && <strong className="activation-off-count"> · {applied ? 'Turned off' : 'Turns off'} {off}</strong>}
      {on > 0 && <strong> · {applied ? 'Turned on' : 'Turns on'} {on} plan{on === 1 ? '' : 's'}</strong>}
      {summary.length > 0 ? ` · ${summary.join(' · ')}` : ''}
      {rig.push && <span className={rig.push.error ? 'director-error' : 'director-muted'}> · {describePush(rig.push, rig.applied)}</span>}
    </p>
    {rig.warnings.map(warning => <p key={warning} className="director-muted" role="note">{warning}</p>)}
    {showDeliveryError && rig.push?.ts_file && rig.push.error && <DeliveryError error={rig.push.error} />}
    {rig.push?.ts_file && <div role="note" aria-label="Target Scheduler planning delivery">
      <p>Planning destination: <code>{rig.push.ts_file.path}</code></p>
      {!applied && <p>Activate sends this project's planning through the catalog's saved TS connection. Captures, grades, and acquisition counts stay in TS.</p>}
      {!applied && Object.keys(rig.push.summary).length > 0 && <ul>{[
        ['project', 'Projects'], ['target', 'Targets'], ['exposuretemplate', 'Exposure templates'], ['exposureplan', 'Exposure plans'], ['ruleweight', 'Rule weights'],
      ].map(([key, label]) => <li key={key}>{label}: {rig.push!.summary[`${key}_inserted`] ?? 0} new, {rig.push!.summary[`${key}_updated`] ?? 0} updated</li>)}</ul>}
    </div>}
    <Shoots rig={rig} applied={applied} objectiveLabel={objectiveLabel} />
    <TurnedOff rig={rig} applied={applied} />
    <TurnedOn rig={rig} applied={applied} />
    <RigChanges rig={rig} />
    {take && !applied && differsFromTargetScheduler(rig) && <div className="director-actions">
      <button type="button" disabled={take.pending} title={`Write ${rig.catalog_name}'s Target Scheduler values into the plan instead`} onClick={take.run}>
        <Download size={16} />{take.pending ? 'Taking…' : "Take Target Scheduler's values"}
      </button>
    </div>}
    {take?.result && <div className="activation-taken" aria-label={`Taken from ${rig.catalog_name}`}>
      {take.result.taken.length === 0 && <p className="director-muted">Nothing to take: the plan already has its values</p>}
      {take.result.taken.length > 0 && <ul>{take.result.taken.map(line => <li key={line}>{line}</li>)}</ul>}
      {take.result.left.map(line => <p key={line} className="director-muted" role="note">{line}</p>)}
    </div>}
  </div>;
}

/** A plan's template in a few words: its name, then the filter, gain and
 *  offset where set. */
function templateText(shot: DirectorActivationShot): string {
  const settings = [shot.filter, shot.gain !== null && `gain ${shot.gain}`, shot.offset !== null && `offset ${shot.offset}`].filter(Boolean);
  return settings.length > 0 ? `${shot.template} (${settings.join(', ')})` : shot.template;
}

/** The end result: every plan Target Scheduler shoots in the rig's project
 *  after the activation, with its frames, and which objectives a summed one
 *  serves. */
function Shoots({ rig, applied, objectiveLabel }: { rig: ReportRig; applied: boolean; objectiveLabel: (id: string) => string }) {
  if (!rig.shoots) return null;
  return <div className="activation-shoots" role="note" aria-label={`Shoots in ${rig.catalog_name}`}>
    <h4><Telescope size={14} aria-hidden="true" />{rig.push?.ts_file && !rig.push.applied
      ? applied ? 'Planned locally; not sent to Target Scheduler' : 'After delivery, Target Scheduler shoots'
      : applied ? 'Target Scheduler now shoots' : 'After activation, Target Scheduler shoots'}</h4>
    {rig.shoots.length === 0 ? <p className="director-muted">Nothing in this project</p> : <ul>{rig.shoots.map(shot => {
      const summed = shot.objectives.length > 1 ? shot.objectives.map(objectiveLabel).filter(Boolean) : [];
      return <li key={shot.plan}>
        <strong>{shot.target} · {shot.exposure_seconds} s</strong> {templateText(shot)} · {shot.desired} frames, {shot.acquired} taken
        {shot.objectives.length > 1 && <span className="director-muted"> · summed from {summed.length === shot.objectives.length ? listed(summed) : `${shot.objectives.length} objectives`}</span>}
      </li>;
    })}</ul>}
  </div>;
}

/** Plans that were off in Target Scheduler and come back on. */
function TurnedOn({ rig, applied }: { rig: ReportRig; applied: boolean }) {
  const on = (rig.shoots ?? []).filter(shot => shot.turned_on);
  if (on.length === 0) return null;
  return <div className="activation-on" role="note" aria-label={`Turned on in ${rig.catalog_name}`}>
    <h4><Power size={14} aria-hidden="true" />{applied ? 'Turned on' : 'Turns on'}</h4>
    <ul>{on.map(shot => <li key={shot.plan}><strong>{shot.target} · {shot.exposure_seconds} s</strong> {shot.template}, plan #{shot.plan}, {shot.acquired} of {shot.desired} frames taken</li>)}</ul>
  </div>;
}

const listed = (words: string[]) => words.length <= 1 ? words.join('') : `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;

/** What activation turns off in Target Scheduler, on its own and open
 *  above the rest, so it is never missed: plans the plan does not have, and
 *  a project whose rig is off. */
function TurnedOff({ rig, applied }: { rig: ReportRig; applied: boolean }) {
  const off = rig.changes.filter(change => change.action === 'disable');
  if (off.length === 0) return null;
  return <div className="activation-off" role="note" aria-label={`Turned off in ${rig.catalog_name}`}>
    <h4><TriangleAlert size={14} aria-hidden="true" />{applied ? 'Turned off' : 'Turns off'}</h4>
    <ul>{off.map((change, index) => <li key={`${change.name}-${index}`}><strong>{change.name}</strong>{change.detail && <span> {change.detail}</span>}</li>)}</ul>
    {!applied && off.some(change => change.kind === 'plan') && <p>To keep one, take Target Scheduler's values or add it to the plan.</p>}
  </div>;
}

/** Every other row activation touches in one rig's database, by kind,
 *  saying what happens to it and which existing row it lands on. */
function RigChanges({ rig }: { rig: ReportRig }) {
  const changes = rig.changes.filter(change => change.action !== 'disable');
  if (changes.length === 0) return null;
  return <details className="activation-changes" open={writesRows({ ...rig, changes })}>
    <summary>What changes in {rig.catalog_name}</summary>
    {KINDS.map(({ kind, label }) => {
      const of = changes.filter(change => change.kind === kind);
      if (of.length === 0) return null;
      const order = (action: DirectorActivationAction) => ACTIONS.findIndex(entry => entry.action === action);
      return <div key={kind} className="activation-changes-kind">
        <h4>{label}</h4>
        <ul>
          {[...of].sort((left, right) => order(left.action) - order(right.action)).map((change, index) => <li key={`${change.name}-${index}`}>
            <span className={`activation-action is-${change.action}`}>{ACTIONS.find(entry => entry.action === change.action)?.label ?? change.action}</span>
            <strong>{change.name}</strong>
            {change.detail && <span className="director-muted"> {change.detail}</span>}
          </li>)}
        </ul>
      </div>;
    })}
  </details>;
}

/** One line on where a rig's rows go after this server: nowhere else, or a peer, and how that went. */
function describePush(push: DirectorActivationPush | null, applied: boolean): string {
  if (!push) return 'This server';
  if (push.error && push.ts_file) return applied ? 'TS delivery pending' : 'TS delivery unavailable; activation will be local';
  if (push.error) return `Push to ${push.peer_name} failed: ${push.error}`;
  if (push.applied) return `Pushed to ${push.peer_name}`;
  return applied ? `Push to ${push.peer_name} pending` : `Will push to ${push.peer_name}`;
}

function DeliveryError({ error }: { error: string }) {
  return <details className="activation-delivery-error">
    <summary>Error details</summary>
    <pre>{error}</pre>
  </details>;
}

/** Activate the framing and plan in each participating rig's database. The
 *  preview runs by itself, on opening and whenever what it reads changes:
 *  the server works it out on a copy of each rig's planning tables, so it
 *  writes nothing and takes no lock. Activate runs it again on the databases
 *  themselves and writes only when that matches. Each rig's part of the
 *  result is shown with that rig (`onReport`); rigs not in `shownElsewhere`
 *  are listed here. `onFraming` opens the Framing tab, for a plan that
 *  needs a framing before it can be activated. */
export default function ActivationPanel({ projectId, onReport, shownElsewhere, onFraming }: {
  projectId: string;
  onReport?: (report: DirectorActivationReport | null) => void;
  shownElsewhere?: ReadonlySet<string>;
  onFraming?: () => void;
}) {
  // The page's unsaved edits: activation writes what is saved, so the dialog
  // asks to save them first rather than preview a plan about to change.
  // Saving is a change of its own, so it never happens unasked.
  const drafts = useDrafts();
  const unsavedLabels = drafts?.unsaved.map(section => section.label) ?? [];
  const unsavedPlan = unsavedLabels.length > 0;
  const { canWrite } = useAccess();
  const status = useDirectorStatus();
  const manageable = status.data?.database_management ?? true;
  const client = useQueryClient();
  const { last, savedPlan, savedFraming, planRevision, framingRevision, behind, notSynced, unsent, matches, checking, unchecked } = useActivationState(projectId);
  const objectiveLabel = (id: string) => {
    const objective = savedPlan.data?.plan?.objectives.find(entry => entry.id === id);
    return objective ? objectiveName(objective) : '';
  };
  const [visits, setVisits] = useState<CollaborationVisit[]>([]);
  const busy = useRef(false);
  const ids = useId();
  const invalidate = (keys: unknown[][]) => Promise.all(keys.map(queryKey => client.invalidateQueries({ queryKey })));
  const save = useMutation({
    retry: false,
    mutationFn: async () => {
      const failed = drafts ? await drafts.saveAll() : null;
      if (failed) throw new Error(`${describeFailure(failed)} Fix it, then try again.`);
    },
  });
  // What the page reads about the rigs changes with an activation, whether or not
  // the dialog is still open when it lands, so these refresh here and not
  // in a callback that closing the dialog would drop.
  const apply = useMutation({
    retry: false,
    mutationFn: (digest: string) => apiClient.applyDirectorActivation(projectId, digest, visits),
    onSuccess: () => invalidate([['directorActivation', projectId], ['directorActivationCheck', projectId], ['collaborationActivation', projectId], ['directorPlanProgress', projectId], ['db'], ['directorCatalog'], ['directorPlans']]),
    // A refusal may be a plan or framing saved elsewhere: read them again
    // before the dialog previews again.
    onError: async error => { if (httpStatus(error) === 409) await invalidate([['directorPlan', projectId], ['directorFraming', projectId], ['directorActivationCheck', projectId]]); },
  });
  const takeValues = useMutation({
    retry: false,
    mutationFn: (rigId: string) => apiClient.takeTargetSchedulerValues(projectId, rigId, planRevision ?? 0, framingRevision ?? 0),
    onSuccess: () => invalidate([['directorPlan', projectId], ['directorFraming', projectId], ['directorActivationCheck', projectId]]),
    onError: async error => { if (httpStatus(error) === 409) await invalidate([['directorPlan', projectId], ['directorFraming', projectId]]); },
  });
  // Everything the preview reads that this page sees change: the saved plan
  // and framing, and the collaboration work chosen here. The rig databases
  // can change too; Activate catches that and previews again.
  const inputs = JSON.stringify([planRevision ?? null, framingRevision ?? null, visits]);
  const preview = useAutoPreview<DirectorActivationReport>({
    key: inputs,
    enabled: canWrite && !unsavedPlan && !savedPlan.isPending && !savedFraming.isPending,
    // A metadata store busy with a save answers 503: wait a moment, once.
    fetch: () => apiClient.previewDirectorActivation(projectId, visits).catch(async (error: unknown) => {
      if (httpStatus(error) !== 503) throw error;
      await new Promise(resolve => window.setTimeout(resolve, 700));
      return apiClient.previewDirectorActivation(projectId, visits);
    }),
  });
  const report = unsavedPlan ? null : preview.result;
  // The server cannot activate this plan yet, and says what to do first.
  const notReady = notReadyReason(preview.error);
  // An activation refused because something changed since the preview:
  // the preview it was refused for, and what Activate found on the rigs.
  const [refused, setRefused] = useState<{ shown: DirectorActivationReport; found: DirectorActivationReport | null } | null>(null);
  // Until a new preview lands, Activate stays off.
  const reconciling = !!refused && preview.result === refused.shown;
  const [taken, setTaken] = useState<Record<string, DirectorTakenValues>>({});
  const [tookStale, setTookStale] = useState(false);
  // Preview once more after a refusal or a take, once the page holds the
  // saved plan and framing just read: a newer revision changes the inputs,
  // and this one run serves for that change too.
  const [again, setAgain] = useState(0);
  useEffect(() => {
    if (!again) return;
    const plan = client.getQueryData<{ plan?: { revision: number } | null }>(['directorPlan', projectId])?.plan?.revision;
    const framing = client.getQueryData<{ draft?: { revision: number } | null }>(['directorFraming', projectId])?.draft?.revision;
    if (plan !== planRevision || framing !== framingRevision) return;
    setAgain(0);
    preview.refresh();
  }, [again, client, projectId, planRevision, framingRevision, preview]);
  const send = () => {
    const shown = preview.result;
    if (!shown || !preview.fresh) return;
    setRefused(null);
    setTookStale(false);
    apply.mutate(shown.preview_digest, {
      onSuccess: applied => preview.replace(applied),
      onError: error => {
        if (httpStatus(error) !== 409) return;
        // What Activate found on the rig databases themselves.
        const found = (answerOf(error)?.data?.data as DirectorActivationReport | null | undefined) ?? null;
        setRefused({ shown, found });
        setAgain(count => count + 1);
      },
    });
  };
  // The plan takes a rig's Target Scheduler values; the preview then runs
  // again on what is saved now.
  const take = (rigId: string) => {
    setTookStale(false);
    takeValues.mutate(rigId, {
      onSuccess: result => {
        setTaken(current => ({ ...current, [rigId]: result }));
        setAgain(count => count + 1);
      },
      onError: error => { if (httpStatus(error) === 409) { setTookStale(true); setAgain(count => count + 1); } },
    });
  };
  // Sending the last activation again to the remote sites that lack it.
  const push = useSendAgain(projectId);
  const pushed = push.data;
  useEffect(() => {
    const shown = preview.result;
    if (!shown?.applied || shown.activation_revision !== last.data?.revision) return;
    let changed = false;
    const rigs = shown.rigs.map(rig => {
      const receipt = last.data?.pushes?.find(receipt => receipt.rig_id === rig.rig.id && receipt.peer_id === rig.push?.peer_id);
      if (!rig.push?.ts_file || !receipt || receipt.applied === rig.push.applied && (receipt.error ?? null) === rig.push.error) return rig;
      changed = true;
      return { ...rig, push: { ...rig.push, applied: receipt.applied, error: receipt.error ?? null } };
    });
    if (changed) preview.replace({ ...shown, rigs });
  }, [last.data, preview]);
  const run = (action: () => void) => { if (busy.current) return; busy.current = true; try { action(); } finally { busy.current = false; } };
  useEffect(() => { onReport?.(report); }, [report, onReport]);
  // Activate takes its own button away; the result takes focus in its place.
  const outcome = useRef<HTMLParagraphElement>(null);
  useEffect(() => {
    if (report?.applied && (!document.activeElement || document.activeElement === document.body)) outcome.current?.focus();
  }, [report]);
  // Save and activate… goes with the note it sits in; the status line, which
  // says the preview is on its way, takes focus so it stays in the dialog.
  const statusLine = useRef<HTMLParagraphElement>(null);
  const wasUnsaved = useRef(unsavedPlan);
  useEffect(() => {
    if (wasUnsaved.current && !unsavedPlan && save.isSuccess) statusLine.current?.focus();
    wasUnsaved.current = unsavedPlan;
  }, [unsavedPlan, save.isSuccess]);
  const writing = apply.isPending || push.isPending || takeValues.isPending || save.isPending;
  const fileDeliveryPending = !!report?.rigs.some(rig => rig.push?.ts_file && !rig.push.applied);
  const applyRefused = apply.isError && httpStatus(apply.error) === 409;
  const previewFailed = notReady ? null : preview.error;
  const error = save.error ?? previewFailed ?? (applyRefused ? null : apply.error) ?? push.error ?? (tookStale ? null : takeValues.error);
  const nothingToWrite = !!report && report.rigs.every(r => r.warnings.length > 0 && r.changes.length === 0);
  const previewing = canWrite && !unsavedPlan;
  // Said once, politely, for a screen reader. It sits outside the busy
  // preview, so "Previewing…" is said too.
  const announcement = !previewing ? ''
    : preview.loading ? 'Previewing…'
    : notReady ? `Not ready to activate: ${notReady}`
    : preview.error ? 'Preview failed'
    : report?.applied ? fileDeliveryPending ? 'Activated locally; TS delivery pending' : 'Activated'
    : preview.fresh ? 'Preview ready' : '';
  // Why Activate is off, said beside it and read with it.
  const sendWait = !manageable ? 'This server cannot change rig databases.'
    : notReady ? 'Nothing to activate until then.'
    : preview.loading || reconciling ? 'Waiting for the preview.'
    : preview.error ? 'No preview to activate.'
    : nothingToWrite ? 'No rig database can take this plan yet.'
    : '';
  const changed = refused && (refused.found ?? (!reconciling ? preview.result : null));
  return <section className="activation" aria-label="Activation">
    <CollaborationActivation projectId={projectId} value={visits} disabled={writing || !canWrite} onChange={setVisits} />
    {last.data && <p className="director-muted">Last activated {new Date(last.data.applied_at_ms).toLocaleString()} · {last.data.rigs.length} rig{last.data.rigs.length === 1 ? '' : 's'}</p>}
    {last.data && (behind.length > 0 || unsent.length > 0) && <p className="activation-behind" role="note">{notSynced}</p>}
    {unsent.length > 0 && <div className="activation-unsent" role="note" aria-label={unsent.some(site => site.peer_id.startsWith('ts:')) ? 'Not sent to Target Scheduler' : 'Not sent to remote sites'}>
      {unsent.some(site => site.peer_id.startsWith('ts:')) && <p className="activation-unsent-help">The plan is saved locally. Retry delivery, or choose Review TS delivery if TS planning changed.</p>}
      <ul>{unsent.map(site => <li key={site.rig_id}>
        {site.rig_name}: not sent to {site.peer_name}{site.error && !site.peer_id.startsWith('ts:') && <span className="director-error">: {site.error}</span>}
        {site.attempted_at_ms !== undefined && <span className="director-muted"> · tried {new Date(site.attempted_at_ms).toLocaleString()}</span>}
        {site.error && site.peer_id.startsWith('ts:') && <DeliveryError error={site.error} />}
      </li>)}</ul>
      {canWrite && <button type="button" disabled={writing || !manageable} title={manageable ? 'Send the last activation\'s rows to these sites again' : 'This server cannot change rig databases'} onClick={() => run(() => push.mutate())}>
        <RefreshCw size={16} />{push.isPending ? 'Sending…' : sendLabel(unsent)}
      </button>}
    </div>}
    {/* Once activated here, the status line says so; the state need not too. */}
    {last.data && behind.length === 0 && unsent.length === 0 && !checking && !unsavedPlan && !report?.applied && planRevision !== undefined && (unchecked
      ? <p className="director-muted" title={unchecked.title}>{unchecked.text}</p>
      : <p className="director-muted">Activated</p>)}
    {!last.data && last.isSuccess && !notReady && <p className="director-muted">{matches ? 'Activated' : notSynced}</p>}
    {canWrite && unsavedPlan && <div className="activation-unsaved" role="note" aria-label="Unsaved changes">
      <p>Unsaved changes in {unsavedLabels.join(', ')}. Save your changes before activating. PSF Guard then updates the preview.</p>
      <div className="director-actions">
        <button type="button" disabled={writing} onClick={() => run(() => save.mutate())}><Save size={16} />{save.isPending ? 'Saving…' : 'Save and activate…'}</button>
      </div>
    </div>}
    <div className="activation-status">
      <p className={notReady ? 'activation-not-ready' : 'director-muted'} role="status" tabIndex={-1} ref={statusLine}>{announcement}</p>
      {previewing && notReady && onFraming && /Framing/.test(notReady) && <button type="button" onClick={onFraming}>Open Framing</button>}
      {previewing && previewFailed != null && !preview.loading && <button type="button" onClick={preview.refresh}><RefreshCw size={16} />Refresh</button>}
    </div>
    {error != null && <p className="director-error" role="alert">{message(error)}</p>}
    {refused && <p className="activation-changed" role="alert">{changed
      ? `Changed since the preview: ${describeChange(refused.shown, changed).replace(/\.$/, '')}. Nothing was written. `
      : 'Changed since the preview. Nothing was written. '}
      {reconciling ? 'Previewing again…' : 'This preview is new; check it, then activate.'}</p>}
    {tookStale && <p className="activation-changed" role="alert">The saved plan changed since this preview, so nothing was taken. The preview has run again.</p>}
    {report && <div className="activation-report" role="region" aria-label="Preview" aria-busy={preview.loading || reconciling}>
      <p tabIndex={-1} ref={outcome}><strong>{report.applied ? fileDeliveryPending ? 'Activated locally; TS delivery pending' : 'Activated' : 'Preview'}</strong>: framing revision {report.framing_revision}, plan revision {report.plan_revision}, {report.panels} panel{report.panels === 1 ? '' : 's'}.{report.activation_revision !== null && ` Revision ${report.activation_revision} on the rigs.`}</p>
      {report.warnings.map(warning => <p key={warning} className="director-error" role="alert">{warning}</p>)}
      {report.rigs.filter(rig => !shownElsewhere?.has(rig.rig.id)).map(rig => <div key={rig.rig.id}>
        <h4 className="activation-rig-name">{rig.catalog_name}</h4>
        <RigActivation rig={rig} applied={report.applied} objectiveLabel={objectiveLabel}
          showDeliveryError={!unsent.some(site => site.rig_id === rig.rig.id && !!site.error)} take={canWrite ? {
          run: () => run(() => take(rig.rig.id)),
          pending: takeValues.isPending && takeValues.variables === rig.rig.id,
          result: taken[rig.rig.id],
        } : undefined} />
      </div>)}
    </div>}
    {pushed && <div className="activation-report" aria-label="Push result">
      <p><strong>Sent again</strong>: activation revision {pushed.activation_revision}.</p>
      {pushed.warnings.map(warning => <p key={warning} className="director-muted">{warning}</p>)}
      <ul>{pushed.rigs.map(rig => <li key={rig.rig.id} className={rig.push.error ? 'director-error' : undefined}>{rig.catalog_name}: {describePush(rig.push, true)}</li>)}</ul>
    </div>}
    {canWrite && report?.applied && fileDeliveryPending && <button type="button" disabled={writing} onClick={preview.refresh}>
      <RefreshCw size={16} />Review TS delivery
    </button>}
    {previewing && !report?.applied && <div className="director-actions">
      <button type="button" disabled={!preview.fresh || reconciling || writing || !!sendWait}
        aria-describedby={sendWait ? `${ids}-activate-wait` : undefined}
        onClick={() => run(send)}><Check size={16} />{apply.isPending ? 'Activating…' : 'Activate'}</button>
      {sendWait && <span id={`${ids}-activate-wait`} className="director-muted">{sendWait}</span>}
    </div>}
    {!canWrite && <p className="director-muted">Read only</p>}
    {canWrite && !manageable && <p className="director-muted">Preview only: this server cannot change rig databases</p>}
  </section>;
}
