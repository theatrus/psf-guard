import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { RefreshCw, Save, Undo2 } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { CollaborationBackgroundInput, CollaborationBackgroundPolicy, CollaborationConnection, RemoteProject } from '../../api/collaborationTypes';

export default function CollaborationBackground({ connection, canWrite, refresh, joinedProjects = [], embedded = false, active = true }: { connection: CollaborationConnection; canWrite: boolean; refresh: () => void; joinedProjects?: RemoteProject[]; embedded?: boolean; active?: boolean }) {
  const [open, setOpen] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [enabled, setEnabled] = useState(false);
  const [catalog, setCatalog] = useState('');
  const [projects, setProjects] = useState<string[]>([]);
  const [interval, setInterval] = useState(15);
  const [activate, setActivate] = useState(false);
  const [expected, setExpected] = useState<CollaborationBackgroundPolicy | null>(null);
  const client = useQueryClient();
  const key = ['collaboration-background', connection.binding.id];
  const query = useQuery({
    queryKey: key,
    queryFn: () => apiClient.collaborationBackground(connection.binding.id, { operation: 'background_status' }),
    enabled: active && (embedded || open),
    refetchInterval: active && (embedded || open) ? 15_000 : false,
  });
  useEffect(() => {
    if (!query.data || dirty) return;
    const policy = query.data.policy;
    setExpected(policy);
    setEnabled(policy?.enabled ?? false);
    setCatalog(policy?.catalog_id ?? (query.data.catalogs.length === 1 ? query.data.catalogs[0].id : ''));
    setProjects(policy?.project_ids ?? []);
    setInterval(policy?.interval_minutes ?? 15);
    setActivate(policy?.activate ?? false);
  }, [query.data, dirty]);
  const mutation = useMutation({
    retry: false,
    mutationFn: (input: CollaborationBackgroundInput) => apiClient.collaborationBackground(connection.binding.id, input),
    onSuccess: data => { client.setQueryData(key, data); setDirty(false); refresh(); },
    onError: () => { void client.invalidateQueries({ queryKey: key }); refresh(); },
  });
  const change = (update: () => void) => { setDirty(true); update(); };
  const saved = query.data?.policy ?? null;
  const policy: CollaborationBackgroundPolicy = { enabled, catalog_id: catalog, project_ids: projects, interval_minutes: interval, activate };
  const valid = !!catalog && projects.length > 0 && projects.length <= 32 && Number.isInteger(interval) && interval >= 5 && interval <= 1440;
  const busy = !canWrite || mutation.isPending || !query.data;
  const status = query.data?.status;
  const connected = (query.data?.connection_status ?? connection.status) === 'registered';
  const choices = new Map(query.data?.projects.map(p => [p.id, p.name]));
  joinedProjects.filter(p => p.joined).forEach(p => choices.set(p.project_id, p.name));
  projects.forEach(id => { if (!choices.has(id)) choices.set(id, id); });
  const time = (ms: number | null) => ms === null ? 'Not yet' : new Date(ms).toLocaleString();
  const content = <>
    {query.isError && <p role="alert">{query.error.message}</p>}
    <form className="rig-profile-form" onSubmit={event => {
      event.preventDefault();
      mutation.mutate({ operation: 'background_configure', expected, policy: valid ? policy : enabled ? policy : null });
    }}>
      <fieldset disabled={busy}><legend>Background refresh</legend>
        <label><input type="checkbox" checked={enabled} onChange={e => change(() => setEnabled(e.target.checked))} />Pull tonight automatically</label>
        <div className="rig-profile-grid">
          <label className="rig-profile-field"><span>Rig database</span><select value={catalog} onChange={e => change(() => setCatalog(e.target.value))}>
            <option value="">Select database</option>
            {query.data?.catalogs.map(c => <option key={c.id} value={c.id}>{c.name}</option>)}
          </select></label>
          <label className="rig-profile-field"><span>Refresh interval (minutes)</span><input type="number" min="5" max="1440" value={interval} onChange={e => change(() => setInterval(Number(e.target.value)))} /></label>
        </div>
        <fieldset><legend>Allowed projects</legend>
          {[...choices].map(([id, name]) => <label className="collaboration-project-choice" key={id}><input type="checkbox" checked={projects.includes(id)} onChange={e => change(() => setProjects(all => e.target.checked ? [...all, id] : all.filter(p => p !== id)))} />{name}</label>)}
          {choices.size === 0 && <p>No joined or imported projects</p>}
        </fieldset>
        <label><input type="checkbox" checked={activate} onChange={e => change(() => setActivate(e.target.checked))} />Activate in rig database</label>
        <div className="director-actions">
          <button type="submit" disabled={!dirty || (enabled && !valid)}><Save size={16} />Save automation</button>
          <button type="button" title="Discard edits" aria-label="Discard automation edits" disabled={!dirty} onClick={() => setDirty(false)}><Undo2 size={16} /></button>
          <button type="button" disabled={!saved?.enabled || !connected || dirty || !!status?.running} onClick={() => mutation.mutate({ operation: 'background_run' })}><RefreshCw size={16} />Refresh now</button>
        </div>
      </fieldset>
    </form>
    {mutation.isError && <p role="alert">{mutation.error.message}</p>}
    {!connected && saved?.enabled && <p role="alert">Automatic refresh paused: {(query.data?.connection_status ?? connection.status).replaceAll('_', ' ')}</p>}
    {status && <div role="status">
      <p>{status.running ? 'Pulling tonight\'s work...' : saved?.enabled ? connected ? 'Automatic refresh enabled' : 'Automatic refresh paused' : 'Automatic refresh off'}</p>
      <p>Last success: {time(status.last_success_ms)}. Next refresh: {saved?.enabled ? connected ? time(status.next_run_ms) : 'Paused' : 'Off'}.</p>
      {status.result && <p>{status.result.night}: {status.result.imported} imported, {status.result.unchanged} unchanged, {status.result.activated} activated</p>}
    </div>}
    {status?.last_error && <p role="alert">{status.last_error}. Existing plans retained.</p>}
    {!!status?.result?.held.length && <ul>{status.result.held.map((reason, index) => <li key={index}>{reason}</li>)}</ul>}
    </>;
  return embedded ? <section className="collaboration-background" aria-label="Automatic work requests">{content}</section>
    : <details className="collaboration-background" open={open} onToggle={event => setOpen(event.currentTarget.open)}><summary>Automatic work requests</summary>{open && content}</details>;
}
