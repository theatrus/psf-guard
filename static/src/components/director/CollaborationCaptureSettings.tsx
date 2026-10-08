import { useEffect, useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { Plus, Save, X } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { CollaborationConnection, CollaborationSettings } from '../../api/collaborationTypes';

export default function CollaborationCaptureSettings({ connection, canWrite, onSaved, finish = false, onBusy }: {
  connection: CollaborationConnection; canWrite: boolean; onSaved: (settings: CollaborationSettings) => void; finish?: boolean; onBusy?: (busy: boolean) => void;
}) {
  const initial = connection.binding.settings;
  const [binning, setBinning] = useState(initial?.binning ?? 1);
  const [colour, setColour] = useState(initial?.colour ?? false);
  const [hours, setHours] = useState(initial?.hours_per_night ?? 6);
  const [status, setStatus] = useState(initial?.share_status ?? false);
  const [filters, setFilters] = useState(() => Object.entries(initial?.filters ?? {}).map(([name, value]) => ({ name, seconds: value.exposure_seconds, bandpass: value.bandpass_nm?.toString() ?? '' })));
  const save = useMutation({ retry: false,
    mutationFn: (settings: CollaborationSettings) => apiClient.collaborationWork(connection.binding.id, { operation: 'configure', settings }),
    onSuccess: (_, settings) => onSaved(settings),
  });
  const busy = !canWrite || save.isPending;
  useEffect(() => { onBusy?.(save.isPending); return () => onBusy?.(false); }, [onBusy, save.isPending]);
  const valid = filters.length > 0 && filters.every(f => f.name.trim()) && new Set(filters.map(f => f.name.trim().toLowerCase())).size === filters.length;
  return <form className="rig-profile-form collaboration-capture" onSubmit={event => {
    event.preventDefault();
    if (busy || !valid) return;
    save.mutate({ binning, colour, hours_per_night: hours, share_status: status,
      filters: Object.fromEntries(filters.map(f => [f.name.trim(), { exposure_seconds: f.seconds, bandpass_nm: f.bandpass === '' ? null : Number(f.bandpass) }])) });
  }}>
    <fieldset disabled={busy}><legend>Capture settings</legend>
      <div className="rig-profile-grid">
        <label className="rig-profile-field"><span>Binning</span><input type="number" min="1" max="16" required value={binning} onChange={e => setBinning(Number(e.target.value))} /></label>
        <label className="rig-profile-field"><span>Hours per night</span><input type="number" min="0.01" max="24" step="0.01" required value={hours} onChange={e => setHours(Number(e.target.value))} /></label>
      </div>
      <div className="collaboration-options">
        <label><input type="checkbox" checked={colour} onChange={e => setColour(e.target.checked)} />Colour camera</label>
        <label><input type="checkbox" checked={status} onChange={e => setStatus(e.target.checked)} />Share current activity</label>
      </div>
      <div className="collaboration-filter-list">
        {filters.map((filter, index) => <div className="collaboration-filter" key={index}>
          <label className="rig-profile-field"><span>Filter</span><input aria-label={`Filter ${index + 1}`} required maxLength={80} value={filter.name} onChange={e => setFilters(all => all.map((f, i) => i === index ? { ...f, name: e.target.value } : f))} /></label>
          <label className="rig-profile-field"><span>Exposure (s)</span><input aria-label={`Exposure ${index + 1}`} type="number" required min="0.001" max="86400" step="0.001" value={filter.seconds} onChange={e => setFilters(all => all.map((f, i) => i === index ? { ...f, seconds: Number(e.target.value) } : f))} /></label>
          <label className="rig-profile-field"><span>Bandpass (nm)</span><input aria-label={`Bandpass ${index + 1}`} type="number" min="0.01" max="1000000" step="0.01" value={filter.bandpass} onChange={e => setFilters(all => all.map((f, i) => i === index ? { ...f, bandpass: e.target.value } : f))} /></label>
          <button type="button" title="Remove filter" aria-label={`Remove filter ${index + 1}`} onClick={() => setFilters(all => all.filter((_, i) => i !== index))}><X size={16} /></button>
        </div>)}
      </div>
      <div className="director-actions"><button type="button" disabled={filters.length >= 32} onClick={() => setFilters(all => [...all, { name: '', seconds: 300, bandpass: '' }])}><Plus size={16} />Add filter</button>
        <button type="submit" disabled={!valid}><Save size={16} />{save.isPending ? 'Saving...' : finish ? 'Finish setup' : 'Save profile'}</button></div>
    </fieldset>
    {save.isError && <p role="alert">{save.error.message}</p>}
    {save.isSuccess && !finish && <p role="status">Capture settings saved</p>}
  </form>;
}
