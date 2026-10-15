import { useEffect, useRef, useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Download, Power, RefreshCw, Save, Send, Telescope, TriangleAlert } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import type { DirectorActivationAction, DirectorActivationChange, DirectorActivationPush, DirectorActivationReport, DirectorActivationShot, DirectorTakenValues } from '../../api/directorTypes';
import { describeFailure, useDrafts } from './pageDraftsState';
import { describeChange, sendLabel, turnedOff, useActivationState, useSendAgain, writesRows } from './activationState';
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

/** What the last preview or send did in one rig's database: a line of
 *  counts, its warnings, what Target Scheduler shoots afterwards, what it
 *  turns off and on, where its rows go, and every other row it touches.
 *  Where a preview would overwrite Target Scheduler's rows, the plan can
 *  take them instead. */
export function RigActivation({ rig, applied, take, objectiveLabel = () => '' }: { rig: ReportRig; applied: boolean; take?: Take; objectiveLabel?: (id: string) => string }) {
  const summary = KINDS
    .filter(({ kind }) => rig.changes.some(change => change.kind === kind))
    .map(({ kind, label }) => `${label}: ${counts(rig.changes, kind)}`);
  const off = turnedOff(rig.changes);
  const on = (rig.shoots ?? []).filter(shot => shot.turned_on).length;
  return <div className="activation-rig" aria-label={`Send to ${rig.catalog_name}`}>
    <p className="activation-rig-summary">
      <strong>{applied ? 'Sent' : 'Preview'}</strong>{rig.applied && <> <Check size={14} aria-label="sent" /></>}
      {off && <strong className="activation-off-count"> · {applied ? 'Turned off' : 'Turns off'} {off}</strong>}
      {on > 0 && <strong> · {applied ? 'Turned on' : 'Turns on'} {on} plan{on === 1 ? '' : 's'}</strong>}
      {summary.length > 0 ? ` · ${summary.join(' · ')}` : ''}
      {rig.push && <span className={rig.push.error ? 'director-error' : 'director-muted'}> · {describePush(rig.push, rig.applied)}</span>}
    </p>
    {rig.warnings.map(warning => <p key={warning} className="director-muted" role="note">{warning}</p>)}
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
    <h4><Telescope size={14} aria-hidden="true" />{applied ? 'Target Scheduler now shoots' : 'Once sent, Target Scheduler shoots'}</h4>
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
  if (push.error) return `Push to ${push.peer_name} failed: ${push.error}`;
  if (push.applied) return `Pushed to ${push.peer_name}`;
  return applied ? `Push to ${push.peer_name} pending` : `Will push to ${push.peer_name}`;
}

/** Send the framing and plan into each participating rig's database. The
 *  preview runs by itself, on opening and whenever what it reads changes,
 *  since it writes nothing; only Send writes. Each rig's part of the result
 *  is shown with that rig (`onReport`); rigs not in `shownElsewhere` are
 *  listed here. */
export default function ActivationPanel({ projectId, onReport, shownElsewhere }: {
  projectId: string;
  onReport?: (report: DirectorActivationReport | null) => void;
  shownElsewhere?: ReadonlySet<string>;
}) {
  // The page's unsaved edits: sending writes what is saved, so the dialog
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
  const save = useMutation({
    retry: false,
    mutationFn: async () => {
      const failed = drafts ? await drafts.saveAll() : null;
      if (failed) throw new Error(`${describeFailure(failed)} Fix it, then try again.`);
    },
  });
  const apply = useMutation({
    retry: false,
    mutationFn: (digest: string) => apiClient.applyDirectorActivation(projectId, digest, visits),
  });
  const takeValues = useMutation({
    retry: false,
    mutationFn: (rigId: string) => apiClient.takeTargetSchedulerValues(projectId, rigId, planRevision ?? 0, framingRevision ?? 0),
  });
  // Everything the preview reads that this page sees change: the saved plan
  // and framing, and the collaboration work chosen here. The rig databases
  // can change too; a refused Send catches that and previews again.
  const inputs = JSON.stringify([planRevision ?? null, framingRevision ?? null, visits]);
  const preview = useAutoPreview<DirectorActivationReport>({
    key: inputs,
    enabled: canWrite && !unsavedPlan && !savedPlan.isPending && !savedFraming.isPending,
    fetch: () => apiClient.previewDirectorActivation(projectId, visits),
  });
  const report = unsavedPlan ? null : preview.result;
  // A Send refused because something changed since the preview: what the
  // preview was, so the new one can say what differs.
  const [refused, setRefused] = useState<DirectorActivationReport | null>(null);
  const [taken, setTaken] = useState<Record<string, DirectorTakenValues>>({});
  const [tookStale, setTookStale] = useState(false);
  const send = () => {
    const shown = preview.result;
    if (!shown || !preview.fresh) return;
    setRefused(null);
    setTookStale(false);
    apply.mutate(shown.preview_digest, {
      onSuccess: applied => {
        preview.replace(applied);
        for (const key of [['directorActivation', projectId], ['directorActivationCheck', projectId], ['collaborationActivation', projectId], ['directorPlanProgress', projectId], ['db'], ['directorCatalog'], ['directorPlans']]) void client.invalidateQueries({ queryKey: key });
      },
      onError: async error => {
        if (httpStatus(error) !== 409) return;
        // Preview again at once; a newer saved plan or framing, once read,
        // changes the inputs and runs it once more.
        setRefused(shown);
        preview.refresh();
        for (const key of [['directorPlan', projectId], ['directorFraming', projectId]]) void client.invalidateQueries({ queryKey: key });
      },
    });
  };
  // The plan takes a rig's Target Scheduler values; the preview then runs
  // again on what is saved now.
  const take = (rigId: string) => {
    setTookStale(false);
    takeValues.mutate(rigId, {
      onSuccess: async result => {
        setTaken(current => ({ ...current, [rigId]: result }));
        await Promise.all([['directorPlan', projectId], ['directorFraming', projectId], ['directorActivationCheck', projectId]]
          .map(key => client.invalidateQueries({ queryKey: key })));
        preview.refresh();
      },
      onError: error => {
        if (httpStatus(error) !== 409) return;
        setTookStale(true);
        preview.refresh();
        for (const key of [['directorPlan', projectId], ['directorFraming', projectId]]) void client.invalidateQueries({ queryKey: key });
      },
    });
  };
  // Sending the last send again to the remote sites that lack it.
  const push = useSendAgain(projectId);
  const pushed = push.data;
  const run = (action: () => void) => { if (busy.current) return; busy.current = true; try { action(); } finally { busy.current = false; } };
  useEffect(() => { onReport?.(report); }, [report, onReport]);
  // Send takes its own button away; the result takes focus in its place.
  const outcome = useRef<HTMLParagraphElement>(null);
  useEffect(() => {
    if (report?.applied && (!document.activeElement || document.activeElement === document.body)) outcome.current?.focus();
  }, [report]);
  const writing = apply.isPending || push.isPending || takeValues.isPending || save.isPending;
  const applyRefused = apply.isError && httpStatus(apply.error) === 409;
  const error = save.error ?? preview.error ?? (applyRefused ? null : apply.error) ?? push.error ?? (tookStale ? null : takeValues.error);
  const nothingToWrite = !!report && report.rigs.every(r => r.warnings.length > 0 && r.changes.length === 0);
  const previewing = canWrite && !unsavedPlan;
  // Said once, politely, for a screen reader: the region is busy until then.
  const announcement = !previewing ? ''
    : preview.loading ? 'Previewing…'
    : preview.error ? 'Preview failed'
    : report?.applied ? 'Sent'
    : preview.fresh ? 'Preview ready' : '';
  return <section className="activation" aria-label="Send to the rigs" aria-busy={preview.loading}>
    <CollaborationActivation projectId={projectId} value={visits} disabled={writing || !canWrite} onChange={setVisits} />
    {last.data && <p className="director-muted">Last sent {new Date(last.data.applied_at_ms).toLocaleString()} · {last.data.rigs.length} rig{last.data.rigs.length === 1 ? '' : 's'}</p>}
    {last.data && (behind.length > 0 || unsent.length > 0) && <p className="activation-behind" role="note">{notSynced}</p>}
    {unsent.length > 0 && <div className="activation-unsent" role="note" aria-label="Not sent to remote sites">
      <ul>{unsent.map(site => <li key={site.rig_id}>
        {site.rig_name}: not sent to {site.peer_name}{site.error && <span className="director-error">: {site.error}</span>}
        {site.attempted_at_ms !== undefined && <span className="director-muted"> · tried {new Date(site.attempted_at_ms).toLocaleString()}</span>}
      </li>)}</ul>
      {canWrite && <button type="button" disabled={writing || !manageable} title={manageable ? 'Send these sites the rows they lack' : 'This server cannot change rig databases'} onClick={() => run(() => push.mutate())}>
        <RefreshCw size={16} />{push.isPending ? 'Sending…' : sendLabel(unsent)}
      </button>}
    </div>}
    {last.data && behind.length === 0 && unsent.length === 0 && !checking && !unsavedPlan && planRevision !== undefined && (unchecked
      ? <p className="director-muted" title={unchecked.title}>{unchecked.text}</p>
      : <p className="director-muted">On the rigs</p>)}
    {!last.data && last.isSuccess && <p className="director-muted">{matches ? 'On the rigs' : notSynced}</p>}
    {canWrite && unsavedPlan && <div className="activation-unsaved" role="note" aria-label="Unsaved changes">
      <p>Unsaved changes in {unsavedLabels.join(', ')}. The rigs get the saved plan, so save first; the preview follows.</p>
      <div className="director-actions">
        <button type="button" disabled={writing} onClick={() => run(() => save.mutate())}><Save size={16} />{save.isPending ? 'Saving…' : 'Save and send…'}</button>
      </div>
    </div>}
    <div className="activation-status">
      <p className="director-muted" role="status">{announcement}</p>
      {previewing && preview.error != null && !preview.loading && <button type="button" onClick={preview.refresh}><RefreshCw size={16} />Refresh</button>}
    </div>
    {error != null && <p className="director-error" role="alert">{message(error)}</p>}
    {refused && <p className="activation-changed" role="note">{preview.fresh && preview.result && preview.result !== refused && !preview.result.applied
      ? `Changed since the preview: ${describeChange(refused, preview.result)}. This preview is new; check it, then send.`
      : 'Changed since the preview; previewing again.'}</p>}
    {tookStale && <p className="activation-changed" role="note">The saved plan changed since this preview, so nothing was taken. The preview has run again.</p>}
    {report && <div className="activation-report">
      <p tabIndex={-1} ref={outcome}><strong>{report.applied ? 'Sent' : 'Preview'}</strong>: framing revision {report.framing_revision}, plan revision {report.plan_revision}, {report.panels} panel{report.panels === 1 ? '' : 's'}.{report.activation_revision !== null && ` Revision ${report.activation_revision} on the rigs.`}</p>
      {report.warnings.map(warning => <p key={warning} className="director-error" role="alert">{warning}</p>)}
      {report.rigs.filter(rig => !shownElsewhere?.has(rig.rig.id)).map(rig => <div key={rig.rig.id}>
        <h4 className="activation-rig-name">{rig.catalog_name}</h4>
        <RigActivation rig={rig} applied={report.applied} objectiveLabel={objectiveLabel} take={canWrite ? {
          run: () => run(() => take(rig.rig.id)),
          pending: takeValues.isPending && takeValues.variables === rig.rig.id,
          result: taken[rig.rig.id],
        } : undefined} />
      </div>)}
    </div>}
    {pushed && <div className="activation-report" aria-label="Push result">
      <p><strong>Sent again</strong>: revision {pushed.activation_revision}.</p>
      {pushed.warnings.map(warning => <p key={warning} className="director-muted">{warning}</p>)}
      <ul>{pushed.rigs.map(rig => <li key={rig.rig.id} className={rig.push.error ? 'director-error' : undefined}>{rig.catalog_name}: {describePush(rig.push, true)}</li>)}</ul>
    </div>}
    {previewing && !report?.applied && <div className="director-actions">
      <button type="button" disabled={!preview.fresh || writing || !manageable || nothingToWrite}
        title={!manageable ? 'This server cannot change rig databases' : !preview.fresh ? 'Waiting for a current preview' : undefined}
        onClick={() => run(send)}><Send size={16} />{apply.isPending ? 'Sending…' : 'Send'}</button>
    </div>}
    {!canWrite && <p className="director-muted">Read only</p>}
    {canWrite && !manageable && <p className="director-muted">Preview only: this server cannot change rig databases</p>}
  </section>;
}
