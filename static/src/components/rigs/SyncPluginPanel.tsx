import { useEffect, useState } from 'react';
import { Check, Copy, KeyRound, Power, Trash2 } from 'lucide-react';
import { apiClient } from '../../api/client';
import { usePairingCode } from './usePairingCode';
import { useAccess } from '../../auth/access';
import { contactText, serverAddress } from './pluginShared';
import { requestMessage as message, useRemoteReceiver } from './remoteReceiver';
import '../director/DirectorPage.css';

/** The PSF Guard Sync plugin on a rig: who is paired, and scheduler sync.
 *  Image uploads, which both plugins use, have their own panel, and so do
 *  other PSF Guard servers and the remote API key (`PeerAccessPanel`). Built like the Director plugin's panel: each part says
 *  whether it is on and acts at once. */
export default function SyncPluginPanel({ databaseId, onChanged }: {
  databaseId: string;
  /** After a change is saved, for pages that keep their own copy of the database list. */
  onChanged?: () => void;
}) {
  const { canWrite } = useAccess();
  // The plugin pairs from N.I.N.A., so the page looks while a code waits,
  // and again whenever you come back to it.
  const [waiting, setWaiting] = useState(false);
  const { loaded, database, upload, act, save, busy } = useRemoteReceiver(databaseId, { onChanged, pollMs: waiting ? 3_000 : false });
  // Another PSF Guard pairs too; those are listed with the peers.
  const clients = (upload?.clients ?? []).filter(entry => entry.kind !== 'peer');
  const { code, copy, issue, copyCode } = usePairingCode(databaseId, 'plugin', clients.length);
  useEffect(() => setWaiting(!!code), [code]);

  if (!canWrite) return <p className="director-muted">Sign in as an editor to manage the Sync plugin.</p>;
  if (loaded.isPending) return <p role="status">Loading the Sync plugin...</p>;
  if (!database) return <p className="director-error" role="alert">{loaded.isError ? message(loaded.error) : 'This database is not registered.'}</p>;

  const credential = clients.length > 0;
  const syncOn = upload?.sync_enabled ?? false;
  const address = serverAddress();
  const generate = () => act.mutate(issue);
  const remove = (name: string, clientUuid: string) => {
    if (window.confirm(`Remove ${name}? The plugin must pair again to reach this database.`)) {
      act.mutate(() => apiClient.revokePairedClient(databaseId, clientUuid));
    }
  };
  const older = clients.filter(entry => entry.last_contact && !entry.last_contact.places_new_projects).map(entry => entry.name);

  return <section className="rig-profile-form rig-plugin plugin-card" aria-label="PSF Guard Sync">
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
          <button type="button" disabled={busy} aria-label={`Remove ${entry.name}`} onClick={() => remove(entry.name, entry.client_uuid)}><Trash2 size={14} />Remove</button>
        </li>)}
      </ul>
      {older.length > 0 && <p className="director-muted" role="note">{older.join(', ')} {older.length === 1 ? 'is' : 'are'} running an older Sync plugin. Update the plugin in N.I.N.A., or N.I.N.A. may not show new projects.</p>}
      {clients.length === 0 && !code && <p className="director-muted">Install PSF Guard Sync from the N.I.N.A. plugin repository <code>https://nina-plugins.psf-guard.com/</code>, restart N.I.N.A., then generate a code.</p>}
      <div className="director-actions">
        <button type="button" disabled={busy} onClick={generate}><KeyRound size={16} />Generate Sync pairing code</button>
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
          ? <button type="button" disabled={busy} onClick={() => save({ sync_enabled: false })}><Power size={16} />Turn off scheduler sync</button>
          : <button type="button" disabled={busy} onClick={() => save({ sync_enabled: true })}><Check size={16} />Turn on scheduler sync</button>}
      </div>}
    </fieldset>
  </section>;
}
