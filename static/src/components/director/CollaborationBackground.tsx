import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { RefreshCw, Save, Undo2 } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { CollaborationBackgroundInput, CollaborationBackgroundPolicy, CollaborationConnection, RemoteProject } from '../../api/collaborationTypes';

export default function CollaborationBackground({ connection, canWrite, refresh, joinedProjects = [], embedded = false, active = true }: { connection: CollaborationConnection; canWrite: boolean; refresh: () => void; joinedProjects?: RemoteProject[]; embedded?: boolean; active?: boolean }) {
  const [open, setOpen] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [enabled, setEnabled] = useState(false);
  const [projects, setProjects] = useState<string[]>([]);
  const [reports, setReports] = useState(false);
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
    setProjects(policy?.project_ids ?? []);
    setReports(policy?.automatic_reports ?? false);
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
  const catalog = query.data?.catalogs.length === 1 ? query.data.catalogs[0] : null;
  const policy: CollaborationBackgroundPolicy = { enabled, catalog_id: catalog?.id ?? expected?.catalog_id ?? '', project_ids: projects, interval_minutes: expected?.interval_minutes ?? 15, activate, automatic_reports: reports };
  const valid = !!catalog && (!enabled || projects.length > 0) && projects.length <= 32;
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
      mutation.mutate({ operation: 'background_configure', expected, policy: valid || enabled || reports || expected ? policy : null });
    }}>
      <fieldset disabled={busy}><legend>Nightly automation</legend>
        <p>Rig database: {catalog?.name ?? 'Unavailable or ambiguous'}</p>
        <label><input type="checkbox" checked={enabled} onChange={e => change(() => setEnabled(e.target.checked))} />Pull tonight automatically</label>
        <fieldset><legend>Allowed projects</legend>
          {[...choices].map(([id, name]) => <label className="collaboration-project-choice" key={id}><input type="checkbox" checked={projects.includes(id)} onChange={e => change(() => setProjects(all => e.target.checked ? [...all, id] : all.filter(p => p !== id)))} />{name}</label>)}
          {choices.size === 0 && <p>No joined or imported projects</p>}
        </fieldset>
        <label><input type="checkbox" checked={activate} onChange={e => change(() => setActivate(e.target.checked))} />Activate in rig database</label>
        <label><input type="checkbox" checked={reports} onChange={e => change(() => setReports(e.target.checked))} />Submit contribution reports automatically</label>
        <div className="director-actions">
          <button type="submit" disabled={!dirty || ((enabled || reports) && !valid)}><Save size={16} />Save automation</button>
          <button type="button" title="Discard edits" aria-label="Discard automation edits" disabled={!dirty} onClick={() => setDirty(false)}><Undo2 size={16} /></button>
          <button type="button" disabled={!(saved?.enabled || saved?.automatic_reports) || !connected || dirty || !!status?.running} onClick={() => mutation.mutate({ operation: 'background_run' })}><RefreshCw size={16} />Refresh now</button>
        </div>
      </fieldset>
    </form>
    {mutation.isError && <p role="alert">{mutation.error.message}</p>}
    {!connected && (saved?.enabled || saved?.automatic_reports) && <p role="alert">Automatic refresh paused: {(query.data?.connection_status ?? connection.status).replaceAll('_', ' ')}</p>}
    {status && <div role="status">
      <p>{status.running ? 'Pulling tonight\'s work...' : saved?.enabled ? connected ? 'Automatic refresh enabled' : 'Automatic refresh paused' : 'Automatic refresh off'}</p>
      <p>Last success: {time(status.last_success_ms)}. Next refresh: {saved?.enabled ? connected ? time(status.next_run_ms) : 'Paused' : 'Off'}.</p>
      {status.result && <p>{status.result.night}: {status.result.imported} imported, {status.result.unchanged} unchanged, {status.result.activated} activated</p>}
      {saved?.automatic_reports && <p>Automatic reports: {!connected ? 'Paused' : status.reports ? `${status.reports.queued} queued, ${status.reports.delivered} delivered, ${status.reports.held} held` : 'Awaiting first pass'}</p>}
    </div>}
    {status?.last_error && <p role="alert">{status.last_error}. Existing plans retained.</p>}
    {status?.report_error && <p role="alert">{status.report_error}. Queued reports retained.</p>}
    {!!status?.result?.held.length && <ul>{status.result.held.map((reason, index) => <li key={index}>{reason}</li>)}</ul>}
    </>;
  return embedded ? <section className="collaboration-background" aria-label="Automatic work requests">{content}</section>
    : <details className="collaboration-background" open={open} onToggle={event => setOpen(event.currentTarget.open)}><summary>Automatic work requests</summary>{open && content}</details>;
}
