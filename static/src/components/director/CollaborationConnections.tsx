import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link2, Plus, RefreshCw, Unplug } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { CollaborationConnection } from '../../api/collaborationTypes';
import CollaborationWorkflows from './CollaborationWorkflows';
import CollaborationWizard from './CollaborationWizard';
import { collaborationMessage, connectionStates } from './collaborationUi';

export default function CollaborationConnections({ rig, name, active = true }: { rig: string; name: string; active?: boolean }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const key = ['collaborationConnections', rig];
  const query = useQuery({ queryKey: key, queryFn: () => apiClient.getCollaborationConnections(rig), retry: false, refetchOnWindowFocus: true });
  const [selected, setSelected] = useState<string | null>(null);
  const [wizard, setWizard] = useState<{ id: string | null } | null>(null);
  const current = query.data?.find(c => c.binding.id === selected) ?? query.data?.[0];
  const refresh = () => { void client.invalidateQueries({ queryKey: key }); };
  const update = (connection: CollaborationConnection) => {
    client.setQueryData<CollaborationConnection[]>(key, rows => rows?.some(c => c.binding.id === connection.binding.id)
      ? rows.map(c => c.binding.id === connection.binding.id ? connection : c) : [...rows ?? [], connection]);
    setSelected(connection.binding.id);
    if (wizard) setWizard({ id: connection.binding.id });
  };
  return <section className="collaboration-connections" aria-label="Collaboration">
    <div className="director-toolbar"><h3>Collaboration</h3><div className="director-actions">
      <button type="button" title="Reload collaboration connections" aria-label="Reload collaboration connections" disabled={query.isFetching} onClick={() => void query.refetch()}><RefreshCw size={16} /></button>
      {canWrite && <button type="button" onClick={() => setWizard({ id: null })}><Plus size={16} />Connect server</button>}
    </div></div>
    {query.isPending && <p role="status">Loading connections...</p>}
    {query.isError && <p role="alert">{collaborationMessage(query.error)}</p>}
    {query.data?.length === 0 && <p className="director-muted">No collaboration servers connected</p>}
    <ul className="collaboration-server-list" aria-label="Collaboration servers">
      {query.data?.map(connection => <li key={connection.binding.id}>
        <button type="button" className="collaboration-server-row" aria-pressed={current?.binding.id === connection.binding.id} onClick={() => setSelected(connection.binding.id)}>
          <span className="director-record-name"><strong>{connection.binding.name}</strong><span className="director-muted">{connection.binding.base_url}</span></span>
          <span className={`workspace-pill${connection.status === 'registered' ? ' is-ok' : ' is-missing'}`}>{connectionStates[connection.status]}</span>
        </button>
      </li>)}
    </ul>
    {query.data?.map(connection => <div className="collaboration-connection" key={connection.binding.id} hidden={current?.binding.id !== connection.binding.id}>
      <CollaborationWorkflows connection={connection} canWrite={canWrite} active={active && current?.binding.id === connection.binding.id} refresh={refresh}
        connectionControls={<ConnectionControls connection={connection} canWrite={canWrite} refresh={refresh} onUpdate={update} onSetup={() => setWizard({ id: connection.binding.id })} />} />
    </div>)}
    {wizard && canWrite && <CollaborationWizard rig={rig} name={name} connection={query.data?.find(c => c.binding.id === wizard.id) ?? null}
      onPendingId={id => setWizard({ id })} onUpdate={update} refresh={refresh} onClose={() => { setWizard(null); refresh(); }} />}
  </section>;
}

function ConnectionControls({ connection, canWrite, refresh, onUpdate, onSetup }: {
  connection: CollaborationConnection; canWrite: boolean; refresh: () => void; onUpdate: (connection: CollaborationConnection) => void; onSetup: () => void;
}) {
  const operation = useMutation({ retry: false,
    mutationFn: (action: 'validate' | 'disconnect') => apiClient.collaborationAction(connection.binding.id, action),
    onSuccess: result => { if ('binding' in result) onUpdate(result); refresh(); }, onError: refresh,
  });
  const b = connection.binding;
  const fresh = b.agent_id === null && (b.state === 'new' || connection.status === 'awaiting_browser');
  const busy = !canWrite || operation.isPending;
  return <section aria-label="Connection details">
    <dl className="collaboration-evidence"><div><dt>Server</dt><dd>{b.base_url}</dd></div><div><dt>Status</dt><dd>{connectionStates[connection.status]}</dd></div>
      <div><dt>Agent</dt><dd>{b.agent_id ?? 'Not registered'}</dd></div></dl>
    {['credential_missing', 'reauth_required', 'storage_unavailable', 'outcome_unknown'].includes(connection.status) && <p role="alert">{connectionStates[connection.status]}. Original agent, work and reports retained; check the connection before registering a replacement.</p>}
    <div className="director-actions">
      {(fresh || (connection.status === 'registered' && !b.settings)) && <button type="button" disabled={busy} onClick={onSetup}><Link2 size={16} />Continue setup</button>}
      {b.agent_id && b.state !== 'disabled' && <button type="button" disabled={busy} onClick={() => operation.mutate('validate')}><RefreshCw size={16} />Check connection</button>}
      {b.state !== 'disabled' && <button type="button" disabled={busy} onClick={() => { if (window.confirm('Disconnect this agent? Its identity, work and reports will be retained.')) operation.mutate('disconnect'); }}><Unplug size={16} />Disconnect</button>}
    </div>
    {operation.isPending && <p role="status">Contacting collaboration server...</p>}
    {operation.isError && <p role="alert">{collaborationMessage(operation.error)}</p>}
  </section>;
}
