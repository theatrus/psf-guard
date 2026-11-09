import { useEffect, useRef, useState } from 'react';
import { Check, Copy, KeyRound, Power, Save, Trash2 } from 'lucide-react';
import UnsavedMark from '../UnsavedMark';
import { apiClient } from '../../api/client';
import type { RemoteImageUploadSummary } from '../../api/types';
import { useAccess } from '../../auth/access';
import { contactText, serverAddress } from './pluginShared';
import { requestMessage as message, useRemoteReceiver } from './remoteReceiver';
import '../director/DirectorPage.css';

/** The PSF Guard Sync plugin on a rig: who is paired, scheduler sync and
 *  the remote API key. Image uploads, which both plugins use, have their
 *  own panel. Built like the Director plugin's panel: each part says
 *  whether it is on and acts at once. */
export default function SyncPluginPanel({ databaseId, onChanged }: {
  databaseId: string;
  /** After a change is saved, for pages that keep their own copy of the database list. */
  onChanged?: () => void;
}) {
  const { canWrite } = useAccess();
  const [code, setCode] = useState<{ token: string; expiresAt: number } | null>(null);
  const [copy, setCopy] = useState<'idle' | 'copied' | 'failed'>('idle');
  // The plugin pairs from N.I.N.A., so the page looks while a code waits,
  // and again whenever you come back to it.
  const { loaded, database, upload, act, save } = useRemoteReceiver(databaseId, { onChanged, pollMs: code ? 3_000 : false });
  const clients = upload?.clients ?? [];
  // A plugin that pairs answers the code: it is spent.
  const pairedCount = useRef(clients.length);
  useEffect(() => {
    if (clients.length > pairedCount.current) setCode(null);
    pairedCount.current = clients.length;
  }, [clients.length]);
  // An expired code can no longer pair: it goes, and so does the polling.
  useEffect(() => {
    if (!code) return;
    const timer = window.setTimeout(() => setCode(null), Math.max(0, code.expiresAt - Date.now()));
    return () => window.clearTimeout(timer);
  }, [code]);

  if (!canWrite) return <p className="director-muted">Sign in as an editor to manage the Sync plugin.</p>;
  if (loaded.isPending) return <p role="status">Loading the Sync plugin...</p>;
  if (!database) return <p className="director-error" role="alert">{loaded.isError ? message(loaded.error) : 'This database is not registered.'}</p>;

  const credential = upload?.token_configured ?? false;
  const syncOn = upload?.sync_enabled ?? false;
  const address = serverAddress();
  const generate = () => act.mutate(async () => {
    const issued = await apiClient.issuePairingCode(databaseId);
    setCopy('idle');
    setCode({ token: issued.pairing_token, expiresAt: issued.expires_at * 1000 });
  });
  const copyCode = async () => {
    if (!code) return;
    try { await navigator.clipboard.writeText(code.token); setCopy('copied'); } catch { setCopy('failed'); }
  };
  const remove = (name: string, clientUuid: string) => {
    if (window.confirm(`Remove ${name}? The plugin must pair again to reach this database.`)) {
      act.mutate(() => apiClient.revokePairedClient(databaseId, clientUuid));
    }
  };
  const older = [
    ...clients.filter(entry => entry.last_contact && !entry.last_contact.places_new_projects).map(entry => entry.name),
    ...(upload?.key_contact && !upload.key_contact.places_new_projects ? ['The client using the remote API key'] : []),
  ];

  return <section className="rig-profile-form rig-plugin plugin-card" aria-label="Sync plugin">
    <div className="director-toolbar plugin-card-heading">
      <h4>PSF Guard Sync</h4>
      <span className="director-muted">Target Scheduler</span>
    </div>
    {act.isError && <p className="director-error" role="alert">{message(act.error)}</p>}

    <fieldset>
      <legend>Pair</legend>
      <ul className="rig-plugin-rows" aria-label="Paired Sync plugins">
        {clients.map(entry => <li key={entry.client_uuid}>
          <span>{entry.name} <small className="director-muted">Paired {new Date(entry.paired_at * 1000).toLocaleDateString()} · {contactText(entry.last_contact)}</small></span>
          <button type="button" disabled={act.isPending} aria-label={`Remove ${entry.name}`} onClick={() => remove(entry.name, entry.client_uuid)}><Trash2 size={14} />Remove</button>
        </li>)}
      </ul>
      {older.length > 0 && <p className="director-muted" role="note">{older.join(', ')} {older.length === 1 ? 'is' : 'are'} running an older Sync plugin. Update the plugin in N.I.N.A., or N.I.N.A. may not show new projects.</p>}
      {clients.length === 0 && !code && <p className="director-muted">Install PSF Guard Sync from the N.I.N.A. plugin repository <code>https://nina-plugins.psf-guard.com/</code>, restart N.I.N.A., then generate a code.</p>}
      <div className="director-actions">
        <button type="button" disabled={act.isPending} onClick={generate}><KeyRound size={16} />Generate Sync pairing code</button>
      </div>
      {code && <div className="rig-plugin-code">
        <label className="rig-profile-field"><span>Pairing code</span>
          <span className="rig-profile-input"><input readOnly aria-label="Sync pairing code" value={code.token} onFocus={event => event.target.select()} />
            <button type="button" onClick={() => void copyCode()}><Copy size={14} />{copy === 'copied' ? 'Copied' : 'Copy'}</button>
          </span>
        </label>
        {address && <p className="director-muted">Server URL: <code>{address}</code></p>}
        <p role="status">{copy === 'failed' ? 'Copy failed. Select and copy the code.' : `In N.I.N.A., open the PSF Guard Sync settings, enter the server URL and this code, click Pair, then Test connection. One use, expires ${new Date(code.expiresAt).toLocaleTimeString()}. Pairing turns on scheduler sync. This page updates when the plugin pairs.`}</p>
      </div>}
    </fieldset>

    <fieldset>
      <legend>Scheduler sync</legend>
      <p className="plugin-state" data-on={syncOn} data-testid="sync-scheduler-state">{!syncOn
        ? 'Off. Paired plugins cannot merge projects, targets, plans or grades into this database.'
        : credential
          ? 'On. Paired plugins merge projects, targets, plans and grades into this database.'
          : 'On, but no plugin is paired, so nothing arrives.'}</p>
      {!syncOn && !credential && <p className="director-muted">Pair the plugin first.</p>}
      {(syncOn || credential) && <div className="director-actions">
        {syncOn
          ? <button type="button" disabled={act.isPending} onClick={() => save({ sync_enabled: false })}><Power size={16} />Turn off scheduler sync</button>
          : <button type="button" disabled={act.isPending} onClick={() => save({ sync_enabled: true })}><Check size={16} />Turn on scheduler sync</button>}
      </div>}
    </fieldset>

    <KeySettings upload={upload} busy={act.isPending} onSave={(token, onSaved) => save({ token }, onSaved)} />
  </section>;
}

/** The single remote API key, for scripts and clients that do not pair. */
function KeySettings({ upload, busy, onSave }: {
  upload: RemoteImageUploadSummary | undefined;
  busy: boolean;
  onSave: (token: string, onSaved: () => void) => void;
}) {
  const [key, setKey] = useState('');
  const [copy, setCopy] = useState<'idle' | 'copied' | 'failed'>('idle');
  const set = upload?.key_configured ?? false;
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
    <p className="plugin-state" data-on={set} data-testid="sync-key-state">{set ? 'Set.' : 'Not set.'} <span className="director-muted">For scripts and clients that cannot pair. The Sync plugin pairs with a code instead.</span></p>
    {upload?.key_contact && <p className="director-muted">{contactText(upload.key_contact)}</p>}
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
      </div>}
  </fieldset>;
}
