import { useEffect, useRef, useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Check, Plus, RefreshCw, X } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorIdentity, DirectorPlanLink, DirectorPlanRow } from '../../api/directorTypes';
import { useAccess } from '../../auth/access';
import { usePlans } from '../header/useCurrentPlan';
import { waitingPlanMatchesShow, type LibraryShow } from '../libraryShow';
import { identityId } from './identityId';
import { matchesSearch } from './planFilters';
import PlanRow from './PlanRow';
import PlanDeleteDialog from './PlanDeleteDialog';
import { coordinatorResult } from './collaborationLeaveText';
import { listNames } from './listNames';
import { CLOSED_PROJECT_STATE, INACTIVE_PROJECT_STATE } from '../../utils/projectNavigation';
import '../projectCard.css';
import './DirectorPage.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

/** The rigs where a plan's project is Closed, which Reopen sets Inactive. */
const closedLinks = (row: DirectorPlanRow): DirectorPlanLink[] =>
  row.links.filter(link => link.source_row_id !== null && link.source_state === CLOSED_PROJECT_STATE);

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
  // Reopen sets each rig's Closed project Inactive. Every rig is tried, so
  // one failure does not stop the rest, and the notice reports each outcome.
  const noticeRef = useRef<HTMLParagraphElement>(null);
  const focusNotice = useRef(false);
  useEffect(() => {
    if (!notice || !focusNotice.current) return;
    focusNotice.current = false;
    noticeRef.current?.focus();
  }, [notice]);
  // Deleting a plan deletes its projects in the rig databases: it needs
  // database management as well as an editor's account.
  const { data: serverInfo } = useQuery({ queryKey: ['serverInfo'], queryFn: apiClient.getServerInfo, staleTime: 5 * 60 * 1000 });
  const deleteAllowed = canWrite && (serverInfo?.allow_database_management ?? false);
  const [deleting, setDeleting] = useState<DirectorPlanRow | null>(null);
  const reopen = useMutation({
    retry: false,
    mutationFn: async (row: DirectorPlanRow) => {
      const closed = closedLinks(row);
      const results = await Promise.allSettled(closed.map(link =>
        apiClient.updateProject(link.catalog_slug, link.source_row_id!, { state: INACTIVE_PROJECT_STATE })));
      return closed.map((link, index) => ({ link, result: results[index] }));
    },
    onSuccess: (outcomes, row) => {
      const reopened = outcomes.filter(outcome => outcome.result.status === 'fulfilled').map(outcome => outcome.link.catalog_name);
      const failures = outcomes.flatMap(outcome => outcome.result.status === 'rejected'
        ? [`Could not reopen it on ${outcome.link.catalog_name}: ${message(outcome.result.reason)}`] : []);
      const done = reopened.length ? [`${row.project.name} is now Inactive on ${reopened.join(' and ')}.`] : [`Could not reopen ${row.project.name}.`];
      focusNotice.current = true;
      setNotice([...done, ...failures].join(' '));
    },
    // The plan list must show the new states before Reopen is offered again.
    onSettled: (_, __, row) => Promise.all([
      client.invalidateQueries({ queryKey: ['directorPlans'] }),
      ...row.links.map(link => client.invalidateQueries({ queryKey: ['db', link.catalog_slug] })),
    ]),
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
    <PlanRow row={row} href={plans.hrefFor(row)} canWrite={canWrite} editing={!!edit} onRename={() => begin({ creating: false, record: row.project })}
      reopenOn={closedLinks(row).map(link => link.catalog_name)} reopenDisabled={reopen.isPending}
      onReopen={() => { setNotice(''); reopen.mutate(row); }} reopening={reopen.isPending && reopen.variables?.project.id === row.project.id}
      onDelete={deleteAllowed ? () => { setNotice(''); setDeleting(row); } : undefined} />
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
    <p className="director-muted">Plans appear here until a rig captures the first frame. After that, they appear in the project list.</p>
    {plans.query.data?.warnings.map(warning => <p key={warning} className="director-error" role="alert">{warning}</p>)}
    {incomplete && <p className="director-muted">Some databases could not be read, so their plans are left out here.</p>}
    {notice && <p ref={noticeRef} tabIndex={-1} className="library-plans-notice" role="status">{notice}</p>}
    {edit?.creating && form}
    {plans.loading && <p role="status">Loading plans...</p>}
    {plans.error && <p className="director-error" role="alert">{message(plans.query.error)}</p>}
    {plans.query.isSuccess && shown.length === 0 && <p className="director-muted">{waiting.length === 0 ? 'Every plan has frames.' : 'No waiting plan matches.'}</p>}
    <ul className="director-plan-list is-compact">{shown.map(item)}</ul>
    {deleting && <PlanDeleteDialog
      planId={deleting.project.id}
      name={deleting.project.name}
      onClose={() => setDeleting(null)}
      onDeleted={(outcome) => {
        setDeleting(null);
        focusNotice.current = true;
        setNotice([
          outcome.databases.length
            ? `Deleted the plan ${outcome.name} and its project${outcome.databases.length === 1 ? '' : 's'} on ${listNames(outcome.databases)}.`
            : `Deleted the plan ${outcome.name}.`,
          outcome.coordinator ? `It left its collaboration. ${coordinatorResult(outcome.coordinator)}` : '',
        ].filter(Boolean).join(' '));
      }}
    />}
  </section>;
}
