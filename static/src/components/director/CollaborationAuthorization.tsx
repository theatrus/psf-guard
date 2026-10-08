import { useEffect, useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { ExternalLink, Link2, RefreshCw, X } from 'lucide-react';
import { apiClient, CollaborationRequestError } from '../../api/client';
import type { CollaborationAction, CollaborationConnection, CollaborationReply } from '../../api/collaborationTypes';
import { collaborationMessage, connectionStates } from './collaborationUi';

export default function CollaborationAuthorization({ connection, canWrite, refresh, onUpdate, onBusy }: {
  connection: CollaborationConnection; canWrite: boolean; refresh: () => void;
  onUpdate: (connection: CollaborationConnection) => void; onBusy?: (busy: boolean) => void;
}) {
  const [code, setCode] = useState('');
  const [capabilities, setCapabilities] = useState<{ signin: boolean; pairing: boolean } | null>(null);
  const [browser, setBrowser] = useState<CollaborationReply | null>(null);
  const operation = useMutation({ retry: false,
    mutationFn: ({ action, pairingCode }: { action: CollaborationAction; pairingCode?: string }) => apiClient.collaborationAction(connection.binding.id, action, pairingCode),
    onSuccess: (result, input) => {
      if ('binding' in result) onUpdate(result);
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
  useEffect(() => { onBusy?.(operation.isPending); return () => onBusy?.(false); }, [onBusy, operation.isPending]);
  const run = (action: CollaborationAction, pairingCode?: string) => {
    operation.mutate({ action, pairingCode });
    if (action === 'pair') setCode('');
  };
  const fresh = connection.binding.agent_id === null && connection.binding.state === 'new';
  const waiting = browser !== null || connection.status === 'awaiting_browser';
  const busy = !canWrite || operation.isPending;
  return <section aria-label="Server authorization" className="collaboration-authorization">
    <p role="status">{browser ? 'Waiting for browser approval' : connectionStates[connection.status]}</p>
    {operation.isPending && <p role="status">Contacting collaboration server...</p>}
    {operation.isError && <p role="alert">{collaborationMessage(operation.error)}</p>}
    <div className="director-actions">
      {fresh && !waiting && !capabilities && <button type="button" disabled={busy} onClick={() => run('discover')}><Link2 size={16} />Connect</button>}
      {fresh && !waiting && capabilities?.signin && <button type="button" disabled={busy} onClick={() => run('signin')}><ExternalLink size={16} />Browser sign-in</button>}
      {waiting && <>
        {browser?.url && <a href={browser.url} target="_blank" rel="noopener noreferrer"><ExternalLink size={16} />Open sign-in</a>}
        <button type="button" disabled={busy} onClick={() => run('poll')}><RefreshCw size={16} />Check sign-in</button>
        <button type="button" disabled={busy} onClick={() => run('cancel')}><X size={16} />Cancel</button>
      </>}
    </div>
    {fresh && !waiting && capabilities?.pairing && <form className="collaboration-pair-form" onSubmit={event => { event.preventDefault(); if (code.trim() && !busy) run('pair', code.trim()); }}>
      <label className="rig-profile-field"><span>Pairing code</span><input type="password" autoComplete="off" maxLength={512} value={code} disabled={busy} onChange={event => setCode(event.target.value)} /></label>
      <button type="submit" disabled={busy || !code.trim()}><Link2 size={16} />Pair</button>
    </form>}
    {capabilities && !capabilities.signin && !capabilities.pairing && <p role="alert">This server offers no supported authentication method.</p>}
    {!fresh && !waiting && connection.status !== 'registered' && <p>Original agent, work and reports retained. Check the connection before registering a replacement.</p>}
  </section>;
}
