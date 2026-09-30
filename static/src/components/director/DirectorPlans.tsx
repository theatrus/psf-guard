import { useRef, useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useSearchParams } from 'react-router-dom';
import { Check, Plus, RefreshCw, X } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorIdentity, DirectorPlanRow } from '../../api/directorTypes';
import { useAccess } from '../../auth/access';
import { identityId } from './identityId';
import { retryWhenBusy } from './retry';
import PlanCard from './PlanCard';
import PlanRow from './PlanRow';
import { setDisplayPreferences, useDisplayPreferences } from '../../hooks/useDisplayPreferences';
import { useFoldedRows } from '../../hooks/useFoldedRows';
import { DensityToggle, FoldButton } from '../projectPills';
import { isArchivedPlan } from './planCardModel';
import '../projectCard.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

interface Edit { record: DirectorIdentity; creating: boolean }

/** Every global project with its links and how far its planning has come. */
export default function DirectorPlans({ instanceId }: { instanceId: string }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const [params] = useSearchParams();
  const queryKey = ['directorPlans', instanceId];
  // The workspace keeps the page's catalog scope so Overview returns where it was.
  const workspaceHref = (projectId: string) => {
    const next = new URLSearchParams(params);
    next.delete('directorSource'); next.delete('directorView'); next.delete('directorCatalog');
    next.set('directorProject', projectId);
    return `/director?${next}`;
  };
  const plans = useQuery({ queryKey, queryFn: apiClient.getDirectorPlans, retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false, refetchOnMount: 'always' });
  // The same Compact / Detailed choice as the Library, and the same folding.
  const displayPreferences = useDisplayPreferences();
  const density = displayPreferences.libraryDensity;
  const folds = useFoldedRows(density);
  const chooseDensity = (next: typeof density) => { setDisplayPreferences({ ...displayPreferences, libraryDensity: next }); folds.reset(); };
  const nowMs = Date.now();
  // Closed everywhere it is shot: behind the same fold the Library uses.
  const [archivedOpen, setArchivedOpen] = useState(false);
  const [edit, setEdit] = useState<Edit | null>(null);
  const [name, setName] = useState('');
  const [notice, setNotice] = useState('');
  const [validation, setValidation] = useState('');
  const submitting = useRef(false);
  const save = useMutation({
    retry: false,
    mutationFn: async ({ record, creating, name }: Edit & { name: string }) => creating
      ? apiClient.createDirectorIdentity('projects', { id: record.id, name })
      : apiClient.renameDirectorIdentity('projects', record.id, { expected_revision: record.revision, name }),
    onSuccess: (record, variables) => {
      setNotice(`${variables.creating ? 'Created' : 'Renamed'} ${record.name}.`);
      setEdit(null);
      void client.invalidateQueries({ queryKey });
    },
    onSettled: () => { submitting.current = false; },
  });
  const begin = (next: Edit) => { save.reset(); setValidation(''); setNotice(''); setName(next.record.name); setEdit(next); };
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
  const form = edit && canWrite && (
    <form className="director-edit" onSubmit={submit} aria-label={`${edit.creating ? 'New' : 'Rename'} project`}>
      <label htmlFor="director-name">Project name</label>
      <div className="director-edit-controls">
        <input id="director-name" value={name} onChange={event => setName(event.target.value)} required maxLength={512} autoFocus disabled={save.isPending} />
        <button type="submit" disabled={save.isPending || !name.trim()}><Check size={16} />{save.isPending ? 'Saving...' : 'Save'}</button>
        <button type="button" onClick={cancel} disabled={save.isPending} title="Cancel" aria-label="Cancel"><X size={16} /></button>
      </div>
      {(validation || save.isError) && <p className="director-error" role="alert">{validation || message(save.error)}</p>}
    </form>
  );
  const rows = plans.data?.rows ?? [];
  const live = rows.filter(row => !isArchivedPlan(row));
  const archived = rows.filter(isArchivedPlan);
  const item = (row: DirectorPlanRow) => {
    const key = row.project.id;
    const href = workspaceHref(key);
    return <li key={key} className="director-plan">
      {folds.showsCard(key)
        ? <PlanCard row={row} href={href} canWrite={canWrite} editing={!!edit} onRename={() => begin({ creating: false, record: row.project })}
            fold={<FoldButton open={true} name={row.project.name} onClick={() => folds.toggle(key)} />} />
        : <PlanRow row={row} href={href} nowMs={nowMs} onOpen={() => folds.toggle(key)} canWrite={canWrite} editing={!!edit} onRename={() => begin({ creating: false, record: row.project })} />}
      {edit?.record.id === key && !edit.creating && form}
    </li>;
  };
  return <section className="director-records director-plans" aria-label="Plans">
    <div className="director-toolbar">
      <h2>Plans</h2>
      <div className="director-actions">
        <DensityToggle density={density} onChange={chooseDensity} />
        <button type="button" title="Refresh records" aria-label="Refresh records" disabled={plans.isFetching || !!edit} onClick={() => void plans.refetch()}><RefreshCw size={16} /></button>
        {canWrite && <button type="button" disabled={!!edit} onClick={() => begin({ creating: true, record: { id: identityId(), name: '', revision: 1 } })}><Plus size={16} />New project</button>}
      </div>
    </div>
    <p className="director-muted">Every project in every registered database is a plan; projects that share a Target Scheduler GUID across databases are one plan. Open a plan to frame it, choose the rigs that shoot it, and activate.</p>
    {plans.data?.warnings.map(warning => <p key={warning} className="director-error" role="alert">{warning}</p>)}
    {notice && <p role="status">{notice}</p>}
    {!canWrite && <p className="director-muted">Read only</p>}
    {edit?.creating && form}
    {plans.isPending && <p role="status">Loading plans...</p>}
    {plans.isError && <p className="director-error" role="alert">{message(plans.error)}</p>}
    {plans.isSuccess && rows.length === 0 && <p className="director-muted">No projects yet.</p>}
    <ul className={`director-plan-list${density === 'compact' ? ' is-compact' : ''}`}>{live.map(item)}</ul>
    {archived.length > 0 && (
      <section className="project-archive">
        <button type="button" className="project-archive-toggle" onClick={() => setArchivedOpen(open => !open)} aria-expanded={archivedOpen}>
          <span className={`expand-toggle ${archivedOpen ? 'expanded' : ''}`} aria-hidden="true">▶</span>
          <span>Archived plans</span>
          <span>{archived.length}</span>
        </button>
        {archivedOpen && <ul className={`director-plan-list${density === 'compact' ? ' is-compact' : ''}`}>{archived.map(item)}</ul>}
      </section>
    )}
  </section>;
}
