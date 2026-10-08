import { useId, useState, type ReactNode } from 'react';
import { useMutation } from '@tanstack/react-query';
import { Download, Link2, RefreshCw, Save, Upload } from 'lucide-react';
import { Link } from 'react-router-dom';
import { apiClient } from '../../api/client';
import type { CollaborationConnection, CollaborationWork, CollaborationWorkInput } from '../../api/collaborationTypes';
import CollaborationReports from './CollaborationReports';
import CollaborationBackground from './CollaborationBackground';
import CollaborationCaptureSettings from './CollaborationCaptureSettings';
import WorkspaceTabs from './WorkspaceTabs';
import { workspacePanel } from './workspacePanel';

const TABS = [
  { id: 'tonight', label: 'Tonight' }, { id: 'automation', label: 'Automation' },
  { id: 'capture', label: 'Capture' }, { id: 'reports', label: 'Reports' }, { id: 'connection', label: 'Connection' },
] as const;
type Tab = (typeof TABS)[number]['id'];

export default function CollaborationWorkflows({ connection, canWrite, refresh, connectionControls, active = true }: { connection: CollaborationConnection; canWrite: boolean; refresh: () => void; connectionControls?: ReactNode; active?: boolean }) {
  const initial = connection.binding.settings;
  const id = useId();
  const connected = connection.status === 'registered';
  const [tab, setTab] = useState<Tab>(connected ? initial ? 'tonight' : 'capture' : 'connection');
  const [visited, setVisited] = useState<Set<Tab>>(() => new Set([tab]));
  const tabs = TABS.filter(t => connected || t.id === 'connection' || (t.id === 'automation' && connection.binding.background));
  const current = tabs.some(t => t.id === tab) ? tab : tabs[0].id;
  const [night, setNight] = useState('');
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
      else setWork(previous => {
        const projects = result.projects ?? previous?.projects;
        return { ...result, projects: input.operation === 'join'
          ? projects?.map(project => project.project_id === input.project ? { ...project, joined: true } : project)
          : projects };
      });
    },
  });
  const busy = !canWrite || !connected || mutation.isPending;
  const nightValid = !night || /^\d{4}-\d{2}-\d{2}$/.test(night);
  const nightOverride = night ? { observing_date: night } : {};
  return <div className="collaboration-workflows">
    {connected && <div className="collaboration-checkin director-actions"><button type="button" disabled={busy || !initial} onClick={() => mutation.mutate({ operation: 'checkin' })}><Upload size={16} />Check in</button></div>}
    <WorkspaceTabs id={id} label="Collaboration sections" tabs={tabs} value={current} onChange={next => { setTab(next); setVisited(previous => new Set([...previous, next])); }} />
    <div {...workspacePanel(id, 'tonight', current)}>
    <fieldset disabled={busy}><legend>Work requests</legend>
    <details><summary>Night override</summary>
      <label className="rig-profile-field"><span>Observing date</span><input type="date" value={night} onChange={e => { setNight(e.target.value); setPreview(null); setWork(null); }} /></label>
    </details><div className="director-actions">
      <button type="button" disabled={!initial} onClick={() => mutation.mutate({ operation: 'browse' })}><RefreshCw size={16} />Browse projects</button>
      <button type="button" disabled={!initial || !nightValid} onClick={() => mutation.mutate({ operation: 'tonight', ...nightOverride })}><Download size={16} />Pull tonight's work</button>
    </div></fieldset>
    {work?.night && <p role="status">Observing night: {work.night.night}</p>}
    {work?.projects && <table><thead><tr><th>Project</th><th>Compatibility</th><th /></tr></thead><tbody>{work.projects.map(p => <tr key={p.project_id}><td>{p.name}</td><td>{p.compatible === null ? 'Unknown' : p.compatible ? 'Compatible' : 'Incompatible'}</td><td><button type="button" disabled={busy || !nightValid || p.joined || p.compatible === false} onClick={() => { if (window.confirm(`Join ${p.name} with this rig?`)) mutation.mutate({ operation: 'join', project: p.project_id, ...nightOverride }); }}><Link2 size={16} />{p.joined ? 'Joined' : 'Join'}</button></td></tr>)}</tbody></table>}
    {work?.shares?.map(share => <div key={share.task_id}><h4>{share.name ?? share.task_id}</h4>
      <p>{share.demands.length} panel/filter visits; geometry version {share.version}</p>
      {share.review_reasons.length > 0 ? <p role="alert">Needs review: {share.review_reasons.join(', ')}</p> : <button type="button" disabled={busy || !work.night} onClick={() => { if (work.night) mutation.mutate({ operation: 'preview', task: share.task_id, night: work.night }); }}><Download size={16} />Review import</button>}
    </div>)}
    {preview?.result.preview && <section aria-label="Review collaboration import"><h4>Review import</h4>
      <p>{preview.result.plan?.share.name ?? preview.input.task}: {preview.result.plan?.share.demands.length} visits for {preview.input.night.night}</p>
      <table><thead><tr><th>Panel</th><th>Filter</th><th>Exposure (s)</th><th>Frames</th></tr></thead><tbody>{preview.result.plan?.share.demands.map(d => <tr key={`${d.panel_index}-${d.filter}`}><td>{d.panel_index}</td><td>{d.filter}</td><td>{d.exposure_ms / 1000}</td><td>{d.requested_frames}</td></tr>)}</tbody></table>
      <button type="button" disabled={busy} onClick={() => mutation.mutate({ ...preview.input, operation: 'apply', review_digest: preview.result.preview!.review_digest })}><Save size={16} />Import draft</button>
      {preview.result.plan && <Link to={`/plan?plan=${encodeURIComponent(preview.result.plan.project_id)}`}>Project plan</Link>}
    </section>}
    </div>
    <div {...workspacePanel(id, 'automation', current)}>{visited.has('automation') && <CollaborationBackground embedded active={active && current === 'automation'} connection={connection} canWrite={canWrite} refresh={refresh} joinedProjects={work?.projects} />}</div>
    <div {...workspacePanel(id, 'capture', current)}>{connected && visited.has('capture') && <CollaborationCaptureSettings key={JSON.stringify(initial)} connection={connection} canWrite={!busy} onSaved={() => { setPreview(null); setWork(null); refresh(); }} />}</div>
    <div {...workspacePanel(id, 'reports', current)}>{connected && visited.has('reports') && <CollaborationReports embedded active={active && current === 'reports'} connection={connection.binding.id} canWrite={canWrite} />}</div>
    <div {...workspacePanel(id, 'connection', current)}>{connectionControls}</div>
    {mutation.isPending && <p role="status">Contacting collaboration server...</p>}
    {mutation.isError && <p role="alert">{mutation.variables?.operation === 'checkin' && 'Check-in failed; queued reports retained. '}{mutation.error.message}</p>}
    {notice && <p role="status">{notice}</p>}
  </div>;
}
