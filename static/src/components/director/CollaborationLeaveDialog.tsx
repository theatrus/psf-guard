import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../../api/client';
import type { DirectorCollaborationLeft } from '../../api/directorTypes';
import Dialog from '../Dialog';
import { collaborationName, coordinatorPlan, coordinatorResult, leaveChanges } from './collaborationLeaveText';
import '../ProjectDeleteDialog.css';

interface Props {
  planId: string;
  onClose: () => void;
  onLeft: (outcome: DirectorCollaborationLeft) => void;
}

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'The request failed.';

/** Leave the collaboration a plan came from. The plan becomes a plan of its
 *  own: it keeps its framing, exposures and activation, and its rig
 *  projects keep their targets, frames and grades. */
export default function CollaborationLeaveDialog({ planId, onClose, onLeft }: Props) {
  const client = useQueryClient();
  const query = useQuery({
    queryKey: ['directorCollaborationLeave', planId],
    queryFn: () => apiClient.getDirectorCollaborationLeave(planId),
    retry: false,
    gcTime: 0,
    refetchInterval: false,
    refetchOnWindowFocus: false,
  });
  const leave = useMutation({
    retry: false,
    mutationFn: () => apiClient.leaveDirectorCollaboration(planId),
    onSuccess: onLeft,
    onSettled: () => {
      void client.invalidateQueries({ queryKey: ['directorPlans'] });
      void client.invalidateQueries({ predicate: entry => entry.queryKey.includes(planId) });
    },
  });
  const preview = query.data ?? null;
  const left = leave.data ?? null;
  const busy = leave.isPending;

  return (
    <Dialog
      open
      title="Leave collaboration"
      className="project-delete-dialog"
      onClose={() => { if (!busy) onClose(); }}
      footer={left
        ? <button type="button" className="header-button" onClick={onClose}>Close</button>
        : <>
            <button type="button" className="header-button" disabled={busy} onClick={onClose}>Cancel</button>
            <button
              type="button"
              className="action-button reject project-delete-confirm"
              disabled={!preview?.collaboration || !!preview.refusal || busy || query.isFetching}
              onClick={() => leave.mutate()}
            >
              {busy ? 'Leaving...' : 'Leave collaboration'}
            </button>
          </>}
    >
      {query.isLoading && <p role="status">Reading the plan's collaboration...</p>}
      {query.isError && <p className="project-delete-error" role="alert">{message(query.error)}</p>}
      {left && <p role="status">
        {left.name} left {collaborationName(left.collaboration)} and is now a plan of your own.
        {left.assignments_ended > 0 && ` Ended ${left.assignments_ended.toLocaleString()} open assignment${left.assignments_ended === 1 ? '' : 's'}.`}
        {' '}{coordinatorResult(left.coordinator)}
      </p>}
      {left && left.warnings.map(warning => <p key={warning} className="project-delete-error" role="alert">{warning}</p>)}
      {!left && preview?.collaboration && (
        <>
          <p><strong>{preview.name}</strong> comes from {collaborationName(preview.collaboration)}. Leaving makes it a plan of your own. PSF Guard:</p>
          <ul className="project-delete-list">
            {leaveChanges(preview.collaboration).map(change => <li key={change}>{change}</li>)}
          </ul>
          <p>{coordinatorPlan(preview.collaboration)}</p>
          {preview.warnings.map(warning => <p key={warning} className="project-delete-error" role="note">{warning}</p>)}
          <p className="project-delete-note">
            The plan keeps its framing, exposures, priorities and activation record. Its rig projects keep their
            targets, frames and grades, and the targets stay scheduled until you change them.
          </p>
        </>
      )}
      {!left && preview?.refusal && <p className="project-delete-error" role="alert">{preview.refusal}</p>}
      {leave.isError && <p className="project-delete-error" role="alert">{message(leave.error)}</p>}
    </Dialog>
  );
}
