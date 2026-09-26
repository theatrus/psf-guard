import { useRef, useState, type FormEvent } from 'react';
import { useInfiniteQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useSearchParams } from 'react-router-dom';
import { Check, Pencil, Plus, RefreshCw, X } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorCollection, DirectorIdentity } from '../../api/directorTypes';
import { useAccess } from '../../auth/access';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import './DirectorPage.css';
import { identityId } from './identityId';
import DirectorCatalogs from './DirectorCatalogs';

const labels = { projects: 'Project', sites: 'Site', rigs: 'Rig' };
const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

interface Edit {
  record: DirectorIdentity;
  creating: boolean;
}

function Records({ collection, instanceId }: { collection: DirectorCollection; instanceId: string }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const queryKey = ['directorIdentities', instanceId, collection];
  const list = useInfiniteQuery({
    queryKey,
    queryFn: ({ pageParam }) => apiClient.getDirectorIdentities(collection, pageParam),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: page => page.next_after ?? undefined,
    refetchInterval: false,
    retry: false,
  });
  const [edit, setEdit] = useState<Edit | null>(null);
  const [name, setName] = useState('');
  const [notice, setNotice] = useState('');
  const [validation, setValidation] = useState('');
  const submitting = useRef(false);
  const save = useMutation({
    retry: false,
    mutationFn: async ({ record, creating, name }: Edit & { name: string }) => creating
      ? apiClient.createDirectorIdentity(collection, { id: record.id, name })
      : apiClient.renameDirectorIdentity(collection, record.id, { expected_revision: record.revision, name }),
    onSuccess: (record, variables) => {
      setNotice(`${variables.creating ? 'Created' : 'Renamed'} ${record.name}.`);
      setEdit(null);
      void client.invalidateQueries({ queryKey });
    },
    onSettled: () => { submitting.current = false; },
  });

  const begin = (next: Edit) => {
    save.reset();
    setValidation('');
    setNotice('');
    setName(next.record.name);
    setEdit(next);
  };
  const cancel = () => { setEdit(null); save.reset(); setValidation(''); };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!edit || !canWrite || submitting.current) return;
    const trimmed = name.trim();
    const hasControl = [...trimmed].some(char => char.charCodeAt(0) < 32 || (char.charCodeAt(0) >= 127 && char.charCodeAt(0) <= 159));
    if (!trimmed || new TextEncoder().encode(trimmed).length > 512 || hasControl) {
      setValidation('Enter a name of at most 512 UTF-8 bytes, without control characters.');
      return;
    }
    setValidation('');
    submitting.current = true;
    save.mutate({ ...edit, name: trimmed });
  };
  const records = list.data?.pages.flatMap(page => page.items) ?? [];
  const form = edit && canWrite && (
    <form className="director-edit" onSubmit={submit} aria-label={`${edit.creating ? 'New' : 'Rename'} ${labels[collection].toLowerCase()}`}>
      <label htmlFor="director-name">{labels[collection]} name</label>
      <div className="director-edit-controls">
        <input id="director-name" value={name} onChange={event => setName(event.target.value)} required maxLength={512} autoFocus disabled={save.isPending} />
        <button type="submit" disabled={save.isPending || !name.trim()}><Check size={16} />{save.isPending ? 'Saving...' : 'Save'}</button>
        <button type="button" onClick={cancel} disabled={save.isPending} title="Cancel" aria-label="Cancel"><X size={16} /></button>
      </div>
      {(validation || save.isError) && <p className="director-error" role="alert">{validation || message(save.error)}</p>}
    </form>
  );

  return (
    <section className="director-records" aria-label={`${labels[collection]} records`}>
      <div className="director-toolbar">
        <h2>{labels[collection]}s</h2>
        <div className="director-actions">
          <button type="button" title="Refresh records" aria-label="Refresh records" disabled={list.isFetching || !!edit} onClick={() => void list.refetch()}><RefreshCw size={16} /></button>
          {canWrite && <button type="button" disabled={!!edit} onClick={() => begin({ creating: true, record: { id: identityId(), name: '', revision: 1 } })}><Plus size={16} />New {labels[collection].toLowerCase()}</button>}
        </div>
      </div>
      {notice && <p role="status">{notice}</p>}
      {!canWrite && <p className="director-muted">Read only</p>}
      {edit?.creating && form}
      {list.isPending && <p role="status">Loading {collection}...</p>}
      {list.isError && <p className="director-error" role="alert">{message(list.error)}</p>}
      {list.isSuccess && records.length === 0 && <p className="director-muted">No {collection} yet.</p>}
      <ul className="director-list">
        {records.map(record => (
          <li key={record.id}>
            <div className="director-record">
              <div className="director-record-name"><strong>{record.name}</strong><code>{record.id}</code></div>
              <span className="director-muted">Revision {record.revision}</span>
              {canWrite && <button type="button" disabled={!!edit} title={`Rename ${record.name}`} aria-label={`Rename ${record.name}`} onClick={() => begin({ creating: false, record })}><Pencil size={16} /></button>}
            </div>
            {edit?.record.id === record.id && !edit.creating && form}
          </li>
        ))}
      </ul>
      {list.hasNextPage && <button type="button" disabled={list.isFetching || !!edit} onClick={() => void list.fetchNextPage()}>{list.isFetchingNextPage ? 'Loading...' : 'Load more'}</button>}
    </section>
  );
}

export default function DirectorPage() {
  const status = useDirectorStatus();
  const [params, setParams] = useSearchParams();
  const selected = params.get('directorView');
  const collection = selected === 'sites' || selected === 'rigs' || selected === 'catalogs' ? selected : 'projects';
  const available = status.data?.enabled && status.data.protocol_version === 1 && !!status.data.instance_id;
  return (
    <main className="director-page">
      <header className="director-heading"><h1>Director</h1><span className="director-preview">Experimental</span></header>
      {status.isPending && <p role="status">Loading Director...</p>}
      {status.isError && <div role="alert"><p>{message(status.error)}</p><button type="button" onClick={() => void status.refetch()}>Retry</button></div>}
      {status.data && !available && <p>Director management is unavailable on this server.</p>}
      {available && status.data && <>
        {!status.data.acquisition_available && <p className="director-muted">Acquisition is not yet available.</p>}
        <nav className="director-tabs" aria-label="Director views">
          {(['projects', 'sites', 'rigs', 'catalogs'] as const).map(value => <button type="button" key={value} aria-current={collection === value ? 'page' : undefined} onClick={() => {
            const next = new URLSearchParams(params);
            next.set('directorView', value);
            setParams(next);
          }}>{value === 'catalogs' ? 'Catalogs' : `${labels[value]}s`}</button>)}
        </nav>
        {collection === 'catalogs'
          ? <DirectorCatalogs key={status.data.instance_id} instanceId={status.data.instance_id!} />
          : <Records key={`${status.data.instance_id}:${collection}`} instanceId={status.data.instance_id!} collection={collection} />}
      </>}
    </main>
  );
}
