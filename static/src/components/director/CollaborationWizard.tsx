import { useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { ArrowRight, Check } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { CollaborationConnection } from '../../api/collaborationTypes';
import Dialog from '../Dialog';
import CollaborationAuthorization from './CollaborationAuthorization';
import CollaborationCaptureSettings from './CollaborationCaptureSettings';
import { collaborationMessage } from './collaborationUi';

export default function CollaborationWizard({ rig, name, connection, onPendingId, onUpdate, refresh, onClose }: {
  rig: string; name: string; connection: CollaborationConnection | null;
  onPendingId: (id: string) => void; onUpdate: (connection: CollaborationConnection) => void; refresh: () => void; onClose: () => void;
}) {
  const [server, setServer] = useState('');
  const [rigName, setRigName] = useState(name);
  const [loopback, setLoopback] = useState(false);
  const [id] = useState(() => crypto.randomUUID());
  const [busy, setBusy] = useState(false);
  const create = useMutation({ retry: false,
    onMutate: () => onPendingId(id),
    mutationFn: () => apiClient.createCollaborationConnection(rig, { id, server_url: server.trim(), name: rigName.trim(), allow_loopback_http: loopback }),
    onSuccess: onUpdate,
    // The response can be lost after persistence; recover the same binding.
    onError: refresh,
  });
  const step = !connection ? 0 : connection.status === 'registered' ? 2 : 1;
  return <Dialog open title="Connect collaboration" onClose={() => { if (!busy && !create.isPending) onClose(); }} className="collaboration-wizard director-page director-embedded" backdropClassName="collaboration-wizard-backdrop"
    footer={<button type="button" disabled={busy || create.isPending} onClick={onClose}>Close</button>}>
    <ol className="collaboration-steps" aria-label="Connection setup progress">
      {['Server', 'Sign in', 'Capture'].map((label, index) => <li key={label} aria-current={step === index ? 'step' : undefined} className={index < step ? 'is-complete' : step === index ? 'is-current' : ''}>
        <span>{index < step ? <Check size={14} /> : index + 1}</span>{label}
      </li>)}
    </ol>
    {!connection ? <form className="rig-profile-form" onSubmit={event => { event.preventDefault(); if (!create.isPending) create.mutate(); }}>
      <fieldset disabled={create.isPending}><legend>Server</legend>
        <label className="rig-profile-field"><span>Collaboration server</span><input required type="url" value={server} placeholder="https://collab.starfront.space" onChange={event => setServer(event.target.value)} /></label>
        <label className="rig-profile-field"><span>Remote rig name</span><input required maxLength={128} value={rigName} onChange={event => setRigName(event.target.value)} /></label>
        <details><summary>Advanced</summary><label><input type="checkbox" checked={loopback} onChange={event => setLoopback(event.target.checked)} />Allow loopback HTTP for isolated testing</label></details>
      </fieldset>
      {create.isError && <p role="alert">{collaborationMessage(create.error)}</p>}
      <button type="submit" disabled={create.isPending || !server.trim() || !rigName.trim()}><ArrowRight size={16} />{create.isPending ? 'Saving...' : 'Continue'}</button>
    </form> : <>
      <p className="collaboration-wizard-server"><strong>{connection.binding.name}</strong><span>{connection.binding.base_url}</span></p>
      {step === 1 ? <CollaborationAuthorization connection={connection} canWrite refresh={refresh} onUpdate={onUpdate} onBusy={setBusy} />
        : <CollaborationCaptureSettings connection={connection} canWrite finish onBusy={setBusy} onSaved={settings => { onUpdate({ ...connection, binding: { ...connection.binding, settings } }); refresh(); onClose(); }} />}
    </>}
  </Dialog>;
}
