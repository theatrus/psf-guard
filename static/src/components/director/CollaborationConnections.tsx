import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { ExternalLink, Link2, Plus, RefreshCw, Unplug, X } from 'lucide-react';
import { apiClient, CollaborationRequestError } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { CollaborationAction, CollaborationConnection, CollaborationReply } from '../../api/collaborationTypes';
import CollaborationWorkflows from './CollaborationWorkflows';
import CollaborationBackground from './CollaborationBackground';

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || 'Collaboration request failed'
  : error instanceof Error ? error.message : 'Collaboration request failed';
const states: Record<CollaborationConnection['status'], string> = {
  not_connected: 'Not connected', registered: 'Registered', disconnected: 'Disconnected',
  credential_missing: 'Credential missing; original agent and reports retained',
  reauth_required: 'Credential rejected; original agent and reports retained',
  storage_unavailable: 'Credential file unavailable; check ownership and permissions',
  outcome_unknown: 'Registration outcome unknown; check the server before registering another agent',
  awaiting_browser: 'Waiting for browser approval',
};

export default function CollaborationConnections({ rig, name }: { rig: string; name: string }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const key = ['collaborationConnections', rig];
  const query = useQuery({ queryKey: key, queryFn: () => apiClient.getCollaborationConnections(rig), retry: false, refetchOnWindowFocus: true });
  const [server, setServer] = useState('');
  const [rigName, setRigName] = useState(name);
  const [loopback, setLoopback] = useState(false);
  const create = useMutation({
    mutationFn: () => apiClient.createCollaborationConnection(rig, { id: crypto.randomUUID(), server_url: server.trim(), name: rigName.trim(), allow_loopback_http: loopback }),
    retry: false,
    onSuccess: () => { setServer(''); void client.invalidateQueries({ queryKey: key }); },
    onError: () => void client.invalidateQueries({ queryKey: key }),
  });
  return <section className="collaboration-connections" aria-label="Collaboration">
    <div className="director-toolbar"><h3>Collaboration</h3><button type="button" title="Reload collaboration connections" aria-label="Reload collaboration connections" disabled={query.isFetching} onClick={() => void query.refetch()}><RefreshCw size={16} /></button></div>
    {query.isPending && <p role="status">Loading connections...</p>}
    {query.isError && <p role="alert">{message(query.error)}</p>}
    {query.data?.map(connection => <Connection key={connection.binding.id} connection={connection} canWrite={canWrite} refresh={() => void client.invalidateQueries({ queryKey: key })} />)}
    {canWrite && <form className="rig-profile-form" onSubmit={event => { event.preventDefault(); if (!create.isPending) create.mutate(); }}>
      <fieldset disabled={create.isPending}><legend>New connection</legend>
        <div className="rig-profile-grid">
          <label className="rig-profile-field"><span>Collaboration server</span><input required type="url" value={server} placeholder="https://collaboration.example" onChange={event => setServer(event.target.value)} /></label>
          <label className="rig-profile-field"><span>Remote rig name</span><input required maxLength={128} value={rigName} onChange={event => setRigName(event.target.value)} /></label>
        </div>
        <label><input type="checkbox" checked={loopback} onChange={event => setLoopback(event.target.checked)} />Allow loopback HTTP for isolated testing</label>
      </fieldset>
      {create.isError && <p role="alert">{message(create.error)}</p>}
      <div className="director-actions"><button type="submit" disabled={create.isPending || !server.trim() || !rigName.trim()}><Plus size={16} />{create.isPending ? 'Saving...' : 'Add connection'}</button></div>
    </form>}
  </section>;
}

function Connection({ connection, canWrite, refresh }: { connection: CollaborationConnection; canWrite: boolean; refresh: () => void }) {
  const [code, setCode] = useState('');
  const [capabilities, setCapabilities] = useState<{ signin: boolean; pairing: boolean } | null>(null);
  const [browser, setBrowser] = useState<CollaborationReply | null>(null);
  const operation = useMutation({
    retry: false,
    mutationFn: ({ action, pairingCode }: { action: CollaborationAction; pairingCode?: string }) => apiClient.collaborationAction(connection.binding.id, action, pairingCode),
    onSuccess: (result, input) => {
      if ('signin' in result) setCapabilities({ signin: result.signin === true, pairing: result.pairing === true });
      else if (!('binding' in result) && result.status === 'awaiting_browser') setBrowser(previous => ({ ...previous, ...result }));
      else if (input.action !== 'discover') setBrowser(null);
      refresh();
    },
    onError: (error, input) => {
      if ((input.action === 'poll' || input.action === 'cancel') && error instanceof CollaborationRequestError && error.status === 409) setBrowser(null);
      refresh();
    },
  });
  const run = (action: CollaborationAction, pairingCode?: string) => {
    operation.mutate({ action, pairingCode });
    if (action === 'pair') setCode('');
  };
  const b = connection.binding;
  const busy = !canWrite || operation.isPending;
  const fresh = b.agent_id === null && b.state === 'new';
  const waiting = browser !== null || connection.status === 'awaiting_browser';
  return <div className="collaboration-connection">
    <strong>{b.name}</strong> <span className="director-muted">{b.base_url}</span>
    <p role="status">{browser ? 'Waiting for browser approval' : states[connection.status]}{b.agent_id && ` (${b.agent_id})`}</p>
    {operation.isError && <p role="alert">{message(operation.error)}</p>}
    <div className="director-actions">
      {fresh && !waiting && !capabilities && <button type="button" disabled={busy} onClick={() => run('discover')}><Link2 size={16} />Connect</button>}
      {fresh && !waiting && capabilities?.signin && <button type="button" disabled={busy} onClick={() => run('signin')}><ExternalLink size={16} />Browser sign-in</button>}
      {waiting && <>
        {browser?.url && <a href={browser.url} target="_blank" rel="noopener noreferrer"><ExternalLink size={16} />Open sign-in</a>}
        <button type="button" disabled={busy} onClick={() => run('poll')}><RefreshCw size={16} />Check sign-in</button>
        <button type="button" disabled={busy} onClick={() => run('cancel')}><X size={16} />Cancel</button>
      </>}
      {b.agent_id && b.state !== 'disabled' && <button type="button" disabled={busy} onClick={() => run('validate')}><RefreshCw size={16} />Check connection</button>}
      {b.state !== 'disabled' && <button type="button" disabled={busy} onClick={() => { if (window.confirm('Disconnect this agent? Its identity, work and reports will be retained.')) run('disconnect'); }}><Unplug size={16} />Disconnect</button>}
    </div>
    {fresh && !waiting && capabilities?.pairing && <form className="director-actions" onSubmit={event => { event.preventDefault(); if (code.trim() && !busy) run('pair', code.trim()); }}>
      <label>Pairing code <input type="password" autoComplete="off" maxLength={512} value={code} disabled={busy} onChange={event => setCode(event.target.value)} /></label>
      <button type="submit" disabled={busy || !code.trim()}><Link2 size={16} />Pair</button>
    </form>}
    {capabilities && !capabilities.signin && !capabilities.pairing && <p role="alert">This server offers no supported authentication method.</p>}
    {connection.status === 'registered' && <CollaborationWorkflows connection={connection} canWrite={canWrite} refresh={refresh} />}
    {connection.status !== 'registered' && b.background && <CollaborationBackground connection={connection} canWrite={canWrite} refresh={refresh} />}
  </div>;
}
