import { Fragment, useMemo, useState } from 'react';
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { Check, Copy, Moon, Plus, Trash2 } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorLibraryTemplate, DirectorTemplate } from '../../api/directorTypes';
import { retryWhenBusy } from './retry';
import { newId } from './planModel';
import MoonSettings from './MoonSettings';
import { moonProblem } from './moonPolicy';
import './TemplateLibrary.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Template request failed';
type Draft = Omit<DirectorLibraryTemplate, 'bandpass'>;
const blank = (): Draft => ({ id: newId(), revision: 0, name: '', filter_name: '', gain: null, offset: null, bin: 1, readout_mode: null, default_exposure_seconds: 120, updated_at_ms: 0 });
const fromRig = (template: DirectorTemplate): Draft => ({ id: newId(), revision: 0, name: template.name, filter_name: template.filter_name, gain: template.gain, offset: template.offset, bin: template.bin, readout_mode: template.readout_mode, default_exposure_seconds: template.default_exposure > 0 ? template.default_exposure : 120, moon: template.moon, updated_at_ms: 0 });
const numberOrNull = (value: string): number | null => { const parsed = Number(value); return value.trim() === '' || !Number.isFinite(parsed) ? null : Math.round(parsed); };
const same = (a: Draft, b: Draft) => a.name === b.name && a.filter_name === b.filter_name && a.gain === b.gain && a.offset === b.offset && a.bin === b.bin && a.readout_mode === b.readout_mode && a.default_exposure_seconds === b.default_exposure_seconds && JSON.stringify(a.moon) === JSON.stringify(b.moon);

/** Director's exposure template library: settings any rig can shoot with.
 *  Rows are edited in place and saved one at a time; a rig's own templates
 *  can be copied in so every database offers the same choices. */
export default function TemplateLibrary() {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const library = useQuery({ queryKey: ['directorTemplateLibrary'], queryFn: apiClient.getDirectorTemplateLibrary, retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false });
  const rigs = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false });
  const rigList = useMemo(() => rigs.data ?? [], [rigs.data]);
  const [copyFrom, setCopyFrom] = useState('');
  const [moonEditor, setMoonEditor] = useState<string | null>(null);
  const rigTemplates = useQueries({ queries: rigList.map(rig => ({ queryKey: ['directorTemplates', rig.catalog_slug], queryFn: () => apiClient.getDirectorTemplates(rig.catalog_slug), enabled: rig.catalog_slug === copyFrom, retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false })) });
  const source = rigList.findIndex(rig => rig.catalog_slug === copyFrom);
  const copyable: DirectorTemplate[] = source >= 0 ? rigTemplates[source]?.data?.templates ?? [] : [];
  // Rows being edited, by id; a new row has revision 0 and no saved twin.
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [notice, setNotice] = useState('');
  const [problem, setProblem] = useState('');
  const saved = useMemo(() => library.data ?? [], [library.data]);
  const rows: Draft[] = useMemo(() => {
    const fresh = Object.values(drafts).filter(draft => draft.revision === 0 && !saved.some(t => t.id === draft.id));
    return [...saved.map(t => drafts[t.id] ?? t), ...fresh];
  }, [saved, drafts]);
  const edit = (id: string, patch: Partial<Draft>) => setDrafts(current => {
    const base = current[id] ?? saved.find(t => t.id === id);
    if (!base) return current;
    return { ...current, [id]: { ...base, ...patch } };
  });
  const forget = (id: string) => setDrafts(current => { const next = { ...current }; delete next[id]; return next; });
  const save = useMutation({
    retry: false,
    mutationFn: (draft: Draft) => apiClient.saveDirectorLibraryTemplate(draft),
    onSuccess: template => {
      setProblem(''); setNotice(`Saved ${template.name}.`);
      forget(template.id);
      client.setQueryData<DirectorLibraryTemplate[]>(['directorTemplateLibrary'], current => {
        const list = current ?? [];
        return list.some(t => t.id === template.id) ? list.map(t => t.id === template.id ? template : t) : [...list, template];
      });
    },
    onError: error => setProblem(message(error)),
  });
  const remove = useMutation({
    retry: false,
    mutationFn: (template: DirectorLibraryTemplate) => apiClient.deleteDirectorLibraryTemplate(template.id, template.revision),
    onSuccess: (_, template) => {
      setProblem(''); setNotice(`Removed ${template.name}.`);
      forget(template.id);
      client.setQueryData<DirectorLibraryTemplate[]>(['directorTemplateLibrary'], current => (current ?? []).filter(t => t.id !== template.id));
    },
    onError: error => setProblem(message(error)),
  });
  const problemWith = (draft: Draft): string | null => {
    if (!draft.name.trim()) return 'Name the template.';
    if (!draft.filter_name.trim()) return 'Name the filter as the rig does.';
    if (!(draft.default_exposure_seconds > 0)) return 'The exposure must be above zero seconds.';
    return moonProblem(draft.moon);
  };
  return <section aria-label="Exposure template library" className="director-records template-library">
    <div className="director-toolbar"><h2>Exposure templates</h2>
      {canWrite && <div className="director-actions">
        <button type="button" onClick={() => { const draft = blank(); setDrafts(current => ({ ...current, [draft.id]: draft })); }}><Plus size={16} />New template</button>
        {rigList.length > 0 && <label className="template-copy">Copy from
          <select aria-label="Copy templates from" value={copyFrom} onChange={event => setCopyFrom(event.target.value)}>
            <option value="">a rig's database…</option>
            {rigList.map(rig => <option key={rig.rig.id} value={rig.catalog_slug}>{rig.catalog_name}</option>)}
          </select></label>}
      </div>}
    </div>
    <p className="director-muted">Settings any rig can shoot with. A plan may bind a rig to one of these when its own database has no template for the band; activation writes the template into that database under the library's identity, so every rig ends up with the same one.</p>
    {library.isError && <p className="director-error" role="alert">{message(library.error)} <button type="button" onClick={() => void library.refetch()}>Retry</button></p>}
    {copyFrom && <div className="template-copy-list" role="group" aria-label="Templates to copy">
      {source >= 0 && rigTemplates[source]?.isPending && <p role="status">Loading templates...</p>}
      {copyable.length === 0 && source >= 0 && !rigTemplates[source]?.isPending && <p className="director-muted">That database has no exposure templates.</p>}
      {copyable.map(template => {
        const already = saved.some(t => t.name === template.name && t.filter_name.toLowerCase() === template.filter_name.toLowerCase());
        return <button key={template.id} type="button" disabled={already || save.isPending} title={already ? 'Already in the library' : `Copy ${template.name} into the library`}
          onClick={() => save.mutate(fromRig(template))}><Copy size={14} />{template.name} <small>({template.filter_name}{template.bin && template.bin > 1 ? `, ${template.bin}×${template.bin}` : ''}{already ? ', in the library' : ''})</small></button>;
      })}
    </div>}
    {rows.length === 0 && !library.isPending && <p className="director-muted">No library templates yet.</p>}
    {rows.length > 0 && <div className="director-table-scroll"><table className="template-table">
      <thead><tr><th>Name</th><th>Filter</th><th>Band</th><th>Gain</th><th>Offset</th><th>Bin</th><th>Readout</th><th>Exposure</th><th>Moon</th><th></th></tr></thead>
      <tbody>{rows.map(row => {
        const stored = saved.find(t => t.id === row.id);
        const dirty = !stored || !same(row, stored);
        const trouble = problemWith(row);
        return <Fragment key={row.id}><tr className={dirty ? 'is-dirty' : undefined}>
          <td><input aria-label="Template name" value={row.name} maxLength={256} disabled={!canWrite} onChange={event => edit(row.id, { name: event.target.value })} /></td>
          <td><input aria-label="Template filter" value={row.filter_name} maxLength={128} placeholder="Ha, L, OIII" disabled={!canWrite} onChange={event => edit(row.id, { filter_name: event.target.value })} /></td>
          <td className="director-muted">{stored && !dirty ? `${stored.bandpass.name}${stored.bandpass.kind === 'narrowband' ? ' (narrowband)' : ''}` : '—'}</td>
          <td><input aria-label="Template gain" type="number" min={0} step={1} value={row.gain ?? ''} disabled={!canWrite} onChange={event => edit(row.id, { gain: numberOrNull(event.target.value) })} /></td>
          <td><input aria-label="Template offset" type="number" min={0} step={1} value={row.offset ?? ''} disabled={!canWrite} onChange={event => edit(row.id, { offset: numberOrNull(event.target.value) })} /></td>
          <td><input aria-label="Template binning" type="number" min={1} max={8} step={1} value={row.bin ?? ''} disabled={!canWrite} onChange={event => edit(row.id, { bin: numberOrNull(event.target.value) })} /></td>
          <td><input aria-label="Template readout mode" type="number" min={0} step={1} value={row.readout_mode ?? ''} disabled={!canWrite} onChange={event => edit(row.id, { readout_mode: numberOrNull(event.target.value) })} /></td>
          <td><span className="plan-goal"><input aria-label="Template exposure seconds" type="number" min={1} step="any" value={row.default_exposure_seconds} disabled={!canWrite} onChange={event => edit(row.id, { default_exposure_seconds: Number(event.target.value) })} /><small>s</small></span></td>
          <td className="template-actions">
            <button type="button" aria-label={`Moon settings for ${row.name || 'template'}`} title="Moon avoidance" aria-expanded={moonEditor === row.id} onClick={() => setMoonEditor(moonEditor === row.id ? null : row.id)}><Moon size={16} />{row.moon?.enabled ? `${row.moon.separation_degrees}°` : 'Off'}</button>
          </td>
          <td className="template-actions">
            {canWrite && dirty && <button type="button" aria-label={`Save ${row.name || 'template'}`} title={trouble ?? 'Save'} disabled={!!trouble || save.isPending} onClick={() => save.mutate(row)}><Check size={16} /></button>}
            {canWrite && dirty && stored && <button type="button" aria-label={`Drop changes to ${stored.name}`} title="Drop changes" onClick={() => forget(row.id)}>Undo</button>}
            {canWrite && (stored
              ? <button type="button" aria-label={`Remove ${stored.name}`} title="Remove from the library" disabled={remove.isPending} onClick={() => remove.mutate(stored)}><Trash2 size={16} /></button>
              : <button type="button" aria-label="Discard new template" title="Discard" onClick={() => forget(row.id)}><Trash2 size={16} /></button>)}
          </td>
        </tr>{moonEditor === row.id && <tr><td colSpan={10}><MoonSettings value={row.moon} disabled={!canWrite || save.isPending} onChange={moon => edit(row.id, { moon })} /></td></tr>}</Fragment>;
      })}</tbody>
    </table></div>}
    {notice && <p role="status">{notice}</p>}
    {problem && <p className="director-error" role="alert">{problem}</p>}
  </section>;
}
