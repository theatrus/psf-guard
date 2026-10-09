import { useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, RefreshCw } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { retryWhenBusy } from './retry';
import './RigSchedulingApply.css';

const STATES = ['Draft', 'Active', 'Inactive', 'Closed'];
const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'Could not compare the rig limits';
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;

/** The rig's saved scheduling limits against every project in its Target
 *  Scheduler database, and one Apply that writes them where they differ. */
export default function RigSchedulingApply({ rigId, unsaved }: { rigId: string; unsaved: boolean }) {
  const { canWrite } = useAccess();
  const status = useDirectorStatus();
  const manageable = status.data?.database_management ?? true;
  const compared = useQuery({ queryKey: ['rigScheduling', rigId], queryFn: () => apiClient.getRigScheduling(rigId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const [notice, setNotice] = useState('');
  const apply = useMutation({
    retry: false,
    mutationFn: (digest: string) => apiClient.applyRigScheduling(rigId, digest),
    onSuccess: (done, digest) => {
      const written = compared.data?.digest === digest ? compared.data.projects.length : 0;
      const pushed = done.push ? (done.push.applied ? ` Sent to ${done.push.peer_name}.` : ` Not sent to ${done.push.peer_name}${done.push.error ? `: ${done.push.error}` : ''}.`) : '';
      setNotice(done.applied ? `Wrote the limits into ${written} project${written === 1 ? '' : 's'}.${pushed}` : done.warnings.join(' '));
      void compared.refetch();
    },
    onError: error => {
      // Something changed since the list was read: show the new one.
      if (httpStatus(error) === 409) { setNotice('A project or the limits changed since; review the list again.'); void compared.refetch(); }
    },
  });
  const data = compared.data;
  const differ = data?.projects.length ?? 0;
  const why = !canWrite ? 'Read only' : !manageable ? 'This server does not write to rig databases' : unsaved ? 'Save the rig profile first' : '';
  return <section className="rig-scheduling-apply" aria-label="Target Scheduler projects">
    <div className="director-toolbar">
      <h5>Target Scheduler projects</h5>
      <button type="button" aria-label="Compare again" title="Compare again" disabled={compared.isFetching} onClick={() => { setNotice(''); apply.reset(); void compared.refetch(); }}><RefreshCw size={16} /></button>
    </div>
    {compared.isPending && <p role="status">Comparing projects...</p>}
    {compared.isError && <p className="director-error" role="alert">{message(compared.error)}</p>}
    {data && <>
      {data.warnings.map(warning => <p key={warning} className="director-muted" role="note">{warning}</p>)}
      <p className="director-muted" data-testid="rig-scheduling-summary">{differ === 0
        ? `${data.matching === 1 ? 'The one project' : `All ${data.matching} projects`} in ${data.catalog_name} ${data.matching === 1 ? 'has' : 'have'} these limits.`
        : `${differ} of ${differ + data.matching} projects in ${data.catalog_name} differ.`}{unsaved && ' The list shows the saved limits.'}</p>
      {differ > 0 && <ul className="rig-scheduling-projects">
        {data.projects.map(project => <li key={`${project.name}:${project.changes.map(change => change.label).join(',')}`}>
          <span className="rig-scheduling-name"><strong>{project.name}</strong><span className="workspace-pill">{STATES[project.state] ?? `State ${project.state}`}</span>
            {project.plan && <small className="director-muted">{project.plan.name}'s own limits</small>}</span>
          <span className="rig-scheduling-changes">{project.changes.map(change => `${change.label} ${change.was} → ${change.now}`).join(', ')}</span>
        </li>)}
      </ul>}
      {data.unset.length > 0 && <p className="director-muted">Not set, so each project keeps its own: {data.unset.join(', ')}.</p>}
      {data.push && <p className="director-muted">Apply also sends the rows to {data.push.peer_name}.</p>}
      {differ > 0 && <div className="director-actions">
        <button type="button" disabled={!!why || apply.isPending || compared.isFetching || !data.digest} title={why || undefined} onClick={() => { setNotice(''); apply.mutate(data.digest); }}>
          <Check size={16} />{apply.isPending ? 'Applying...' : `Apply to ${differ} project${differ === 1 ? '' : 's'}`}
        </button>
        {why && <span className="director-muted">{why}</span>}
      </div>}
    </>}
    {apply.isError && httpStatus(apply.error) !== 409 && <p className="director-error" role="alert">{message(apply.error)}</p>}
    {notice && <p role="status">{notice}</p>}
  </section>;
}
