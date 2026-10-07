import { useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { Check, RefreshCw, Upload } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { CollaborationWork, CollaborationWorkInput, ContributionSelection } from '../../api/collaborationTypes';

export default function CollaborationReports({ connection, canWrite }: { connection: string; canWrite: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const inputs = useQuery({ queryKey: ['collaborationReportInputs', connection], enabled: expanded && canWrite, retry: false, queryFn: () => apiClient.collaborationWork(connection, { operation: 'report_inputs' }) });
  const [importId, setImport] = useState('');
  const [catalog, setCatalog] = useState('');
  const [panel, setPanel] = useState('');
  const [filter, setFilter] = useState('');
  const [target, setTarget] = useState('');
  const [revision, setRevision] = useState('');
  const [observingNight, setObservingNight] = useState('');
  const [selected, setSelected] = useState<string[]>([]);
  const [preview, setPreview] = useState<{ result: CollaborationWork; selection: ContributionSelection } | null>(null);
  const [notice, setNotice] = useState('');
  const candidates = useQuery({ queryKey: ['db', catalog, 'collaborationReportCandidates', connection, importId, observingNight], enabled: expanded && canWrite && !!importId && !!catalog && !!observingNight, retry: false, refetchInterval: 30_000,
    queryFn: () => apiClient.collaborationWork(connection, { operation: 'report_candidates', import_id: importId, catalog, observing_night: observingNight }) });
  const operation = useMutation({ retry: false, mutationFn: (input: CollaborationWorkInput) => apiClient.collaborationWork(connection, input),
    onMutate: () => { setPreview(null); setNotice(''); },
    onSuccess: (result, input) => {
      if (input.operation === 'preview_report') setPreview({ result, selection: input.selection });
      if (input.operation === 'queue_report') { setSelected([]); setNotice('Contribution queued for check-in'); }
    } });
  const reset = () => { setSelected([]); setPreview(null); setNotice(''); };
  const busy = !canWrite || operation.isPending;
  const imported = inputs.data?.imports?.find(i => i.id === importId);
  const revisions = [...new Set(candidates.data?.images?.map(i => i.source_digest).filter((v): v is string => !!v) ?? [])];
  const effectiveRevision = revision || revisions[0] || '';
  const panels = [...new Set([...(imported?.panels ?? []), ...(candidates.data?.images?.map(i => i.panel).filter((p): p is number => p != null) ?? [])])].sort((a, b) => a - b);
  const images = candidates.data?.images?.filter(i => (!filter || i.filter === filter) && (!target || i.target === target)
    && (i.panel == null || String(i.panel) === panel) && (!i.source_digest || i.source_digest === effectiveRevision)) ?? [];
  const selectedImages = selected.filter(guid => images.some(image => image.guid === guid));
  const selection = { import_id: importId, catalog, panel: Number(panel), image_guids: selectedImages, observing_night: observingNight, ...(effectiveRevision ? { source_digest: effectiveRevision } : {}) };
  const reviewed = !candidates.isError && preview?.selection.image_guids.every(guid => images.some(image => image.guid === guid)) ? preview : null;
  const measured = (value: number | null | undefined, unit: string) => value == null ? 'Unknown' : `${value.toFixed(2)} ${unit}`;
  return <section className="collaboration-reports" aria-label="Contribution reports">
    <div className="director-actions"><button type="button" disabled={!canWrite} onClick={() => setExpanded(v => !v)}><Upload size={16} />Contribution reports</button></div>
    {expanded && <>
      <fieldset disabled={busy}><legend>Saved images</legend><div className="rig-profile-grid">
        <label className="rig-profile-field"><span>Imported visit</span><select value={importId} onChange={e => { setImport(e.target.value); setPanel(''); setRevision(''); setObservingNight(inputs.data?.imports?.find(i => i.id === e.target.value)?.night ?? ''); reset(); }}><option value="">Select visit</option>{inputs.data?.imports?.map(i => <option key={i.id} value={i.id}>{i.name ?? i.id} ({i.night})</option>)}</select></label>
        <label className="rig-profile-field"><span>Observing night</span><input type="date" required value={observingNight} onChange={e => { setObservingNight(e.target.value); setRevision(''); reset(); }} /></label>
        <label className="rig-profile-field"><span>Rig database</span><select value={catalog} onChange={e => { setCatalog(e.target.value); setFilter(''); setTarget(''); setRevision(''); reset(); }}><option value="">Select database</option>{inputs.data?.catalogs?.map(c => <option key={c.id} value={c.id}>{c.name}</option>)}</select></label>
        <label className="rig-profile-field"><span>Remote panel</span><select value={panel} onChange={e => { setPanel(e.target.value); reset(); }}><option value="">Select panel</option>{panels.map(p => <option key={p} value={p}>{p}</option>)}</select></label>
        <label className="rig-profile-field"><span>Target</span><select value={target} onChange={e => { setTarget(e.target.value); reset(); }}><option value="">All targets</option>{[...new Set(candidates.data?.images?.map(i => i.target) ?? [])].map(t => <option key={t}>{t}</option>)}</select></label>
        <label className="rig-profile-field"><span>Filter</span><select value={filter} onChange={e => { setFilter(e.target.value); reset(); }}><option value="">All filters</option>{[...new Set(candidates.data?.images?.map(i => i.filter) ?? [])].map(f => <option key={f}>{f}</option>)}</select></label>
      </div><div className="director-actions">
        {revisions.length > 0 && <label>Assignment revision<select value={effectiveRevision} onChange={e => { setRevision(e.target.value); reset(); }}>{revisions.map(r => <option key={r} value={r}>{r.slice(0, 12)}</option>)}</select></label>}
        <button type="button" disabled={!images.length} onClick={() => { setSelected(images.map(i => i.guid)); setPreview(null); }}><Check size={16} />Select shown</button>
        <button type="button" disabled={!catalog || !importId} onClick={() => { reset(); void candidates.refetch(); }}><RefreshCw size={16} />Reload images</button>
      </div>
      {candidates.data && <p role="status">{images.length} accepted image{images.length === 1 ? '' : 's'}</p>}
      <div className="collaboration-image-list"><table><thead><tr><th /><th>Image</th><th>Target</th><th>Filter</th><th>Captured</th></tr></thead><tbody>{images.map(image => <tr key={image.guid}>
        <td><input type="checkbox" aria-label={`Select ${image.file}`} checked={selected.includes(image.guid)} onChange={e => { setSelected(all => e.target.checked ? [...all, image.guid] : all.filter(id => id !== image.guid)); setPreview(null); }} /></td>
        <td>{image.file}</td><td>{image.target}</td><td>{image.filter}</td><td>{new Date(image.captured_at * 1000).toLocaleString()}</td>
      </tr>)}</tbody></table></div>
      <button type="button" disabled={!selectedImages.length || !catalog || !importId || !observingNight || panel === '' || candidates.isFetching || candidates.isError} onClick={() => operation.mutate({ operation: 'preview_report', selection })}><Upload size={16} />Review {selectedImages.length} images</button>
      </fieldset>
      {(inputs.isFetching || candidates.isFetching || operation.isPending) && <p role="status">Loading contribution evidence...</p>}
      {[inputs.error, candidates.error, operation.error].filter(Boolean).map((error, i) => <p key={i} role="alert">{error!.message}</p>)}
      {notice && <p role="status">{notice}</p>}
      {reviewed?.result.report && <section aria-label="Review contribution"><h4>Review contribution</h4><p>{reviewed.result.report.frames} frames; {reviewed.result.report.seconds} seconds; {reviewed.result.report.filterName}; {reviewed.result.report.calibrated ? 'calibrated' : 'uncalibrated'}</p>
        <dl className="collaboration-evidence">
          <div><dt>Observing night</dt><dd>{reviewed.selection.observing_night}</dd></div>
          <div><dt>Exposure</dt><dd>{measured(reviewed.result.report.exposure, 's')}</dd></div>
          <div><dt>Image scale</dt><dd>{measured(reviewed.result.report.scale, 'arcsec/px')}</dd></div>
          <div><dt>Focal length</dt><dd>{measured(reviewed.result.report.focalLength, 'mm')}</dd></div>
          <div><dt>HFR</dt><dd>{measured(reviewed.result.report.hfr, 'arcsec')}</dd></div>
          <div><dt>Guiding RMS</dt><dd>{measured(reviewed.result.report.guideRms, 'arcsec')}</dd></div>
          <div><dt>Moon illumination</dt><dd>{measured(reviewed.result.report.moonIllumination == null ? null : reviewed.result.report.moonIllumination * 100, '%')}</dd></div>
          <div><dt>Moon separation</dt><dd>{measured(reviewed.result.report.moonSeparation, 'degrees')}</dd></div>
          <div><dt>Bandpass</dt><dd>{measured(reviewed.result.report.bandpass, 'nm')}</dd></div>
          <div><dt>Camera</dt><dd>{reviewed.result.report.colour == null ? 'Unknown' : reviewed.result.report.colour ? 'Colour' : 'Mono'}</dd></div>
        </dl>
        <p>Measured shared coverage: {reviewed.result.report.footprint.width.toFixed(3)} x {reviewed.result.report.footprint.height.toFixed(3)} degrees</p>
        <button type="button" disabled={busy || candidates.isFetching} onClick={() => operation.mutate({ operation: 'queue_report', selection: reviewed.selection, review_digest: reviewed.result.review_digest! })}><Upload size={16} />Queue finalized contribution</button>
      </section>}
    </>}
  </section>;
}
