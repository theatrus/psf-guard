import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../api/client';
import { useDirectorStatus } from '../hooks/useDirectorStatus';
import Dialog from './Dialog';
import { listNames } from './director/listNames';
import './ProjectDeleteDialog.css';
import './DatabaseRemoveDialog.css';

interface Props {
  database: { id: string; name: string };
  onClose: () => void;
  onRemoved: (name: string) => void;
}

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'The request failed.';

const BLOCKERS = {
  open_work: 'This rig has open work. Open Rig setup, choose Close work on the Director tab, then remove it.',
} as const;

/** Remove a database and its rig. The file and images stay on disk, but the
 *  rig's settings and pairing are deleted, so the dialog lists what goes. The
 *  server refuses a rig with open work. */
export default function DatabaseRemoveDialog({ database, onClose, onRemoved }: Props) {
  const client = useQueryClient();
  const director = useDirectorStatus();
  const planning = !!director.data?.enabled;
  const review = useQuery({
    queryKey: ['directorRigRemoval', database.id],
    queryFn: () => apiClient.getDirectorRigRemoval(database.id),
    enabled: planning,
    retry: false,
    gcTime: 0,
  });
  const remove = useMutation({
    retry: false,
    mutationFn: () => apiClient.removeDatabase(database.id),
    onSuccess: (removed) => {
      if (!removed) return;
      for (const queryKey of [['databases'], ['db', database.id], ['directorPlans'], ['directorRigProfiles'], ['directorRigStatuses'], ['directorRigSummary']]) {
        void client.invalidateQueries({ queryKey });
      }
      onRemoved(database.name);
    },
  });
  const busy = remove.isPending;
  const removal = review.data ?? null;
  // Until Director's status arrives, the dialog cannot tell whether a rig's
  // records go too, so Remove waits for it.
  const reading = director.isLoading || (planning && review.isLoading);
  const blocked = removal?.blocker ? BLOCKERS[removal.blocker] : null;

  return (
    <Dialog
      open
      title={`Remove ${database.name}`}
      className="project-delete-dialog database-remove-dialog"
      onClose={() => { if (!busy) onClose(); }}
      footer={
        <>
          <button type="button" className="header-button" disabled={busy} onClick={onClose}>Cancel</button>
          <button
            type="button"
            className="action-button reject project-delete-confirm"
            disabled={busy || reading || !!blocked}
            onClick={() => remove.mutate()}
          >
            {busy ? 'Removing...' : 'Remove'}
          </button>
        </>
      }
    >
      {reading && <p role="status">Reading what this rig has...</p>}
      {review.isError && (
        <p className="project-delete-error" role="alert">
          PSF Guard could not list this rig's records: {message(review.error)} Remove still removes the database, and
          deletes the rig's settings, pairing and plan parts if PSF Guard can.
        </p>
      )}
      <p>The database file and its images stay on disk. PSF Guard stops listing <strong>{database.name}</strong>.</p>
      {removal?.collaboration && (
        <p className="project-delete-note">
          {removal.rig.name} has collaboration records, which can hold reports not sent yet. PSF Guard keeps the rig's
          settings, pairing and plans, and adding the file back restores them.
        </p>
      )}
      {removal && !removal.collaboration && (
        <>
          <p>This also deletes, and you cannot undo it:</p>
          <ul className="project-delete-list">
            <li>the settings for {removal.rig.name}: optics, site, limits and Director history</li>
            {removal.plans.length > 0 && <li>this rig's part of {listNames(removal.plans)}. The {removal.plans.length === 1 ? 'plan stays' : 'plans stay'}, and other rigs keep theirs.</li>}
            {removal.plugins.length > 0 && <li>the Director pairing with {listNames(removal.plugins)}, which must pair again</li>}
          </ul>
        </>
      )}
      {blocked && <p className="project-delete-error" role="alert">{blocked}</p>}
      {remove.isError && <p className="project-delete-error" role="alert">{message(remove.error)}</p>}
      {remove.isSuccess && !remove.data && <p className="project-delete-error" role="alert">PSF Guard no longer lists this database.</p>}
    </Dialog>
  );
}
