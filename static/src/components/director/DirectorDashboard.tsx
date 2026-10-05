import { Link, useSearchParams } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { RefreshCw } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorRigStatusView } from '../../api/directorTypes';
import { describeNow, errorsOf, formatAge } from './dashboardModel';
import { retryWhenBusy } from './retry';
import { planHref } from './planAddress';
import { useDirectorClock } from '../../hooks/useDirectorClock';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

const STATE_LABEL: Record<DirectorRigStatusView['connectivity']['state'], string> = { online: 'Online', stale: 'Quiet', offline: 'Offline', never: 'Never seen' };

/** Every rig as the operator wants to see it at a glance: reachable or not,
 *  what it is doing, when it last pulled, checked in and reported, and what
 *  it is assigned. Ages are from server receipt times; a stale report is
 *  labelled, never shown as the present. */
export default function DirectorDashboard() {
  const [params] = useSearchParams();
  const statuses = useQuery({ queryKey: ['directorRigStatuses'], queryFn: apiClient.getDirectorRigStatuses, retry: retryWhenBusy, retryDelay: 1200, refetchInterval: 15_000, refetchOnWindowFocus: true });
  const now = useDirectorClock();
  const workspaceHref = (projectId: string) => planHref(projectId, params);
  const rows = statuses.data ?? [];
  const age = (at: number | null | undefined) => at ? formatAge(now - at) : 'never';
  return <section className="director-records director-dashboard" aria-label="Live rigs">
    <div className="director-toolbar">
      <h2>Live</h2>
      <button type="button" title="Refresh live status" aria-label="Refresh live status" disabled={statuses.isFetching} onClick={() => void statuses.refetch()}><RefreshCw size={16} /></button>
    </div>
    {statuses.isPending && <p role="status">Loading live status...</p>}
    {statuses.isError && <p className="director-error" role="alert">{message(statuses.error)}</p>}
    {statuses.isSuccess && rows.length === 0 && <p className="director-muted">No rig yet. Every registered database becomes a rig once the Library lists its plans.</p>}
    {rows.length > 0 && <div className="director-table-scroll"><table className="director-dashboard-table" data-testid="director-dashboard">
      <thead><tr><th>Rig</th><th>Link</th><th>Now</th><th>Program pull</th><th>Check-in</th><th>Status report</th><th>Assigned</th></tr></thead>
      <tbody>{rows.map(view => {
        const errors = errorsOf(view);
        return <tr key={view.rig.id} className={`is-${view.connectivity.state}`}>
          <td><span className="director-cell-label">Rig</span><strong>{view.catalog_name ?? view.rig.name}</strong>{view.catalog_name && view.catalog_name !== view.rig.name && <span className="director-muted"> {view.rig.name}</span>}</td>
          <td><span className="director-cell-label">Link</span><span className={`director-connectivity is-${view.connectivity.state}`}>{STATE_LABEL[view.connectivity.state]}</span>{view.connectivity.age_ms !== null && <span className="director-muted"> {formatAge(view.connectivity.age_ms)}</span>}</td>
          <td><span className="director-cell-label">Now</span>{describeNow(view, now)}{errors.map(error => <span key={error} className="director-error"> {error}</span>)}
            {!!view.recent_operations?.length && <details><summary>Completed operations ({view.recent_operations.length})</summary>
              <ol>{view.recent_operations.map(receipt => {
                const { command, completion } = receipt.event.event.observation;
                return <li key={`${receipt.event.ledger_id}:${receipt.event.sequence}`}>
                  {command.operation.operation.replaceAll('_', ' ')}: {completion.outcome.outcome}, {Math.round(completion.elapsed_ms / 1000)} s
                  {completion.outcome.reason && ` (${completion.outcome.reason})`}
                  <span className="director-muted"> · completed {age(completion.ended_at_ms)} · received {age(receipt.received_at_ms)}</span>
                </li>;
              })}</ol>
            </details>}
          </td>
          <td><span className="director-cell-label">Program pull</span>{age(view.contacts.program_pull?.at_ms)}{view.contacts.program_pull?.detail && <span className="director-muted"> rev {view.contacts.program_pull.detail.slice(0, 8)}</span>}</td>
          <td><span className="director-cell-label">Check-in</span>{age(view.contacts.check_in?.at_ms)}{view.pending_receipts > 0 && <span className="director-muted"> {view.pending_receipts} saved, ungraded</span>}</td>
          <td><span className="director-cell-label">Status report</span>{age(view.contacts.status?.at_ms)}</td>
          <td><span className="director-cell-label">Assigned</span>{view.assignments.length === 0 ? <span className="director-muted">nothing activated</span>
            : view.assignments.map((assignment, index) => <span key={assignment.project.id}>{index > 0 && ', '}<Link to={workspaceHref(assignment.project.id)}>{assignment.project.name}</Link></span>)}</td>
        </tr>;
      })}</tbody>
    </table></div>}
  </section>;
}
