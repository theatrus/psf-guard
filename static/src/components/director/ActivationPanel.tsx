import { useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Eye, Send } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import type { DirectorActivationAction, DirectorActivationChange, DirectorActivationPush, DirectorActivationPushReport, DirectorActivationReport } from '../../api/directorTypes';
import { retryWhenBusy } from './retry';
import './ActivationPanel.css';

const message = (error: unknown) => isAxiosError(error) ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'Activation request failed';
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;

/** What each action does to a row, in words, most consequential first. */
const ACTIONS: Array<{ action: DirectorActivationAction; count: string; label: string }> = [
  { action: 'create', count: 'new', label: 'New' },
  { action: 'adopt', count: 'taken over', label: 'Taken over' },
  { action: 'update', count: 'updated', label: 'Updated' },
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

/** Every row activation touches in one rig's database, by kind, saying what
 *  happens to it and which existing row it lands on. */
function RigChanges({ rig }: { rig: DirectorActivationReport['rigs'][number] }) {
  if (rig.changes.length === 0) return null;
  const busy = rig.changes.some(change => change.action !== 'unchanged' && change.action !== 'keep');
  return <details className="activation-changes" open={busy}>
    <summary>{rig.catalog_name}: what changes</summary>
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

/** Push the framing and plan into each participating rig's database, with a preview first. */
export default function ActivationPanel({ projectId }: { projectId: string }) {
  const { canWrite } = useAccess();
  const status = useDirectorStatus();
  const manageable = status.data?.database_management ?? true;
  const client = useQueryClient();
  const last = useQuery({ queryKey: ['directorActivation', projectId], queryFn: () => apiClient.getDirectorActivation(projectId), retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false });
  const [report, setReport] = useState<DirectorActivationReport | null>(null);
  const busy = useRef(false);
  const preview = useMutation({ retry: false, mutationFn: () => apiClient.previewDirectorActivation(projectId), onSuccess: setReport });
  const apply = useMutation({
    retry: false,
    mutationFn: () => { if (!report) throw new Error('Preview first'); return apiClient.applyDirectorActivation(projectId, report.preview_digest); },
    onSuccess: applied => { setReport(applied); for (const key of [['directorActivation', projectId], ['db'], ['directorCatalog'], ['directorPlans']]) void client.invalidateQueries({ queryKey: key }); },
    onError: error => { if (httpStatus(error) === 409) setReport(null); },
  });
  const [pushed, setPushed] = useState<DirectorActivationPushReport | null>(null);
  const push = useMutation({ retry: false, mutationFn: () => apiClient.pushDirectorActivation(projectId), onSuccess: setPushed });
  const run = (action: () => void) => { if (busy.current) return; busy.current = true; try { action(); } finally { busy.current = false; } };
  const pending = preview.isPending || apply.isPending || push.isPending;
  const error = preview.error ?? apply.error ?? push.error;
  return <section className="activation" aria-label="Activation">
    <p className="director-muted">Activation writes this plan into each rig's Target Scheduler database: a project, a target per panel and an exposure plan per objective. Rows already there for the same work are taken over, not doubled, and captured frames and grades are never touched. Preview first; Apply is refused if anything changed since, and a rig on another PSF Guard gets the rows by Sync once applied.</p>
    {last.data && <p className="director-muted">Last activated revision {last.data.revision} on {new Date(last.data.applied_at_ms).toLocaleString()} across {last.data.rigs.length} rig{last.data.rigs.length === 1 ? '' : 's'}.</p>}
    {error && !(apply.isError && httpStatus(apply.error) === 409) && <p className="director-error" role="alert">{message(error)}</p>}
    {apply.isError && httpStatus(apply.error) === 409 && <p className="director-error" role="alert">Something changed since the preview. Preview again before applying.</p>}
    {report && <div className="activation-report">
      <p><strong>{report.applied ? 'Applied' : 'Preview'}</strong>: framing revision {report.framing_revision}, plan revision {report.plan_revision}, {report.panels} panel{report.panels === 1 ? '' : 's'}.{report.activation_revision !== null && ` Activation revision ${report.activation_revision}.`}</p>
      {report.warnings.map(warning => <p key={warning} className="director-error" role="alert">{warning}</p>)}
      <div className="director-table-scroll"><table className="activation-table"><thead><tr><th>Rig database</th><th>Project</th><th>Targets</th><th>Exposure plans</th><th>Remote site</th><th>Notes</th></tr></thead><tbody>
        {report.rigs.map(rig => <tr key={rig.rig.id}>
          <td><span className="director-cell-label">Rig database</span>{rig.catalog_name}{rig.applied && <> <Check size={14} aria-label="applied" /></>}</td>
          <td><span className="director-cell-label">Project</span>{counts(rig.changes, 'project')}</td>
          <td><span className="director-cell-label">Targets</span>{counts(rig.changes, 'target')}</td>
          <td><span className="director-cell-label">Exposure plans</span>{counts(rig.changes, 'plan')}</td>
          <td className={rig.push?.error ? 'director-error' : undefined}><span className="director-cell-label">Remote site</span>{describePush(rig.push, rig.applied)}</td>
          <td><span className="director-cell-label">Notes</span>{rig.warnings.join(' ')}</td>
        </tr>)}
      </tbody></table></div>
      {report.rigs.map(rig => <RigChanges key={rig.rig.id} rig={rig} />)}
    </div>}
    {pushed && <div className="activation-report" aria-label="Push result">
      <p><strong>Pushed again</strong>: activation revision {pushed.activation_revision}.</p>
      {pushed.warnings.map(warning => <p key={warning} className="director-muted">{warning}</p>)}
      <ul>{pushed.rigs.map(rig => <li key={rig.rig.id} className={rig.push.error ? 'director-error' : undefined}>{rig.catalog_name}: {describePush(rig.push, true)}</li>)}</ul>
    </div>}
    {canWrite && <div className="director-actions">
      <button type="button" disabled={pending} onClick={() => run(() => preview.mutate())}><Eye size={16} />{preview.isPending ? 'Previewing...' : report && !report.applied ? 'Preview again' : 'Preview activation'}</button>
      {report && !report.applied && <button type="button" disabled={pending || !manageable || report.rigs.every(r => r.warnings.length > 0 && r.changes.length === 0)} title={manageable ? undefined : 'This server cannot change rig databases'} onClick={() => run(() => apply.mutate())}><Check size={16} />{apply.isPending ? 'Applying...' : 'Apply to rig databases'}</button>}
      {last.data && <button type="button" disabled={pending || !manageable} title={manageable ? 'Send the last activation\'s rows to each remote rig\'s peer again' : 'This server cannot change rig databases'} onClick={() => run(() => push.mutate())}><Send size={16} />{push.isPending ? 'Pushing...' : 'Push to remote sites again'}</button>}
    </div>}
    {!canWrite && <p className="director-muted">Read only</p>}
    {canWrite && !manageable && <p className="director-muted">Preview works here; applying and pushing need a server started with database management.</p>}
  </section>;
}
