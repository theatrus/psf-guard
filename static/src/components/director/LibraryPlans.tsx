import { useRef, useState, type FormEvent } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Check, Plus, RefreshCw, X } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorIdentity, DirectorPlanRow } from '../../api/directorTypes';
import { useAccess } from '../../auth/access';
import { usePlans } from '../header/useCurrentPlan';
import { waitingPlanMatchesShow, type LibraryShow } from '../libraryShow';
import { identityId } from './identityId';
import { matchesSearch } from './planFilters';
import PlanRow from './PlanRow';
import '../projectCard.css';
import './DirectorPage.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

interface Edit { record: DirectorIdentity; creating: boolean }

/** Planning's part of the Library: plans with nothing captured yet, with
 *  the way to start one and to rename one. The Library lists projects that
 *  have frames; a plan framed before any rig takes it, or activated on rigs
 *  that have not captured yet, has no row there, so it waits here until its
 *  first frame arrives. `listed` holds every project row the Library has,
 *  as `slug:id`, whatever its filters show. */
export default function LibraryPlans({ search, show, listed, dbFilter = null, incomplete = false }: { search: string; show: LibraryShow; listed: ReadonlySet<string>; dbFilter?: string | null; incomplete?: boolean }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const plans = usePlans();
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
      void client.invalidateQueries({ queryKey: ['directorPlans'] });
    },
    onSettled: () => { submitting.current = false; },
  });
  if (!plans.enabled) return null;
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
    <form className="director-edit" onSubmit={submit} aria-label={`${edit.creating ? 'New' : 'Rename'} plan`}>
      <label htmlFor="director-name">Plan name</label>
      <div className="director-edit-controls">
        <input id="director-name" value={name} onChange={event => setName(event.target.value)} required maxLength={512} autoFocus disabled={save.isPending} />
        <button type="submit" disabled={save.isPending || !name.trim()}><Check size={16} />{save.isPending ? 'Saving...' : 'Save'}</button>
        <button type="button" onClick={cancel} disabled={save.isPending} title="Cancel" aria-label="Cancel"><X size={16} /></button>
      </div>
      {(validation || save.isError) && <p className="director-error" role="alert">{validation || message(save.error)}</p>}
    </form>
  );
  // When a database's projects could not be read, `listed` is short and a
  // linked plan would look empty; only plans with no database are certain.
  const waiting = plans.rows.filter(row => (!incomplete || row.links.length === 0)
    && row.links.every(link => link.source_row_id === null || !listed.has(`${link.catalog_slug}:${link.source_row_id}`)));
  const shown = waiting.filter(row => waitingPlanMatchesShow(row.links, show) && matchesSearch(row, search)
    && (!dbFilter || row.links.some(link => link.catalog_slug === dbFilter)));
  // Nothing to show and nothing to do: stay out of the Library.
  if (!canWrite && shown.length === 0 && !plans.error && !plans.query.data?.warnings.length) return null;
  const item = (row: DirectorPlanRow) => <li key={row.project.id} className="director-plan">
    <PlanRow row={row} href={plans.hrefFor(row)} canWrite={canWrite} editing={!!edit} onRename={() => begin({ creating: false, record: row.project })} />
    {edit?.record.id === row.project.id && !edit.creating && form}
  </li>;
  return <section className="director-page director-embedded library-plans" aria-label="Plans with nothing captured yet">
    <div className="director-toolbar">
      <h3>Plans with nothing captured yet</h3>
      <div className="director-actions">
        <button type="button" title="Refresh plans" aria-label="Refresh plans" disabled={plans.query.isFetching || !!edit} onClick={() => void plans.query.refetch()}><RefreshCw size={16} /></button>
        {canWrite && <button type="button" disabled={!!edit} onClick={() => begin({ creating: true, record: { id: identityId(), name: '', revision: 1 } })}><Plus size={16} />New plan</button>}
      </div>
    </div>
    <p className="director-muted">A plan waits here from its framing until a rig captures its first frame, then joins the projects above.</p>
    {plans.query.data?.warnings.map(warning => <p key={warning} className="director-error" role="alert">{warning}</p>)}
    {incomplete && <p className="director-muted">Some databases could not be read, so their plans are left out here.</p>}
    {notice && <p role="status">{notice}</p>}
    {edit?.creating && form}
    {plans.loading && <p role="status">Loading plans...</p>}
    {plans.error && <p className="director-error" role="alert">{message(plans.query.error)}</p>}
    {plans.query.isSuccess && shown.length === 0 && <p className="director-muted">{waiting.length === 0 ? 'Every plan has frames.' : 'No waiting plan matches.'}</p>}
    <ul className="director-plan-list is-compact">{shown.map(item)}</ul>
  </section>;
}
