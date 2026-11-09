import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Check, Copy, KeyRound, Power, RefreshCw, Save, Trash2 } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DatabaseSummary, RemoteImageUploadPlacement, RemoteImageUploadSummary } from '../../api/types';
import { useAccess } from '../../auth/access';
import { contactText, DEFAULT_UPLOAD_LAYOUT, isUploadLayoutPreset, serverAddress, UPLOAD_LAYOUT_PRESETS } from './pluginShared';
import '../director/DirectorPage.css';

type UploadChange = {
  enabled?: boolean;
  image_directory?: string;
  token?: string;
  sync_enabled?: boolean;
  placement?: RemoteImageUploadPlacement;
  directory_template?: string;
  directory_template_source?: 'catalog' | 'preset';
  rescan_directory_layout?: boolean;
};

const message = (error: unknown) => error instanceof Error ? error.message : 'The request failed';

/** What the database's receiver keeps, as an update that changes nothing. */
function unchanged(upload: RemoteImageUploadSummary | undefined) {
  return {
    enabled: upload?.enabled ?? false,
    image_directory: upload?.image_directory,
    sync_enabled: upload?.sync_enabled ?? false,
    placement: upload?.placement,
  };
}

/** The PSF Guard Sync plugin on a rig: who is paired, and what the database
 *  takes from it. Built like the Director plugin's panel: each part says
 *  whether it is on and acts at once. */
export default function SyncPluginPanel({ databaseId }: { databaseId: string }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const [code, setCode] = useState<{ token: string; expiresAt: number } | null>(null);
  const [copy, setCopy] = useState<'idle' | 'copied' | 'failed'>('idle');
  const loaded = useQuery({
    queryKey: ['databases'],
    queryFn: apiClient.getDatabases,
    // The plugin pairs from N.I.N.A., so the page looks while a code waits.
    refetchInterval: code ? 3_000 : false,
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
  const database: DatabaseSummary | undefined = loaded.data?.find(entry => entry.id === databaseId);
  const upload = database?.remote_image_upload;
  const clients = upload?.clients ?? [];
  const act = useMutation({
    retry: false,
    mutationFn: (work: () => Promise<unknown>) => work(),
    onSettled: () => { void client.invalidateQueries({ queryKey: ['databases'] }); },
  });
  const save = (change: UploadChange) => act.mutate(() => apiClient.updateDatabase(databaseId, {
    remote_image_upload: { ...unchanged(upload), ...change },
  }));
  // A plugin that pairs answers the code: it is spent.
  const pairedCount = useRef(clients.length);
  useEffect(() => {
    if (clients.length > pairedCount.current) setCode(null);
    pairedCount.current = clients.length;
  }, [clients.length]);

  if (!canWrite) return <p className="director-muted">Sign in as an editor to manage the Sync plugin.</p>;
  if (loaded.isPending) return <p role="status">Loading the Sync plugin...</p>;
  if (!database) return <p className="director-error" role="alert">{loaded.isError ? message(loaded.error) : 'This database is not registered.'}</p>;

  const credential = upload?.token_configured ?? false;
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

  return <section className="rig-profile-form rig-plugin" aria-label="Sync plugin">
    <div className="director-toolbar">
      <h4>Sync plugin</h4>
      <button type="button" aria-label="Reload Sync plugin" title="Reload Sync plugin" disabled={loaded.isFetching} onClick={() => void loaded.refetch()}><RefreshCw size={16} /></button>
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
      <p className="plugin-state" data-on={upload?.sync_enabled ?? false} data-testid="sync-scheduler-state">{upload?.sync_enabled
        ? 'On. Paired plugins merge projects, targets, plans and grades into this database.'
        : 'Off. Paired plugins cannot merge projects, targets, plans or grades into this database.'}</p>
      {!credential && <p className="director-muted">Pair the plugin first.</p>}
      {credential && <div className="director-actions">
        {upload?.sync_enabled
          ? <button type="button" disabled={act.isPending} onClick={() => save({ sync_enabled: false })}><Power size={16} />Turn off scheduler sync</button>
          : <button type="button" disabled={act.isPending} onClick={() => save({ sync_enabled: true })}><Check size={16} />Turn on scheduler sync</button>}
      </div>}
    </fieldset>

    <UploadSettings key={JSON.stringify([upload?.enabled, upload?.image_directory, upload?.placement, upload?.directory_template, upload?.directory_template_source, upload?.catalog_directory_template])}
      upload={upload} folders={database.image_directories} credential={credential} busy={act.isPending} onSave={save} />

    <KeySettings upload={upload} busy={act.isPending} onSave={token => save({ token })} />
  </section>;
}

/** Image uploads: on or off, and where received files go. */
function UploadSettings({ upload, folders, credential, busy, onSave }: {
  upload: RemoteImageUploadSummary | undefined;
  folders: string[];
  credential: boolean;
  busy: boolean;
  onSave: (change: UploadChange) => void;
}) {
  const on = upload?.enabled ?? false;
  const match = upload?.catalog_directory_template
    ? { template: upload.catalog_directory_template, samples: upload.directory_template_samples }
    : null;
  const [folder, setFolder] = useState(upload?.image_directory ?? '');
  const [placement, setPlacement] = useState<RemoteImageUploadPlacement>(upload?.placement ?? 'flat');
  const [choice, setChoice] = useState<'catalog' | 'preset'>(upload?.directory_template_source === 'catalog' && match ? 'catalog' : 'preset');
  const [template, setTemplate] = useState(upload?.directory_template || DEFAULT_UPLOAD_LAYOUT);
  const [touched, setTouched] = useState(false);
  const [rescan, setRescan] = useState(false);
  const changed = folder !== (upload?.image_directory ?? '') || placement !== (upload?.placement ?? 'flat') || touched || rescan;
  const tree = placement === 'target_tree';
  const settings = (): UploadChange => ({
    image_directory: folder,
    placement,
    // In catalog mode no layout is sent: the match stays, and the server
    // leaves the stored fallback alone.
    directory_template: tree && choice === 'preset' ? template.trim() : undefined,
    directory_template_source: tree && (touched || choice === 'catalog') ? choice : undefined,
    rescan_directory_layout: tree && rescan ? true : undefined,
  });
  const layoutValue = choice === 'catalog' && match ? 'catalog' : isUploadLayoutPreset(template) ? template : 'custom';
  // A custom layout needs its folders written out.
  const ready = !!folder && !(tree && choice === 'preset' && !template.trim());
  return <fieldset>
    <legend>Image uploads</legend>
    <p className="plugin-state" data-on={on} data-testid="sync-upload-state">{on
      ? `On. Paired plugins send image files to ${upload?.image_directory}.`
      : 'Off. Paired plugins do not send image files.'}</p>
    {!credential && <p className="director-muted">Pair the plugin first.</p>}
    {credential && <>
      <div className="rig-profile-grid">
        <label className="rig-profile-field" htmlFor="sync-upload-folder"><span>Receive folder</span>
          <span className="rig-profile-input"><select id="sync-upload-folder" value={folder} onChange={event => setFolder(event.target.value)}>
            <option value="">Choose an image folder</option>
            {folders.map(entry => <option key={entry} value={entry}>{entry}</option>)}
          </select></span>
        </label>
        <label className="rig-profile-field" htmlFor="sync-upload-placement"><span>Folder layout</span>
          <span className="rig-profile-input"><select id="sync-upload-placement" value={placement} onChange={event => setPlacement(event.target.value as RemoteImageUploadPlacement)}>
            <option value="flat">All in the receive folder</option>
            <option value="target_tree">Folders like the database's</option>
          </select></span>
        </label>
        {tree && <label className="rig-profile-field" htmlFor="sync-upload-layout"><span>Layout</span>
          <span className="rig-profile-input">
            <select id="sync-upload-layout" value={layoutValue} onChange={event => {
              const value = event.target.value;
              setTouched(true);
              if (value === 'catalog') { setChoice('catalog'); return; }
              setChoice('preset');
              setTemplate(value === 'custom' ? '' : value);
              // A rescan would find the database's match again.
              setRescan(false);
            }}>
              {match && <option value="catalog">Match database: {match.template}</option>}
              {UPLOAD_LAYOUT_PRESETS.map(preset => <option key={preset} value={preset}>{preset}</option>)}
              <option value="custom">Custom</option>
            </select>
            <button type="button" aria-pressed={rescan} disabled={rescan} title="Read the database's folders again when you save" onClick={() => setRescan(true)}><RefreshCw size={14} />{rescan ? 'Rescan on save' : 'Rescan'}</button>
          </span>
        </label>}
        {tree && choice === 'preset' && !isUploadLayoutPreset(template) && <label className="rig-profile-field" htmlFor="sync-upload-custom"><span>Custom layout</span>
          <span className="rig-profile-input"><input id="sync-upload-custom" value={template} maxLength={240} placeholder="%TARGET%/%NIGHT%/%TYPE%" onChange={event => { setTouched(true); setTemplate(event.target.value); }} /></span>
        </label>}
      </div>
      {tree && match && <p className="director-muted">Detected from {match.samples} database {match.samples === 1 ? 'image' : 'images'}: <code>{match.template}</code></p>}
      <div className="director-actions">
        {on
          ? <>
            <button type="button" disabled={busy || !changed || !ready} onClick={() => onSave(settings())}><Save size={16} />Save upload settings</button>
            <button type="button" disabled={busy} onClick={() => onSave({ enabled: false })}><Power size={16} />Turn off image uploads</button>
          </>
          : <button type="button" disabled={busy || !ready} onClick={() => onSave({ ...settings(), enabled: true })}><Check size={16} />Turn on image uploads</button>}
      </div>
      {!folder && <p className="director-muted">Choose a receive folder.</p>}
      {folder && !ready && <p className="director-muted">Enter the custom layout's folders.</p>}
    </>}
  </fieldset>;
}

/** The single remote API key, for scripts and clients that do not pair. */
function KeySettings({ upload, busy, onSave }: {
  upload: RemoteImageUploadSummary | undefined;
  busy: boolean;
  onSave: (token: string) => void;
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
          <button type="button" disabled={busy} onClick={() => { onSave(key); setKey(''); }}><Save size={16} />Save key</button>
          <button type="button" disabled={busy} onClick={() => setKey('')}>Cancel</button>
        </div>
      </div>
      : <div className="director-actions">
        <button type="button" disabled={busy} onClick={generate}><KeyRound size={16} />{set ? 'Replace key' : 'Generate key'}</button>
      </div>}
  </fieldset>;
}
