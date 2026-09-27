import { useEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useSearchParams } from 'react-router-dom';
import { isAxiosError } from 'axios';
import { Check, Eye, Plus, RefreshCw, X } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAllDatabases } from '../../hooks/useDatabases';
import { useAccess } from '../../auth/access';
import type { DirectorAdoptionPlan, DirectorAdoptionReport, DirectorIdentity } from '../../api/directorTypes';
import { identityId } from './identityId';
import { loadCatalog, type CatalogData } from './catalogData';

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'Director request failed';
const issueNames: Record<string, string> = {
  missing_project_guid: 'Missing project GUID', invalid_project_guid: 'Invalid project GUID',
  duplicate_project_guid: 'Duplicate project GUID', missing_profile_id: 'Missing profile ID',
  invalid_profile_id: 'Invalid profile ID', invalid_project_name: 'Invalid project name',
};
type NewIdentity = { collection: 'projects' | 'rigs'; id: string; guid: string; profile: string };

function MappingForm({ data, refreshing, onBusy, onApplied }: {
  data: CatalogData; refreshing: boolean; onBusy: (busy: boolean) => void; onApplied: () => void;
}) {
  const { canWrite } = useAccess();
  const [catalogId] = useState(() => data.discovery.catalog_identity?.id ?? identityId());
  const [selected, setSelected] = useState<Record<string, boolean>>({});
  const [projectIds, setProjectIds] = useState<Record<string, string>>({});
  const [rigIds, setRigIds] = useState<Record<string, string>>(() => Object.fromEntries(data.mappings.map(m => [m.source_profile_id, m.rig_id])));
  const [extraProjects, setExtraProjects] = useState<DirectorIdentity[]>([]);
  const [extraRigs, setExtraRigs] = useState<DirectorIdentity[]>([]);
  const [creating, setCreating] = useState<NewIdentity | null>(null);
  const [name, setName] = useState('');
  const [review, setReview] = useState<{ plan: DirectorAdoptionPlan; report: DirectorAdoptionReport } | null>(null);
  const [busy, setBusy] = useState(false);
  const active = useRef(false);
  const [error, setError] = useState('');
  const [page, setPage] = useState(0);
  const projects = [...new Map([...data.projects, ...extraProjects].map(record => [record.id, record])).values()];
  const rigs = [...new Map([...data.rigs, ...extraRigs].map(record => [record.id, record])).values()];
  const existing = new Map(data.mappings.map(m => [m.source_project_guid, m]));
  const rows = data.discovery.evidence.projects;
  const linkedCount = rows.filter(row => row.issues.length === 0 && row.source_project_guid
    && existing.get(row.source_project_guid)?.source_profile_id === row.source_profile_id).length;
  const count = Object.values(selected).filter(Boolean).length;
  const locked = busy || refreshing || !!review || !canWrite;
  useEffect(() => { onBusy(busy); return () => onBusy(false); }, [busy, onBusy]);

  const run = async (operation: () => Promise<void>) => {
    if (active.current || !canWrite || refreshing) return;
    active.current = true;
    setBusy(true); setError('');
    try { await operation(); } catch (cause) {
      setError(message(cause));
      const httpError = isAxiosError(cause) ? cause
        : cause instanceof Error && isAxiosError(cause.cause) ? cause.cause : null;
      if (httpError?.response?.status === 409) setReview(null);
    } finally { active.current = false; setBusy(false); }
  };
  const preview = () => run(async () => {
    const mappings = rows.filter(row => row.source_project_guid && selected[row.source_project_guid]).map(row => ({
      catalog_id: catalogId, source_project_guid: row.source_project_guid!, source_profile_id: row.source_profile_id!,
      project_id: projectIds[row.source_project_guid!], rig_id: rigIds[row.source_profile_id!],
    }));
    if (!mappings.length || mappings.some(m => !m.project_id || !m.rig_id)) throw new Error('Choose a global project and rig for every selected project.');
    const plan = { catalog_id: catalogId, mappings };
    const report = await apiClient.previewDirectorAdoption(data.discovery.catalog_slug, plan);
    setReview({ plan, report });
  });
  const apply = () => run(async () => {
    if (!review) return;
    await apiClient.applyDirectorAdoption(data.discovery.catalog_slug, review.plan, review.report.preview_digest);
    setReview(null); onApplied();
  });
  const begin = (collection: NewIdentity['collection'], guid: string, profile: string, initialName = '') => {
    setCreating({ collection, guid, profile, id: identityId() }); setName(initialName); setError('');
  };
  const create = () => run(async () => {
    if (!creating) return;
    const trimmed = name.trim();
    if (!trimmed || new TextEncoder().encode(trimmed).length > 512) throw new Error('Enter a name of at most 512 UTF-8 bytes.');
    const record = await apiClient.createDirectorIdentity(creating.collection, { id: creating.id, name: trimmed });
    if (creating.collection === 'projects') {
      setExtraProjects(values => [...values, record]); setProjectIds(values => ({ ...values, [creating.guid]: record.id }));
    } else {
      setExtraRigs(values => [...values, record]); setRigIds(values => ({ ...values, [creating.profile]: record.id }));
    }
    setCreating(null);
  });
  const label = (record: DirectorIdentity) => `${record.name} (${record.id.slice(0, 8)})`;

  return <section aria-label="Catalog mappings" className="director-mappings">
    <div className="director-toolbar"><h3>{data.discovery.catalog_name}</h3><span className="director-muted">{linkedCount} linked / {rows.length} source projects</span></div>
    {!canWrite && <p className="director-muted">Read only</p>}
    {error && <p role="alert" className="director-error">{error}</p>}
    {creating && <form className="director-edit" onSubmit={event => { event.preventDefault(); void create(); }}>
      <label htmlFor="director-create-name">New {creating.collection === 'projects' ? 'global project' : 'rig'} name</label>
      <div className="director-edit-controls"><input id="director-create-name" autoFocus value={name} maxLength={512} disabled={busy} onChange={event => setName(event.target.value)} />
        <button type="submit" disabled={busy || !name.trim()}><Check size={16} />{busy ? 'Creating...' : 'Create'}</button>
        <button type="button" aria-label="Cancel creation" title="Cancel creation" disabled={busy} onClick={() => setCreating(null)}><X size={16} /></button>
      </div>
    </form>}
    {review ? <section aria-label="Mapping review">
      <h3>Review {review.report.mappings.length} mappings</h3>
      <div className="director-table-scroll"><table className="director-review-table"><thead><tr><th>Source project</th><th>Global project</th><th>Rig</th></tr></thead><tbody>
        {review.report.mappings.map(row => <tr key={row.mapping.source_project_guid}><td><span className="director-cell-label">Source project</span>{row.source_name}<code>{row.mapping.source_project_guid}</code></td><td><span className="director-cell-label">Global project</span>{row.project.name}<code>{row.project.id}</code></td><td><span className="director-cell-label">Rig</span>{row.rig.name}<code>{row.rig.id}</code></td></tr>)}
      </tbody></table></div>
      <div className="director-actions"><button type="button" disabled={busy || !canWrite} onClick={() => void apply()}><Check size={16} />{busy ? 'Applying...' : 'Apply mappings'}</button><button type="button" disabled={busy} onClick={() => setReview(null)}>Edit choices</button></div>
    </section> : <>
      <div className="director-table-scroll"><table className="director-mapping-table"><thead><tr><th aria-label="Selected" /><th>Source project / profile</th><th>Global project</th><th>Rig</th><th>Status</th></tr></thead><tbody>
        {rows.slice(page * 50, (page + 1) * 50).map(row => {
          const guid = row.source_project_guid ?? '';
          const profile = row.source_profile_id ?? '';
          const mapped = existing.get(guid);
          const valid = !!guid && !!profile && row.issues.length === 0;
          const sourceName = row.name ?? `Row ${row.source_row_id}`;
          return <tr key={`${row.source_row_id}:${guid}`}>
            <td><input type="checkbox" aria-label={`Select ${sourceName} (${row.source_row_id})`} checked={!!selected[guid]} disabled={locked || !!creating || !!mapped || !valid || (!selected[guid] && count >= 256)} onChange={event => setSelected(values => ({ ...values, [guid]: event.target.checked }))} /></td>
            <td><strong>{sourceName}</strong><code>{profile || 'No profile'}</code></td>
            <td><span className="director-cell-label">Global project</span>{mapped ? projects.find(p => p.id === mapped.project_id)?.name ?? mapped.project_id : <div className="director-mapping-choice">
              <select aria-label={`Global project for ${sourceName} (${row.source_row_id})`} value={projectIds[guid] ?? ''} disabled={locked || !!creating || !valid} onChange={event => setProjectIds(values => ({ ...values, [guid]: event.target.value }))}><option value="">Choose project</option>{projects.map(p => <option key={p.id} value={p.id}>{label(p)}</option>)}</select>
              {canWrite && <button type="button" title={`New global project for ${sourceName}`} aria-label={`New global project for ${sourceName}`} disabled={locked || !!creating || !valid} onClick={() => begin('projects', guid, profile, sourceName)}><Plus size={16} /></button>}
            </div>}</td>
            <td><span className="director-cell-label">Rig</span>{mapped ? rigs.find(r => r.id === mapped.rig_id)?.name ?? mapped.rig_id : <div className="director-mapping-choice">
              <select aria-label={`Rig for ${sourceName} (${row.source_row_id})`} value={rigIds[profile] ?? ''} disabled={locked || !!creating || !valid || data.mappings.some(m => m.source_profile_id === profile)} onChange={event => setRigIds(values => ({ ...values, [profile]: event.target.value }))}><option value="">Choose rig</option>{rigs.map(r => <option key={r.id} value={r.id}>{label(r)}</option>)}</select>
              {canWrite && <button type="button" title={`New rig for ${sourceName}`} aria-label={`New rig for ${sourceName}`} disabled={locked || !!creating || !valid || data.mappings.some(m => m.source_profile_id === profile)} onClick={() => begin('rigs', guid, profile)}><Plus size={16} /></button>}
            </div>}</td>
            <td><span className="director-cell-label">Status</span><span>{row.issues.length ? row.issues.map(issue => issueNames[issue] ?? issue).join(', ')
              : mapped ? mapped.source_profile_id === profile ? 'Linked' : 'Source profile changed' : 'Unmapped'}</span></td>
          </tr>;
        })}
      </tbody></table></div>
      {!rows.length && <p className="director-muted">No source projects.</p>}
      <div className="director-toolbar"><span>{count} selected</span><div className="director-actions">
        {rows.length > 50 && <><button type="button" disabled={page === 0 || busy} onClick={() => setPage(value => value - 1)}>Previous</button><span>Page {page + 1} / {Math.ceil(rows.length / 50)}</span><button type="button" disabled={(page + 1) * 50 >= rows.length || busy} onClick={() => setPage(value => value + 1)}>Next</button></>}
        {canWrite && <button type="button" disabled={locked || !!creating || !count} onClick={() => void preview()}><Eye size={16} />{busy && !creating ? 'Preparing preview...' : 'Preview mappings'}</button>}
      </div></div>
    </>}
  </section>;
}

export default function DirectorCatalogs({ instanceId }: { instanceId: string }) {
  const databases = useAllDatabases();
  const [params, setParams] = useSearchParams();
  const slug = params.get('directorCatalog') ?? '';
  const currentSlug = useRef(slug);
  currentSlug.current = slug;
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState('');
  const available = databases.data?.some(db => db.id === slug) ?? false;
  const loaded = useQuery({ queryKey: ['directorCatalog', instanceId, slug], queryFn: () => loadCatalog(slug), enabled: available,
    retry: false, refetchOnWindowFocus: false });
  const data = loaded.data;
  const key = data ? `${slug}:${data.discovery.snapshot_digest}:${JSON.stringify(data.mappings)}` : slug;
  return <section aria-label="Director catalogs">
    <div className="director-toolbar"><label className="director-catalog-select">Catalog<select value={available ? slug : ''} disabled={busy || databases.isPending} onChange={event => {
      const next = new URLSearchParams(params); next.set('directorCatalog', event.target.value); setParams(next); setNotice('');
    }}><option value="">Choose catalog</option>{databases.data?.map(db => <option key={db.id} value={db.id}>{db.name}</option>)}</select></label>
      <button type="button" aria-label="Refresh catalog" title="Refresh catalog" disabled={!available || busy || loaded.isFetching} onClick={() => { setNotice(''); void loaded.refetch(); }}><RefreshCw size={16} /></button>
    </div>
    {notice && <p role="status">{notice}</p>}
    {databases.isError && <div role="alert"><p className="director-error">{message(databases.error)}</p><button type="button" onClick={() => void databases.refetch()}><RefreshCw size={16} />Retry catalog list</button></div>}
    {databases.data?.length === 0 && <p className="director-muted">No configured catalogs.</p>}
    {slug && databases.isSuccess && !available && <p role="alert">Catalog is not configured on this server.</p>}
    {available && loaded.isFetching && <p role="status">Loading catalog mappings...</p>}
    {loaded.isError && <p className="director-error" role="alert">{message(loaded.error)}</p>}
    {data && !loaded.isError && available && <MappingForm key={key} data={data} refreshing={loaded.isFetching} onBusy={setBusy} onApplied={() => {
      if (currentSlug.current !== slug) return;
      setNotice('Mappings saved.'); void loaded.refetch();
    }} />}
  </section>;
}
