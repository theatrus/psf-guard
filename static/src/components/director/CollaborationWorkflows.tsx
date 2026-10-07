import { useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { Download, Link2, Plus, RefreshCw, Save, Upload, X } from 'lucide-react';
import { Link } from 'react-router-dom';
import { apiClient } from '../../api/client';
import type { CollaborationConnection, CollaborationSettings, CollaborationWork, CollaborationWorkInput } from '../../api/collaborationTypes';
import CollaborationReports from './CollaborationReports';

export default function CollaborationWorkflows({ connection, canWrite, refresh }: { connection: CollaborationConnection; canWrite: boolean; refresh: () => void }) {
  const initial = connection.binding.settings;
  const [binning, setBinning] = useState(initial?.binning ?? 1);
  const [colour, setColour] = useState(initial?.colour ?? false);
  const [hours, setHours] = useState(initial?.hours_per_night ?? 6);
  const [status, setStatus] = useState(initial?.share_status ?? false);
  const [filters, setFilters] = useState(() => Object.entries(initial?.filters ?? {}).map(([name, v]) => ({ name, seconds: v.exposure_seconds, bandpass: v.bandpass_nm?.toString() ?? '' })));
  const [night, setNight] = useState('');
  const [moon, setMoon] = useState('');
  const [moonUp, setMoonUp] = useState('');
  const [work, setWork] = useState<CollaborationWork | null>(null);
  const [preview, setPreview] = useState<{ result: CollaborationWork; input: { task: string; night: { night: string; moon: number; moon_up: number } } } | null>(null);
  const [notice, setNotice] = useState('');
  const mutation = useMutation({
    retry: false,
    mutationFn: (input: CollaborationWorkInput) => apiClient.collaborationWork(connection.binding.id, input),
    onMutate: () => { setNotice(''); setPreview(null); },
    onSuccess: (result, input) => {
      if (input.operation === 'configure') { setNotice('Rig profile saved'); refresh(); }
      else if (input.operation === 'preview') setPreview({ result, input: { task: input.task, night: input.night } });
      else if (input.operation === 'apply') { setNotice('Imported as an inactive project draft'); refresh(); }
      else if (input.operation === 'checkin') setNotice(`${result.delivered ?? 0} reports delivered; ${result.accepted ?? 0} accepted; ${result.rejected ?? 0} rejected`);
      else setWork(result);
    },
  });
  const busy = !canWrite || mutation.isPending;
  const nightValid = /^\d{4}-\d{2}-\d{2}$/.test(night) && moon !== '' && moonUp !== '' && [Number(moon), Number(moonUp)].every(v => Number.isFinite(v) && v >= 0 && v <= 100);
  const observingNight = { night, moon: Number(moon) / 100, moon_up: Number(moonUp) / 100 };
  const save = () => {
    const settings: CollaborationSettings = { binning, colour, hours_per_night: hours, share_status: status,
      filters: Object.fromEntries(filters.map(f => [f.name.trim(), { exposure_seconds: f.seconds, bandpass_nm: f.bandpass === '' ? null : Number(f.bandpass) }])) };
    mutation.mutate({ operation: 'configure', settings });
  };
  return <div className="collaboration-workflows">
    <form className="rig-profile-form" onSubmit={event => { event.preventDefault(); save(); }}>
      <fieldset disabled={busy}><legend>Collaboration rig profile</legend>
        <div className="rig-profile-grid">
          <label className="rig-profile-field"><span>Binning</span><input type="number" min="1" max="16" required value={binning} onChange={e => setBinning(Number(e.target.value))} /></label>
          <label className="rig-profile-field"><span>Hours per night</span><input type="number" min="0.01" max="24" step="0.01" required value={hours} onChange={e => setHours(Number(e.target.value))} /></label>
        </div>
        <label><input type="checkbox" checked={colour} onChange={e => setColour(e.target.checked)} />Colour camera</label>
        <label><input type="checkbox" checked={status} onChange={e => setStatus(e.target.checked)} />Share current activity</label>
        <table><thead><tr><th>Filter</th><th>Exposure (s)</th><th>Bandpass (nm)</th><th /></tr></thead><tbody>
          {filters.map((filter, index) => <tr key={index}>
            <td><input aria-label={`Filter ${index + 1}`} required maxLength={80} value={filter.name} onChange={e => setFilters(all => all.map((f, i) => i === index ? { ...f, name: e.target.value } : f))} /></td>
            <td><input aria-label={`Exposure ${index + 1}`} type="number" required min="0.001" max="86400" step="0.001" value={filter.seconds} onChange={e => setFilters(all => all.map((f, i) => i === index ? { ...f, seconds: Number(e.target.value) } : f))} /></td>
            <td><input aria-label={`Bandpass ${index + 1}`} type="number" min="0.01" max="1000000" step="0.01" value={filter.bandpass} onChange={e => setFilters(all => all.map((f, i) => i === index ? { ...f, bandpass: e.target.value } : f))} /></td>
            <td><button type="button" title="Remove filter" aria-label={`Remove filter ${index + 1}`} onClick={() => setFilters(all => all.filter((_, i) => i !== index))}><X size={16} /></button></td>
          </tr>)}
        </tbody></table>
        <div className="director-actions"><button type="button" disabled={filters.length >= 32} onClick={() => setFilters(all => [...all, { name: '', seconds: 300, bandpass: '' }])}><Plus size={16} />Add filter</button>
          <button type="submit" disabled={!filters.length || new Set(filters.map(f => f.name.trim())).size !== filters.length}><Save size={16} />Save profile</button></div>
      </fieldset>
    </form>
    <fieldset disabled={busy}><legend>Observing night</legend><div className="rig-profile-grid">
      <label className="rig-profile-field"><span>Night</span><input type="date" value={night} onChange={e => { setNight(e.target.value); setPreview(null); }} /></label>
      <label className="rig-profile-field"><span>Moon illumination (%)</span><input type="number" min="0" max="100" step="0.1" value={moon} onChange={e => { setMoon(e.target.value); setPreview(null); }} /></label>
      <label className="rig-profile-field"><span>Moon above horizon (%)</span><input type="number" min="0" max="100" step="0.1" value={moonUp} onChange={e => { setMoonUp(e.target.value); setPreview(null); }} /></label>
    </div><div className="director-actions">
      <button type="button" disabled={!initial} onClick={() => mutation.mutate({ operation: 'browse' })}><RefreshCw size={16} />Browse projects</button>
      <button type="button" disabled={!initial || !nightValid} onClick={() => mutation.mutate({ operation: 'tonight', night: observingNight })}><Download size={16} />Pull nightly work</button>
      <button type="button" disabled={!initial} onClick={() => mutation.mutate({ operation: 'checkin' })}><Upload size={16} />Check in</button>
    </div></fieldset>
    {mutation.isPending && <p role="status">Contacting collaboration server...</p>}
    {mutation.isError && <p role="alert">{mutation.error.message}</p>}
    {notice && <p role="status">{notice}</p>}
    {work?.projects && <table><thead><tr><th>Project</th><th>Compatibility</th><th /></tr></thead><tbody>{work.projects.map(p => <tr key={p.project_id}><td>{p.name}</td><td>{p.compatible === null ? 'Unknown' : p.compatible ? 'Compatible' : 'Incompatible'}</td><td><button type="button" disabled={busy || !nightValid || p.joined || p.compatible === false} onClick={() => { if (window.confirm(`Join ${p.name} with this rig?`)) mutation.mutate({ operation: 'join', project: p.project_id, night: observingNight }); }}><Link2 size={16} />{p.joined ? 'Joined' : 'Join'}</button></td></tr>)}</tbody></table>}
    {work?.shares?.map(share => <div key={share.task_id}><h4>{share.name ?? share.task_id}</h4>
      <p>{share.demands.length} panel/filter visits; geometry version {share.version}</p>
      {share.review_reasons.length > 0 ? <p role="alert">Needs review: {share.review_reasons.join(', ')}</p> : <button type="button" disabled={busy || !nightValid} onClick={() => mutation.mutate({ operation: 'preview', task: share.task_id, night: observingNight })}><Download size={16} />Review import</button>}
    </div>)}
    {preview?.result.preview && <section aria-label="Review collaboration import"><h4>Review import</h4>
      <p>{preview.result.plan?.share.name ?? preview.input.task}: {preview.result.plan?.share.demands.length} visits for {preview.input.night.night}</p>
      <table><thead><tr><th>Panel</th><th>Filter</th><th>Exposure (s)</th><th>Frames</th></tr></thead><tbody>{preview.result.plan?.share.demands.map(d => <tr key={`${d.panel_index}-${d.filter}`}><td>{d.panel_index}</td><td>{d.filter}</td><td>{d.exposure_ms / 1000}</td><td>{d.requested_frames}</td></tr>)}</tbody></table>
      <button type="button" disabled={busy} onClick={() => mutation.mutate({ ...preview.input, operation: 'apply', review_digest: preview.result.preview!.review_digest })}><Save size={16} />Import draft</button>
      {preview.result.plan && <Link to={`/plan?plan=${encodeURIComponent(preview.result.plan.project_id)}`}>Project plan</Link>}
    </section>}
    <CollaborationReports connection={connection.binding.id} canWrite={canWrite} />
  </div>;
}
