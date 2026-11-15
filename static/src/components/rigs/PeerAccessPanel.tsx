import { useEffect, useRef, useState } from 'react';
import { Check, Copy, KeyRound, Power, Save, Trash2 } from 'lucide-react';
import UnsavedMark from '../UnsavedMark';
import { apiClient } from '../../api/client';
import type { RemoteImageUploadSummary } from '../../api/types';
import { useAccess } from '../../auth/access';
import { ago, serverAddress } from './pluginShared';
import { requestMessage as message, useRemoteReceiver } from './remoteReceiver';
import '../director/DirectorPage.css';

/** Other PSF Guard servers that sync with this database: who is paired,
 *  whether they may sync, and the remote API key for clients that cannot
 *  pair. The N.I.N.A. Sync plugin has its own panel and its own switch. */
export default function PeerAccessPanel({ databaseId, databaseName, onChanged }: {
  databaseId: string;
  databaseName: string;
  /** After a change is saved, for pages that keep their own copy of the database list. */
  onChanged?: () => void;
}) {
  const { canWrite } = useAccess();
  const [code, setCode] = useState<{ token: string; expiresAt: number } | null>(null);
  const [copy, setCopy] = useState<'idle' | 'copied' | 'failed'>('idle');
  // The other server pairs from its own Settings, so the page looks while
  // a code waits.
  const { loaded, database, upload, act, save, busy } = useRemoteReceiver(databaseId, { onChanged, pollMs: code ? 3_000 : false });
  const peers = (upload?.clients ?? []).filter(entry => entry.kind === 'peer');
  // A server that pairs answers the code: it is spent.
  const pairedCount = useRef(peers.length);
  useEffect(() => {
    if (peers.length > pairedCount.current) setCode(null);
    pairedCount.current = peers.length;
  }, [peers.length]);
  // An expired code can no longer pair: it goes, and so does the polling.
  useEffect(() => {
    if (!code) return;
    const timer = window.setTimeout(() => setCode(null), Math.max(0, code.expiresAt - Date.now()));
    return () => window.clearTimeout(timer);
  }, [code]);

  if (!canWrite) return null;
  if (loaded.isPending) return <p role="status">Loading paired PSF Guard servers...</p>;
  if (!database) return <p className="director-error" role="alert">{loaded.isError ? message(loaded.error) : 'This database is not registered.'}</p>;

  const keySet = upload?.key_configured ?? false;
  const syncOn = upload?.peer_sync_enabled ?? upload?.sync_enabled ?? false;
  const credential = peers.length > 0 || keySet;
  const address = serverAddress();
  const generate = () => act.mutate(async () => {
    const issued = await apiClient.issuePairingCode(databaseId, 'peer');
    setCopy('idle');
    setCode({ token: issued.pairing_token, expiresAt: issued.expires_at * 1000 });
  });
  const copyCode = async () => {
    if (!code) return;
    try { await navigator.clipboard.writeText(code.token); setCopy('copied'); } catch { setCopy('failed'); }
  };
  const remove = (name: string, clientUuid: string) => {
    if (window.confirm(`Remove ${name}? That server must pair again to sync with ${databaseName}.`)) {
      act.mutate(() => apiClient.revokePairedClient(databaseId, clientUuid));
    }
  };

  return <section className="rig-profile-form rig-plugin plugin-card" aria-label="Other PSF Guard servers">
    <div className="director-toolbar plugin-card-heading">
      <h4>Other PSF Guard servers</h4>
      <span className="director-muted">Sync with {databaseName}</span>
    </div>
    {act.isError && <p className="director-error" role="alert">{message(act.error)}</p>}

    <fieldset>
      <legend>Pair</legend>
      <ul className="rig-plugin-rows" aria-label="Paired PSF Guard servers">
        {peers.map(entry => <li key={entry.client_uuid}>
          <span>{entry.name} <small className="director-muted">Paired {new Date(entry.paired_at * 1000).toLocaleDateString()} · {entry.last_contact ? `last sync ${ago(entry.last_contact.at * 1000)}` : 'no sync recorded'}</small></span>
          <button type="button" disabled={busy} aria-label={`Remove ${entry.name}`} onClick={() => remove(entry.name, entry.client_uuid)}><Trash2 size={14} />Remove</button>
        </li>)}
      </ul>
      {peers.length === 0 && !code && <p className="director-muted">No PSF Guard server is paired. Each server pairs with its own code, and you can remove one without affecting the others.</p>}
      <div className="director-actions">
        <button type="button" disabled={busy} onClick={generate}><KeyRound size={16} />Generate server pairing code</button>
      </div>
      {code && <div className="rig-plugin-code">
        <label className="rig-profile-field"><span>Pairing code</span>
          <span className="rig-profile-input"><input readOnly aria-label="Server pairing code" value={code.token} onFocus={event => event.target.select()} />
            <button type="button" onClick={() => void copyCode()}><Copy size={14} />{copy === 'copied' ? 'Copied' : 'Copy'}</button>
          </span>
        </label>
        {address && <p className="director-muted">Server URL: <code>{address}</code></p>}
        <p role="status">{copy === 'failed' ? 'Copy failed. Select and copy the code.' : `On the other PSF Guard, open Settings, Sync, Remote PSF Guard. Click Add peer, then enter this server's URL and the code. One use, expires ${new Date(code.expiresAt).toLocaleTimeString()}. Pairing turns on sync for other servers. This page updates when the server pairs.`}</p>
      </div>}
    </fieldset>

    <fieldset>
      <legend>Sync</legend>
      <p className="plugin-state" data-on={syncOn} data-testid="peer-sync-state">{!syncOn
        ? `Off. Other PSF Guard servers and the remote API key cannot merge projects, targets, plans or grades into ${databaseName}.`
        : credential
          ? `On. Paired PSF Guard servers${keySet ? ' and the remote API key' : ''} merge projects, targets, plans and grades into ${databaseName}.`
          : 'On, but no server is paired and no remote API key is set, so nothing arrives.'}</p>
      {!syncOn && !credential && <p className="director-muted">Pair a server or set a remote API key first.</p>}
      {(syncOn || credential) && <div className="director-actions">
        {syncOn
          ? <button type="button" disabled={busy} onClick={() => save({ peer_sync_enabled: false })}><Power size={16} />Turn off sync for other servers</button>
          : <button type="button" disabled={busy} onClick={() => save({ peer_sync_enabled: true })}><Check size={16} />Turn on sync for other servers</button>}
      </div>}
    </fieldset>

    <KeySettings upload={upload} busy={busy}
      onSave={(token, onSaved) => save({ token }, onSaved)}
      onRemove={() => { if (window.confirm('Remove the remote API key? Anything using it stops at once. Paired servers and plugins keep working.')) save({ clear_token: true }); }} />
  </section>;
}

/** The single remote API key, for scripts and clients that cannot pair. */
function KeySettings({ upload, busy, onSave, onRemove }: {
  upload: RemoteImageUploadSummary | undefined;
  busy: boolean;
  onSave: (token: string, onSaved: () => void) => void;
  onRemove: () => void;
}) {
  const [key, setKey] = useState('');
  const [copy, setCopy] = useState<'idle' | 'copied' | 'failed'>('idle');
  const set = upload?.key_configured ?? false;
  const contact = upload?.key_contact;
  const generate = () => {
    const bytes = new Uint8Array(32);
    crypto.getRandomValues(bytes);
    setKey(Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join(''));
    setCopy('idle');
  };
  const copyKey = async () => {
    try { await navigator.clipboard.writeText(key); setCopy('copied'); } catch { setCopy('failed'); }
  };
  return <fieldset>
    <legend>Remote API key</legend>
    <UnsavedMark label="remote API key" dirty={!!key} />
    <p className="plugin-state" data-on={set} data-testid="sync-key-state">{set ? 'Set.' : 'Not set.'} <span className="director-muted">One shared key for scripts, and for a PSF Guard server or Sync plugin too old to pair. Pair with a code where you can.</span></p>
    {set && <p className="director-muted">{contact ? `Last used ${ago(contact.at * 1000)}${contact.version ? `, by Sync plugin ${contact.version}` : ''}.` : 'No use recorded.'}</p>}
    {key
      ? <div className="rig-plugin-code">
        <label className="rig-profile-field"><span>New key</span>
          <span className="rig-profile-input"><input readOnly aria-label="New remote API key" value={key} onFocus={event => event.target.select()} />
            <button type="button" onClick={() => void copyKey()}><Copy size={14} />{copy === 'copied' ? 'Copied' : 'Copy'}</button>
          </span>
        </label>
        <p role="status">{copy === 'failed' ? 'Copy failed. Select and copy the key.' : 'Copy this key now. It is not shown again after you save it.'}</p>
        <div className="director-actions">
          <button type="button" disabled={busy} onClick={() => onSave(key, () => setKey(''))}><Save size={16} />Save key</button>
          <button type="button" disabled={busy} onClick={() => setKey('')}>Cancel</button>
        </div>
      </div>
      : <div className="director-actions">
        <button type="button" disabled={busy} onClick={generate}><KeyRound size={16} />{set ? 'Replace key' : 'Generate key'}</button>
        {set && <button type="button" disabled={busy} onClick={onRemove}><Trash2 size={16} />Remove key</button>}
      </div>}
  </fieldset>;
}
