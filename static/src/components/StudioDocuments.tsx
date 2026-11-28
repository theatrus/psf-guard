import { useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import axios from 'axios';
import { apiClient } from '../api/client';
import type { ApiResponse, StudioDocumentSummary } from '../api/types';
import { studioLink } from '../utils/studioLink';
import { useOpenInStudio } from '../utils/useOpenInStudio';
import { PROCESSING_SETUPS_QUERY_KEY } from './defaultColorSetup';

function errorMessage(error: unknown): string {
  if (axios.isAxiosError<ApiResponse<unknown>>(error)) {
    return error.response?.data?.error || error.message;
  }
  return error instanceof Error ? error.message : String(error);
}

function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
  if (bytes >= 1024 * 1024) return `${Math.round(bytes / 1024 / 1024)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function formatDate(seconds: number): string {
  return new Date(seconds * 1000).toLocaleString(undefined, {
    dateStyle: 'medium',
    timeStyle: 'short',
  });
}

/** Who a server without accounts records as saving. */
const NO_ACCOUNT = 'this computer';

/**
 * Who saved a document's newest revision, and from which computer. A
 * server without accounts records every save as by "this computer", so that
 * save reads by the computer it came from.
 */
function savedBy(document: StudioDocumentSummary): string {
  const from = document.saved_from;
  if (from && document.saved_by === NO_ACCOUNT) return `saved from ${from}`;
  if (from) return `saved by ${document.saved_by} from ${from}`;
  return `saved by ${document.saved_by}`;
}

interface StudioDocumentsProps {
  dbId: string;
  projectId: number;
  canWrite: boolean;
}

/**
 * The project's PSF Studio documents: what each was last saved as, its
 * exports, and **Open in PSF Studio**. An editor can save a document as a
 * Color processing setup or delete it.
 */
export default function StudioDocuments({ dbId, projectId, canWrite }: StudioDocumentsProps) {
  const queryClient = useQueryClient();
  const studio = useOpenInStudio(dbId, projectId);
  const serverOrigin = studio.origin;
  const targets = useQuery({
    queryKey: ['studioDocumentTargets', dbId, projectId],
    queryFn: () => apiClient.getTargets(dbId, projectId),
  });
  const targetNames = useMemo(
    () => new Map((targets.data ?? []).map((target) => [target.id, target.name])),
    [targets.data],
  );
  const queryKey = ['studioDocuments', dbId, projectId];
  const documents = useQuery({
    queryKey,
    queryFn: () => apiClient.getStudioDocuments(dbId, projectId),
  });
  const [status, setStatus] = useState<{ id: string; text: string; error: boolean } | null>(null);
  const [naming, setNaming] = useState<{ id: string; name: string } | null>(null);

  const remove = useMutation({
    mutationFn: (document: StudioDocumentSummary) => apiClient.deleteStudioDocument(dbId, document.id),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey }),
    onError: (error, document) => setStatus({ id: document.id, text: errorMessage(error), error: true }),
  });
  const saveSetup = useMutation({
    mutationFn: async ({ id, name }: { id: string; name: string }) => {
      const settings = await apiClient.getStudioDocumentProcessing(dbId, id);
      return apiClient.saveProcessingSetup(name, 'color', settings);
    },
    onSuccess: (setup, { id }) => {
      setNaming(null);
      setStatus({ id, text: `Saved as the Color setup “${setup.name}”.`, error: false });
      void queryClient.invalidateQueries({ queryKey: PROCESSING_SETUPS_QUERY_KEY });
    },
    onError: (error, { id }) => setStatus({ id, text: errorMessage(error), error: true }),
  });

  const confirmDelete = (document: StudioDocumentSummary) => {
    const exports = document.exports.length;
    const question =
      `Delete the Studio document “${document.name}” and its saved revisions` +
      (exports > 0 ? `, and its ${exports} export${exports === 1 ? '' : 's'}` : '') +
      '? Files no other document uses are deleted from the server. This cannot be undone. ' +
      'A copy open in PSF Studio stays on that computer.';
    if (window.confirm(question)) remove.mutate(document);
  };

  if (documents.isLoading) return null;
  if (documents.isError) {
    return <div className="status-message error">{errorMessage(documents.error)}</div>;
  }
  const list = documents.data ?? [];
  if (list.length === 0) return null;

  return (
    <section className="studio-documents" aria-label="PSF Studio documents">
      <h4>PSF Studio documents</h4>
      {studio.error && <span className="status-message error" role="alert">{studio.error}</span>}
      <ul>
        {list.map((document) => {
          const target = document.target_id;
          return (
            <li key={document.id} className="studio-document">
              {document.preview ? (
                <img
                  className="studio-document-preview"
                  src={apiClient.studioFileUrl(dbId, document.preview)}
                  alt=""
                />
              ) : (
                <div className="studio-document-preview studio-document-no-preview" aria-hidden="true" />
              )}
              <div className="studio-document-main">
                <strong>{document.name}</strong>
                <span className="muted">
                  {target !== undefined && targetNames.get(target) ? `${targetNames.get(target)} · ` : ''}
                  Revision {document.revision}, {savedBy(document)} on {formatDate(document.saved_at)}
                </span>
                {document.exports.length > 0 && (
                  <ul className="studio-document-exports" aria-label="Exports">
                    {document.exports.map((exported) => (
                      <li key={`${exported.sha256}-${exported.saved_at}`}>
                        <a href={apiClient.studioFileUrl(dbId, exported.sha256)} download={exported.name}>
                          {exported.name}
                        </a>{' '}
                        <span className="muted">
                          {formatBytes(exported.size)}, from revision {exported.revision}
                        </span>
                      </li>
                    ))}
                  </ul>
                )}
                {naming?.id === document.id && (
                  <form
                    className="studio-document-setup"
                    onSubmit={(event) => {
                      event.preventDefault();
                      if (naming.name.trim()) saveSetup.mutate({ id: document.id, name: naming.name.trim() });
                    }}
                  >
                    <label>
                      Setup name{' '}
                      <input
                        value={naming.name}
                        onChange={(event) => setNaming({ id: document.id, name: event.target.value })}
                        maxLength={64}
                        autoFocus
                      />
                    </label>
                    <button type="submit" className="save-button" disabled={saveSetup.isPending}>
                      {saveSetup.isPending ? 'Saving…' : 'Save setup'}
                    </button>
                    <button type="button" className="cancel-button" onClick={() => setNaming(null)}>
                      Cancel
                    </button>
                  </form>
                )}
                {status?.id === document.id && (
                  <span className={status.error ? 'status-message error' : 'muted'} role="status">
                    {status.text}
                  </span>
                )}
              </div>
              <div className="studio-document-actions">
                {serverOrigin && target !== undefined && (
                  <a
                    className="stack-preview-card-action"
                    href={studioLink(serverOrigin, dbId, projectId, target, undefined, document.id)}
                    onClick={(event) => void studio.open(event, target, document.id)}
                    title="PSF Studio opens this document, with the target's current channel stacks. PSF Studio must be installed on this computer."
                  >
                    Open in PSF Studio
                  </a>
                )}
                {canWrite && (
                  <button
                    type="button"
                    className="stack-preview-card-action"
                    onClick={() => {
                      setStatus(null);
                      setNaming({ id: document.id, name: document.name.slice(0, 64) });
                    }}
                    title="Saves the document's steps as a Color processing setup, which color previews can start from. Works when every step is one PSF Guard's color pipeline runs."
                  >
                    Save as a Color setup
                  </button>
                )}
                {canWrite && (
                  <button
                    type="button"
                    className="stack-preview-card-action remove-button"
                    onClick={() => confirmDelete(document)}
                    disabled={remove.isPending}
                  >
                    Delete
                  </button>
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
