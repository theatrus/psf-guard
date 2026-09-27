import { useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Eye } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorActivationReport } from '../../api/directorTypes';
import './ActivationPanel.css';

const message = (error: unknown) => isAxiosError(error) ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'Activation request failed';
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;

function counts(changes: DirectorActivationReport['rigs'][number]['changes'], kind: 'project' | 'target' | 'plan') {
  const of = changes.filter(c => c.kind === kind);
  const n = (action: string) => of.filter(c => c.action === action).length;
  const parts = [n('create') && `${n('create')} new`, n('update') && `${n('update')} updated`, n('unchanged') && `${n('unchanged')} unchanged`].filter(Boolean);
  return parts.length ? parts.join(', ') : 'none';
}

/** Push the framing and plan into each participating rig's database, with a preview first. */
export default function ActivationPanel({ projectId }: { projectId: string }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const last = useQuery({ queryKey: ['directorActivation', projectId], queryFn: () => apiClient.getDirectorActivation(projectId), retry: false, refetchOnWindowFocus: false });
  const [report, setReport] = useState<DirectorActivationReport | null>(null);
  const busy = useRef(false);
  const preview = useMutation({ retry: false, mutationFn: () => apiClient.previewDirectorActivation(projectId), onSuccess: setReport });
  const apply = useMutation({
    retry: false,
    mutationFn: () => { if (!report) throw new Error('Preview first'); return apiClient.applyDirectorActivation(projectId, report.preview_digest); },
    onSuccess: applied => { setReport(applied); void client.invalidateQueries({ queryKey: ['directorActivation', projectId] }); void client.invalidateQueries({ queryKey: ['db'] }); void client.invalidateQueries({ queryKey: ['directorCatalog'] }); },
    onError: error => { if (httpStatus(error) === 409) setReport(null); },
  });
  const run = (action: () => void) => { if (busy.current) return; busy.current = true; try { action(); } finally { busy.current = false; } };
  const pending = preview.isPending || apply.isPending;
  const error = preview.error ?? apply.error;
  return <section className="activation" aria-label="Activation">
    <p className="director-muted">Activation writes the framing and plan into each participating rig's database: one Target Scheduler project, one target per panel, and one exposure plan per rig objective. Existing rows are updated in place; captured frames and grades are never touched. Preview first; Apply is refused when anything changed since the preview.</p>
    {last.data && <p className="director-muted">Last activated revision {last.data.revision} on {new Date(last.data.applied_at_ms).toLocaleString()} across {last.data.rigs.length} rig{last.data.rigs.length === 1 ? '' : 's'}.</p>}
    {error && !(apply.isError && httpStatus(apply.error) === 409) && <p className="director-error" role="alert">{message(error)}</p>}
    {apply.isError && httpStatus(apply.error) === 409 && <p className="director-error" role="alert">Something changed since the preview. Preview again before applying.</p>}
    {report && <div className="activation-report">
      <p><strong>{report.applied ? 'Applied' : 'Preview'}</strong>: framing revision {report.framing_revision}, plan revision {report.plan_revision}, {report.panels} panel{report.panels === 1 ? '' : 's'}.{report.activation_revision !== null && ` Activation revision ${report.activation_revision}.`}</p>
      {report.warnings.map(warning => <p key={warning} className="director-error" role="alert">{warning}</p>)}
      <div className="director-table-scroll"><table className="activation-table"><thead><tr><th>Rig database</th><th>Project</th><th>Targets</th><th>Exposure plans</th><th>Notes</th></tr></thead><tbody>
        {report.rigs.map(rig => <tr key={rig.rig.id}>
          <td><span className="director-cell-label">Rig database</span>{rig.catalog_name}{rig.applied && <> <Check size={14} aria-label="applied" /></>}</td>
          <td><span className="director-cell-label">Project</span>{counts(rig.changes, 'project')}</td>
          <td><span className="director-cell-label">Targets</span>{counts(rig.changes, 'target')}</td>
          <td><span className="director-cell-label">Exposure plans</span>{counts(rig.changes, 'plan')}</td>
          <td><span className="director-cell-label">Notes</span>{rig.warnings.length ? rig.warnings.join(' ') : rig.changes.filter(c => c.action !== 'unchanged').map(c => `${c.name}: ${c.detail}`).slice(0, 4).join('; ')}</td>
        </tr>)}
      </tbody></table></div>
    </div>}
    {canWrite && <div className="director-actions">
      <button type="button" disabled={pending} onClick={() => run(() => preview.mutate())}><Eye size={16} />{preview.isPending ? 'Previewing...' : report && !report.applied ? 'Preview again' : 'Preview activation'}</button>
      {report && !report.applied && <button type="button" disabled={pending || report.rigs.every(r => r.warnings.length > 0 && r.changes.length === 0)} onClick={() => run(() => apply.mutate())}><Check size={16} />{apply.isPending ? 'Applying...' : 'Apply to rig databases'}</button>}
    </div>}
    {!canWrite && <p className="director-muted">Read only</p>}
  </section>;
}
