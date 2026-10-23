import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../api/client';
import type { ProjectRemovalOutcome, ProjectRemovalPreview } from '../api/types';
import Dialog from './Dialog';
import './ProjectDeleteDialog.css';

export interface ProjectDeleteRequest {
  dbId: string;
  dbName: string;
  projectId: number;
  name: string;
}

interface Props {
  request: ProjectDeleteRequest;
  onClose: () => void;
  /** The project is gone; `outcome` reports what went with it. */
  onDeleted: (outcome: Extract<ProjectRemovalOutcome, { state: 'removed' }>) => void;
}

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'The request failed.';
const count = (value: number, word: string) => `${value.toLocaleString()} ${word}${value === 1 ? '' : 's'}`;

/** Why this project cannot be deleted here, or null. */
function refusal(preview: ProjectRemovalPreview): string | null {
  if (!preview.allowed) {
    return 'This server does not allow database changes. Restart it with --allow-database-management to delete projects.';
  }
  if (preview.linked_plan?.from_collaboration) {
    return `This project belongs to ${preview.linked_plan.name}, a plan from a collaboration. Open the plan and choose `
      + 'Leave collaboration under Collaboration assignments, then delete the project here.';
  }
  if (preview.protected_frames > 0) {
    return `${count(preview.protected_frames, 'frame')} of this project ${preview.protected_frames === 1 ? 'was' : 'were'} reported to a collaboration, so it cannot be deleted.`;
  }
  return null;
}

/** Delete a project from one catalog: what goes, what stays, and a typed
 *  confirmation when frames go too. The server refuses a delete whose
 *  preview is out of date and answers with a new one. */
export default function ProjectDeleteDialog({ request, onClose, onDeleted }: Props) {
  const client = useQueryClient();
  const [typed, setTyped] = useState('');
  const [changed, setChanged] = useState<ProjectRemovalPreview | null>(null);
  const query = useQuery({
    queryKey: ['projectRemoval', request.dbId, request.projectId],
    queryFn: () => apiClient.getProjectRemoval(request.dbId, request.projectId),
    retry: false,
    gcTime: 0,
    // The digest belongs to what the dialog shows. The app's 30 s refresh,
    // or one on focus, would swap both without a word, and a frame captured
    // meanwhile would be deleted unseen.
    staleTime: Infinity,
    refetchInterval: false,
    refetchOnWindowFocus: false,
  });
  const preview = changed ?? query.data ?? null;
  const remove = useMutation({
    retry: false,
    mutationFn: (digest: string) => apiClient.removeProject(request.dbId, request.projectId, digest),
    onSuccess: (outcome) => {
      if (outcome.state === 'changed') {
        setChanged(outcome.preview);
        return;
      }
      void client.invalidateQueries({ queryKey: ['db', request.dbId] });
      void client.invalidateQueries({ queryKey: ['directorPlans'] });
      onDeleted(outcome);
    },
  });
  const blocked = preview ? refusal(preview) : null;
  const withFrames = (preview?.frames.total ?? 0) > 0;
  // What to type: the project's name, or "delete" for a project without one.
  const confirmation = preview?.name.trim() || 'delete';
  const confirmed = !withFrames || typed.trim() === confirmation;
  const busy = remove.isPending;

  return (
    <Dialog
      open
      title={`Delete ${request.name}`}
      className="project-delete-dialog"
      onClose={() => { if (!busy) onClose(); }}
      footer={
        <>
          <button type="button" className="header-button" disabled={busy} onClick={onClose}>Cancel</button>
          <button
            type="button"
            className="action-button reject project-delete-confirm"
            disabled={!preview || !!blocked || !confirmed || busy}
            onClick={() => preview && remove.mutate(preview.digest)}
          >
            {busy ? 'Deleting...' : 'Delete project'}
          </button>
        </>
      }
    >
      {query.isLoading && <p role="status">Counting the project's targets and frames...</p>}
      {query.isError && <p className="project-delete-error" role="alert">{message(query.error)}</p>}
      {preview && (
        <>
          <p>
            Deleting <strong>{preview.name}</strong> removes it from <strong>{request.dbName}</strong>, with:
          </p>
          <ul className="project-delete-list">
            <li>{count(preview.targets, 'target')} and {count(preview.exposure_plans, 'exposure plan')}</li>
            {withFrames
              ? <li>
                  {count(preview.frames.total, 'frame')} ({preview.frames.accepted.toLocaleString()} accepted,{' '}
                  {preview.frames.rejected.toLocaleString()} rejected, {preview.frames.pending.toLocaleString()} pending),
                  with their grades and measurements. The image files stay on disk.
                </li>
              : <li>No frames: nothing was captured for this project.</li>}
          </ul>
          {preview.synced_from
            ? <p className="project-delete-note">
                This catalog copies from <code>{preview.synced_from}</code>. The project stays in that
                database, and N.I.N.A. still schedules it there while it is Active. {withFrames
                  ? 'Later syncs leave the project and its new frames out of this catalog.'
                  : 'Later syncs leave the project out of this catalog.'}
              </p>
            : preview.state === 1 && <p className="project-delete-note">
                The project is Active. When N.I.N.A. uses this database, it stops scheduling the project.
              </p>}
          {withFrames && <p className="project-delete-note">
            Imports skip these files from now on, so they do not bring the project back.
          </p>}
          {preview.linked_plan && !preview.linked_plan.from_collaboration && <p className="project-delete-note">
            {preview.linked_plan.databases > 1
              ? `Its plan, ${preview.linked_plan.name}, keeps its other ${count(preview.linked_plan.databases - 1, 'database')} and no longer plans work for this rig.`
              : `Its plan, ${preview.linked_plan.name}, keeps its framing and moves to Plans with nothing captured yet.`}
          </p>}
          {changed && !busy && <p className="project-delete-error" role="alert">
            The project changed after you opened this dialog. Check the new counts, then delete again.
          </p>}
          {blocked && <p className="project-delete-error" role="alert">{blocked}</p>}
          {withFrames && !blocked && (
            <label className="project-delete-confirm-field">
              <span>Type <strong>{confirmation}</strong> to delete the project and its frames</span>
              <input
                value={typed}
                onChange={(event) => setTyped(event.target.value)}
                autoComplete="off"
                spellCheck={false}
                disabled={busy}
              />
            </label>
          )}
          {remove.isError && <p className="project-delete-error" role="alert">{message(remove.error)}</p>}
        </>
      )}
    </Dialog>
  );
}
