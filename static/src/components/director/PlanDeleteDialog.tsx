import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../../api/client';
import type { DirectorPlanDeletion, DirectorPlanDeletionOutcome } from '../../api/directorTypes';
import Dialog from '../Dialog';
import { listNames } from './listNames';
import { collaborationName, coordinatorPlan, leaveChanges } from './collaborationLeaveText';
import '../ProjectDeleteDialog.css';

interface Props {
  planId: string;
  name: string;
  onClose: () => void;
  onDeleted: (outcome: Extract<DirectorPlanDeletionOutcome, { state: 'deleted' }>) => void;
}

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'The request failed.';
const count = (value: number, word: string) => `${value.toLocaleString()} ${word}${value === 1 ? '' : 's'}`;
/** "The project on C925 was" or "The projects on C925 and RedCat were". */
const deletedOn = (names: string[]) =>
  `The project${names.length === 1 ? '' : 's'} on ${listNames(names)} ${names.length === 1 ? 'was' : 'were'} deleted`;

/** Delete a plan with nothing captured: its project in each database it
 *  links, then the plan with its framing and drafts. A plan from a
 *  collaboration leaves it first. The server refuses a plan whose rigs
 *  captured or reported frames, and a delete whose preview is out of date,
 *  which answers with a new one. */
export default function PlanDeleteDialog({ planId, name, onClose, onDeleted }: Props) {
  const client = useQueryClient();
  // What the last attempt left: a newer preview, or an error, and the
  // databases whose project went before it stopped.
  const [after, setAfter] = useState<{ preview?: DirectorPlanDeletion; error?: string; deleted: string[] } | null>(null);
  const query = useQuery({
    queryKey: ['directorPlanDeletion', planId],
    queryFn: () => apiClient.getDirectorPlanDeletion(planId),
    retry: false,
    gcTime: 0,
    // The digests belong to what the dialog shows: no refresh replaces them.
    staleTime: Infinity,
    refetchInterval: false,
    refetchOnWindowFocus: false,
  });
  const preview = after?.preview ?? query.data ?? null;
  const remove = useMutation({
    retry: false,
    mutationFn: (current: DirectorPlanDeletion) => apiClient.deleteDirectorPlan(planId, Object.fromEntries(
      current.databases.flatMap(database => database.project ? [[database.source_project_guid, database.project.digest]] : []))),
    onSuccess: (outcome) => {
      if (outcome.state === 'deleted') onDeleted(outcome);
      else if (outcome.state === 'changed') setAfter({ preview: outcome.preview, deleted: outcome.deleted });
      else {
        // The preview no longer matches what is left: read it again.
        setAfter({ error: outcome.error, deleted: outcome.deleted });
        void query.refetch();
      }
    },
    onError: () => { setAfter(null); void query.refetch(); },
    onSettled: () => {
      void client.invalidateQueries({ queryKey: ['directorPlans'] });
      for (const database of preview?.databases ?? []) {
        if (database.catalog_slug) void client.invalidateQueries({ queryKey: ['db', database.catalog_slug] });
      }
    },
  });
  const busy = remove.isPending;

  return (
    <Dialog
      open
      title={`Delete ${name}`}
      className="project-delete-dialog"
      onClose={() => { if (!busy) onClose(); }}
      footer={
        <>
          <button type="button" className="header-button" disabled={busy} onClick={onClose}>Cancel</button>
          <button
            type="button"
            className="action-button reject project-delete-confirm"
            disabled={!preview || !!preview.refusal || busy || query.isFetching}
            onClick={() => preview && remove.mutate(preview)}
          >
            {busy ? 'Deleting...' : 'Delete plan'}
          </button>
        </>
      }
    >
      {query.isLoading && <p role="status">Reading the plan's projects...</p>}
      {query.isError && <p className="project-delete-error" role="alert">{message(query.error)}</p>}
      {preview && (
        <>
          <p>Deleting the plan <strong>{preview.name}</strong> removes:</p>
          <ul className="project-delete-list">
            <li>its Framing, Exposures and Priority settings, and its activation record</li>
            {preview.databases.map((database, index) => (
              <li key={database.source_project_guid || index}>
                {database.project
                  ? `On ${database.catalog_name}: the project ${database.project.name}, with ${count(database.project.targets, 'target')} and ${count(database.project.exposure_plans, 'exposure plan')}`
                  : database.catalog_name
                    ? `On ${database.catalog_name}: nothing; the project is no longer there`
                    : 'A database this server does not have'}
              </li>
            ))}
          </ul>
          {preview.collaboration && <div className="project-delete-note" role="note" aria-label="Collaboration">
            <p>This plan comes from {collaborationName(preview.collaboration)}. Deleting it leaves the collaboration first:</p>
            <ul className="project-delete-list">
              {leaveChanges(preview.collaboration).map(change => <li key={change}>{change}</li>)}
            </ul>
            <p>{coordinatorPlan(preview.collaboration)}</p>
          </div>}
          {preview.databases.map(database => database.project && (database.synced_from
            ? <p key={`note-${database.source_project_guid}`} className="project-delete-note">
                {database.catalog_name} copies from <code>{database.synced_from}</code>. The project stays in that
                database, N.I.N.A. still schedules it there while it is Active, and the next sync adds it back here.
              </p>
            : database.project.state === 1 && <p key={`note-${database.source_project_guid}`} className="project-delete-note">
                On {database.catalog_name} the project is Active. When N.I.N.A. uses that database, it stops scheduling the project.
              </p>))}
          {after && !busy && <p className="project-delete-error" role="alert">
            {[
              after.deleted.length > 0 ? `${deletedOn(after.deleted)}, but the plan stays.` : '',
              after.error ?? 'The plan changed after you opened this dialog. Check the new list, then delete again.',
            ].filter(Boolean).join(' ')}
          </p>}
          {preview.refusal && <p className="project-delete-error" role="alert">{preview.refusal}</p>}
          {remove.isError && <p className="project-delete-error" role="alert">{message(remove.error)}</p>}
        </>
      )}
    </Dialog>
  );
}
