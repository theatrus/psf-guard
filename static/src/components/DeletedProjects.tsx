import { useEffect, useRef, useState } from 'react';
import { useMutation, useQueries, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../api/client';
import type { DeletedProject } from '../api/types';

interface Catalog {
  id: string;
  name: string;
}

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'The request failed.';
const day = (seconds: number) => new Date(seconds * 1000).toLocaleDateString();

/** The Library's list of deleted projects, which syncs and imports leave
 *  out, with **Allow again** to let the next sync or import add one back.
 *  Shown only to editors on a server that allows database changes. */
export default function DeletedProjects({ catalogs }: { catalogs: Catalog[] }) {
  const client = useQueryClient();
  const [open, setOpen] = useState(false);
  const [notice, setNotice] = useState('');
  const noticeRef = useRef<HTMLParagraphElement>(null);
  const lists = useQueries({
    queries: catalogs.map(catalog => ({
      queryKey: ['db', catalog.id, 'deleted-projects'],
      queryFn: () => apiClient.getDeletedProjects(catalog.id),
      // Deleting a project and Allow again refresh the list; nothing else
      // changes it, so it skips the app's 30 s refresh. A catalog that
      // cannot be read lists nothing.
      staleTime: Infinity,
      refetchInterval: false as const,
      retry: false,
    })),
  });
  const rows = catalogs.flatMap((catalog, index) =>
    (lists[index]?.data ?? []).map(deleted => ({ catalog, deleted })));
  const allow = useMutation({
    retry: false,
    mutationFn: ({ catalog, deleted }: { catalog: Catalog; deleted: DeletedProject }) =>
      apiClient.allowDeletedProjectBack(catalog.id, deleted.id),
    onSuccess: (_, { catalog, deleted }) => setNotice(
      `${deleted.name} can come back to ${catalog.name}: the next sync, or an import of its files, adds it as a new project.`),
    onError: (error, { deleted }) => setNotice(`Could not allow ${deleted.name} back: ${message(error)}`),
    onSettled: (_, __, { catalog }) =>
      client.invalidateQueries({ queryKey: ['db', catalog.id, 'deleted-projects'] }),
  });
  // The row and its button leave the list: focus moves to the notice.
  useEffect(() => {
    if (notice) noticeRef.current?.focus();
  }, [notice]);
  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(''), 8000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  return <>
    {notice && <p ref={noticeRef} tabIndex={-1} className="library-narrowed" role="status">{notice}</p>}
    {rows.length > 0 && (
      <section className="project-archive deleted-projects" aria-label="Deleted projects">
        <button type="button" className="project-archive-toggle" aria-expanded={open} onClick={() => setOpen(value => !value)}>
          <span className={`expand-toggle ${open ? 'expanded' : ''}`} aria-hidden="true">▶</span>
          <span>Deleted projects</span>
          <span>{rows.length}</span>
        </button>
        {open && <>
          <p className="deleted-projects-help">
            Syncs and imports leave these projects out. Allow again lets the next sync, or an import
            of the project's files, add it as a new project. Its grades and measurements do not come back.
          </p>
          <div className="project-archive-list">
            {rows.map(({ catalog, deleted }) => (
              <div key={`${catalog.id}:${deleted.id}`} className="project-archive-item">
                <span className="deleted-project-text">
                  <strong>{deleted.name || `Project ${deleted.project_id}`}</strong>
                  <small>{catalog.name} · deleted {day(deleted.removed_at)} · {deleted.frames.toLocaleString()} frame{deleted.frames === 1 ? '' : 's'}</small>
                </span>
                <button
                  type="button"
                  className="library-row-button"
                  disabled={allow.isPending}
                  aria-label={`Allow ${deleted.name || `Project ${deleted.project_id}`} back on ${catalog.name}`}
                  title="Let the next sync, or an import of the project's files, add it as a new project"
                  onClick={() => { setNotice(''); allow.mutate({ catalog, deleted }); }}
                >
                  {allow.isPending && allow.variables?.deleted.id === deleted.id ? 'Allowing...' : 'Allow again'}
                </button>
              </div>
            ))}
          </div>
        </>}
      </section>
    )}
  </>;
}
