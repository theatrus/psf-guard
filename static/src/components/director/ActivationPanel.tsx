import { useEffect, useRef, useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Download, Eye, Send } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import type { DirectorActivationAction, DirectorActivationChange, DirectorActivationPush, DirectorActivationPushReport, DirectorActivationReport, DirectorTakenValues } from '../../api/directorTypes';
import { describeFailure, useDrafts } from './pageDraftsState';
import { useActivationState } from './activationState';
import './ActivationPanel.css';
import CollaborationActivation from './CollaborationActivation';
import type { CollaborationVisit } from '../../api/collaborationTypes';

const message = (error: unknown) => isAxiosError(error) ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'Activation request failed';
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
  return rig.changes.some(change => change.kind === 'project' && change.action !== 'create')
    && rig.changes.some(change => change.action !== 'unchanged' && change.action !== 'keep');
}

/** Taking a rig's Target Scheduler values: the action, and what it did. */
interface Take {
  run: () => void;
  pending: boolean;
  result?: DirectorTakenValues;
}

/** What the last preview or apply did in one rig's database: a line of
 *  counts, its warnings, where its rows go, and every row it touches. Where
 *  a preview would overwrite Target Scheduler's rows, the plan can take
 *  them instead. */
export function RigActivation({ rig, applied, take }: { rig: ReportRig; applied: boolean; take?: Take }) {
  const summary = KINDS
    .filter(({ kind }) => rig.changes.some(change => change.kind === kind))
    .map(({ kind, label }) => `${label}: ${counts(rig.changes, kind)}`);
  return <div className="activation-rig" aria-label={`${rig.catalog_name} activation`}>
    <p className="activation-rig-summary">
      <strong>{applied ? 'Applied' : 'Preview'}</strong>{rig.applied && <> <Check size={14} aria-label="applied" /></>}
      {summary.length > 0 ? ` · ${summary.join(' · ')}` : ''}
      {rig.push && <span className={rig.push.error ? 'director-error' : 'director-muted'}> · {describePush(rig.push, rig.applied)}</span>}
    </p>
    {rig.warnings.map(warning => <p key={warning} className="director-muted" role="note">{warning}</p>)}
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

/** Every row activation touches in one rig's database, by kind, saying what
 *  happens to it and which existing row it lands on. */
function RigChanges({ rig }: { rig: ReportRig }) {
  if (rig.changes.length === 0) return null;
  const busy = rig.changes.some(change => change.action !== 'unchanged' && change.action !== 'keep');
  return <details className="activation-changes" open={busy}>
    <summary>What changes in {rig.catalog_name}</summary>
    {KINDS.map(({ kind, label }) => {
      const of = rig.changes.filter(change => change.kind === kind);
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

/** Push the framing and plan into each participating rig's database, with a
 *  preview first. Each rig's part of the result is shown with that rig
 *  (`onReport`); rigs not in `shownElsewhere` are listed here. */
export default function ActivationPanel({ projectId, onReport, shownElsewhere }: {
  projectId: string;
  onReport?: (report: DirectorActivationReport | null) => void;
  shownElsewhere?: ReadonlySet<string>;
}) {
  // The page's unsaved edits: activation reads what is saved, so it saves
  // them first and never offers Apply over a preview they have outdated.
  const drafts = useDrafts();
  const unsavedLabels = drafts?.unsaved.map(section => section.label) ?? [];
  const unsavedPlan = unsavedLabels.length > 0;
  const { canWrite } = useAccess();
  const status = useDirectorStatus();
  const manageable = status.data?.database_management ?? true;
  const client = useQueryClient();
  const { last, planRevision, framingRevision, behind, behindText, matches } = useActivationState(projectId);
  const [report, setReport] = useState<DirectorActivationReport | null>(null);
  const [visits, setVisits] = useState<CollaborationVisit[]>([]);
  const busy = useRef(false);
  // Activation writes the saved plan, so edits still in the editor are saved
  // first; otherwise the rig databases would get the plan as it was before.
  const preview = useMutation({
    retry: false,
    mutationFn: async () => {
      const failed = drafts ? await drafts.saveAll() : null;
      if (failed) throw new Error(`${describeFailure(failed)} Fix it, then preview again.`);
      return apiClient.previewDirectorActivation(projectId, visits);
    },
    onSuccess: setReport,
  });
  const apply = useMutation({
    retry: false,
    mutationFn: () => { if (!report) throw new Error('Preview first'); return apiClient.applyDirectorActivation(projectId, report.preview_digest, visits); },
    onSuccess: applied => { setReport(applied); for (const key of [['directorActivation', projectId], ['directorActivationCheck', projectId], ['collaborationActivation', projectId], ['directorPlanProgress', projectId], ['db'], ['directorCatalog'], ['directorPlans']]) void client.invalidateQueries({ queryKey: key }); },
    onError: error => { if (httpStatus(error) === 409) setReport(null); },
  });
  // The plan takes a rig's Target Scheduler values; then the preview runs
  // again on what is saved now.
  const [taken, setTaken] = useState<Record<string, DirectorTakenValues>>({});
  const takeValues = useMutation({
    retry: false,
    mutationFn: (rigId: string) => apiClient.takeTargetSchedulerValues(projectId, rigId, planRevision ?? 0, framingRevision ?? 0),
    onSuccess: async (result, rigId) => {
      setTaken(current => ({ ...current, [rigId]: result }));
      await Promise.all([['directorPlan', projectId], ['directorFraming', projectId], ['directorActivationCheck', projectId]]
        .map(key => client.invalidateQueries({ queryKey: key })));
      preview.mutate();
    },
  });
  const [pushed, setPushed] = useState<DirectorActivationPushReport | null>(null);
  const push = useMutation({ retry: false, mutationFn: () => apiClient.pushDirectorActivation(projectId), onSuccess: setPushed });
  const run = (action: () => void) => { if (busy.current) return; busy.current = true; try { action(); } finally { busy.current = false; } };
  useEffect(() => { onReport?.(report); }, [report, onReport]);
  // Apply takes its own button away; the result takes focus in its place.
  const outcome = useRef<HTMLParagraphElement>(null);
  useEffect(() => {
    if (report?.applied && (!document.activeElement || document.activeElement === document.body)) outcome.current?.focus();
  }, [report]);
  const pending = preview.isPending || apply.isPending || push.isPending || takeValues.isPending;
  const tookStale = takeValues.isError && httpStatus(takeValues.error) === 409;
  const error = preview.error ?? apply.error ?? push.error ?? (tookStale ? null : takeValues.error);
  return <section className="activation" aria-label="Activation">
    <CollaborationActivation projectId={projectId} value={visits} disabled={pending || !canWrite} onChange={next => { setVisits(next); setReport(null); }} />
    {last.data && <p className="director-muted">Last activated {new Date(last.data.applied_at_ms).toLocaleString()} · {last.data.rigs.length} rig{last.data.rigs.length === 1 ? '' : 's'}</p>}
    {last.data && behind.length > 0 && <p className="activation-behind" role="note">{behindText}</p>}
    {last.data && behind.length === 0 && !unsavedPlan && planRevision !== undefined && <p className="director-muted">Rigs are up to date</p>}
    {!last.data && last.isSuccess && <p className="director-muted">{matches ? 'The rigs already match the saved plan' : 'Not activated yet'}</p>}
    {error && !(apply.isError && httpStatus(apply.error) === 409) && <p className="director-error" role="alert">{message(error)}</p>}
    {((apply.isError && httpStatus(apply.error) === 409) || tookStale) && <p className="director-error" role="alert">Changed since the preview. Preview again.</p>}
    {report && <div className="activation-report">
      <p tabIndex={-1} ref={outcome}><strong>{report.applied ? 'Applied' : 'Preview'}</strong>: framing revision {report.framing_revision}, plan revision {report.plan_revision}, {report.panels} panel{report.panels === 1 ? '' : 's'}.{report.activation_revision !== null && ` Activation revision ${report.activation_revision}.`}</p>
      {report.warnings.map(warning => <p key={warning} className="director-error" role="alert">{warning}</p>)}
      {report.rigs.filter(rig => !shownElsewhere?.has(rig.rig.id)).map(rig => <div key={rig.rig.id}>
        <h4 className="activation-rig-name">{rig.catalog_name}</h4>
        <RigActivation rig={rig} applied={report.applied} take={canWrite ? {
          run: () => run(() => takeValues.mutate(rig.rig.id)),
          pending: takeValues.isPending && takeValues.variables === rig.rig.id,
          result: taken[rig.rig.id],
        } : undefined} />
      </div>)}
    </div>}
    {pushed && <div className="activation-report" aria-label="Push result">
      <p><strong>Pushed again</strong>: activation revision {pushed.activation_revision}.</p>
      {pushed.warnings.map(warning => <p key={warning} className="director-muted">{warning}</p>)}
      <ul>{pushed.rigs.map(rig => <li key={rig.rig.id} className={rig.push.error ? 'director-error' : undefined}>{rig.catalog_name}: {describePush(rig.push, true)}</li>)}</ul>
    </div>}
    {canWrite && <div className="director-actions">
      <button type="button" disabled={pending} onClick={() => run(() => preview.mutate())}><Eye size={16} />{preview.isPending ? 'Previewing...' : unsavedPlan ? 'Save and preview' : report && !report.applied ? 'Preview again' : 'Preview'}</button>
      {report && !report.applied && !unsavedPlan && <button type="button" disabled={pending || !manageable || report.rigs.every(r => r.warnings.length > 0 && r.changes.length === 0)} title={manageable ? undefined : 'This server cannot change rig databases'} onClick={() => run(() => apply.mutate())}><Check size={16} />{apply.isPending ? 'Applying...' : 'Apply'}</button>}
      {last.data && <button type="button" disabled={pending || !manageable} title={manageable ? 'Send the last activation\'s rows to each remote rig\'s peer again' : 'This server cannot change rig databases'} onClick={() => run(() => push.mutate())}><Send size={16} />{push.isPending ? 'Pushing...' : 'Push again'}</button>}
    </div>}
    {canWrite && unsavedPlan && <p className="director-muted" role="note">Unsaved changes in {unsavedLabels.join(', ')}; preview saves them{report && !report.applied ? ' (this preview predates them)' : ''}</p>}
    {!canWrite && <p className="director-muted">Read only</p>}
    {canWrite && !manageable && <p className="director-muted">Preview only: this server cannot change rig databases</p>}
  </section>;
}
