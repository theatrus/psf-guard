import { useRef, useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from 'react-router-dom';
import { Check, Pencil, Plus, RefreshCw, Telescope, X } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorIdentity, DirectorPlanRow } from '../../api/directorTypes';
import { useAccess } from '../../auth/access';
import { identityId } from './identityId';
import { openSettings } from '../../utils/settingsIntent';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

interface Edit { record: DirectorIdentity; creating: boolean }

/** Where Rig planning opens for a row: its first linked database project. */
function planningHref(row: DirectorPlanRow): string | null {
  const link = row.links.find(entry => entry.source_row_id !== null);
  if (!link) return null;
  return `/director?${new URLSearchParams({ db: link.catalog_slug, project: String(link.source_row_id), dbfilter: link.catalog_slug, directorSource: link.catalog_slug, directorView: 'projects' })}`;
}

function stage(row: DirectorPlanRow): string {
  if (row.activation) return `Activated rev ${row.activation.revision} on ${new Date(row.activation.applied_at_ms).toLocaleDateString()}, ${row.activation.rigs} rig${row.activation.rigs === 1 ? '' : 's'}`;
  if (row.plan && row.plan.objectives > 0) return `Planned: ${row.plan.objectives} objective${row.plan.objectives === 1 ? '' : 's'}, ${row.plan.rigs} rig${row.plan.rigs === 1 ? '' : 's'}, not activated`;
  if (row.framing) return `Framed: ${row.framing.target_name || 'target'}, ${row.framing.panels} panel${row.framing.panels === 1 ? '' : 's'}`;
  return row.links.length ? 'Linked, not framed yet' : 'Not linked to any database';
}

/** Every global project with its links and how far its planning has come. */
export default function DirectorPlans({ instanceId }: { instanceId: string }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const queryKey = ['directorPlans', instanceId];
  const plans = useQuery({ queryKey, queryFn: apiClient.getDirectorPlans, retry: false, refetchOnWindowFocus: false, refetchOnMount: 'always' });
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
  const rows = plans.data ?? [];
  return <section className="director-records director-plans" aria-label="Plans">
    <div className="director-toolbar">
      <h2>Plans</h2>
      <div className="director-actions">
        <button type="button" title="Refresh records" aria-label="Refresh records" disabled={plans.isFetching || !!edit} onClick={() => void plans.refetch()}><RefreshCw size={16} /></button>
        {canWrite && <button type="button" disabled={!!edit} onClick={() => begin({ creating: true, record: { id: identityId(), name: '', revision: 1 } })}><Plus size={16} />New project</button>}
      </div>
    </div>
    <p className="director-muted">A plan is one project across every rig that shoots it. Frame, plan and activate it from Rig planning; link a database's projects to it under that database's project planning links.</p>
    {notice && <p role="status">{notice}</p>}
    {!canWrite && <p className="director-muted">Read only</p>}
    {edit?.creating && form}
    {plans.isPending && <p role="status">Loading plans...</p>}
    {plans.isError && <p className="director-error" role="alert">{message(plans.error)}</p>}
    {plans.isSuccess && rows.length === 0 && <p className="director-muted">No projects yet.</p>}
    <ul className="director-list">
      {rows.map(row => {
        const href = planningHref(row);
        return <li key={row.project.id}>
          <div className="director-plan">
            <div className="director-record-name">
              <strong>{row.project.name}</strong>
              <span className="director-muted">{stage(row)}</span>
              {row.links.length > 0 && <span className="director-plan-links">{row.links.map(link => <span key={`${link.catalog_slug}:${link.source_project_guid}`}>{link.catalog_name}{link.source_name ? `: ${link.source_name}` : ''}</span>)}</span>}
            </div>
            <div className="director-actions">
              {href ? <Link to={href}><Telescope size={16} />Rig planning</Link>
                : <button type="button" onClick={() => openSettings()}>Link a database</button>}
              {canWrite && <button type="button" disabled={!!edit} title={`Rename ${row.project.name}`} aria-label={`Rename ${row.project.name}`} onClick={() => begin({ creating: false, record: row.project })}><Pencil size={16} /></button>}
            </div>
          </div>
          {edit?.record.id === row.project.id && !edit.creating && form}
        </li>;
      })}
    </ul>
  </section>;
}
