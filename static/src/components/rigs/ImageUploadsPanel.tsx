import { useState } from 'react';
import { Check, Power, RefreshCw, Save } from 'lucide-react';
import UnsavedMark from '../UnsavedMark';
import type { RemoteImageUploadPlacement, RemoteImageUploadSummary } from '../../api/types';
import { useAccess } from '../../auth/access';
import { DEFAULT_UPLOAD_LAYOUT, isUploadLayoutPreset, UPLOAD_LAYOUT_PRESETS } from './pluginShared';
import { requestMessage, useRemoteReceiver, type ReceiverChange } from './remoteReceiver';
import '../director/DirectorPage.css';

/** The database's image uploads, which the Sync and Director plugins both
 *  use: on or off, the receive folder and its layout. A plugin needs its
 *  own pairing to send files; turning uploads on needs none. */
export default function ImageUploadsPanel({ databaseId, onChanged }: { databaseId: string; onChanged?: () => void }) {
  const { canWrite } = useAccess();
  const { loaded, database, upload, act, save, busy } = useRemoteReceiver(databaseId, { onChanged });
  if (!canWrite || loaded.isPending || !database) return null;
  return <section className="rig-profile-form rig-plugin plugin-card" aria-label="Image uploads">
    <div className="director-toolbar plugin-card-heading">
      <h4>Image uploads</h4>
      <span className="director-muted">Sync and Director</span>
    </div>
    <p className="director-muted">The Sync plugin uploads frames from its upload instruction. The Director plugin uploads the frames it captures.</p>
    {act.isError && <p className="director-error" role="alert">{requestMessage(act.error)}</p>}
    <UploadSettings key={JSON.stringify([upload?.enabled, upload?.image_directory, upload?.placement, upload?.directory_template, upload?.directory_template_source, upload?.catalog_directory_template])}
      upload={upload} folders={database.image_directories} busy={busy} onSave={save} />
  </section>;
}

/** Image uploads: on or off, and where received files go. */
function UploadSettings({ upload, folders, busy, onSave }: {
  upload: RemoteImageUploadSummary | undefined;
  folders: string[];
  busy: boolean;
  onSave: (change: ReceiverChange, onSaved?: () => void) => void;
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
  const settings = (): ReceiverChange => ({
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
  // A save that changes nothing the summary shows keeps this form mounted.
  const saved = () => { setTouched(false); setRescan(false); };
  return <fieldset>
    <legend>Receive folder</legend>
    <UnsavedMark label="image upload settings" dirty={changed} />
    <p className="plugin-state" data-on={on} data-testid="sync-upload-state">{on
      ? `On. Received image files go to ${upload?.image_directory}.`
      : 'Off. PSF Guard takes no image files from either plugin.'}</p>
    {<>
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
            <button type="button" disabled={busy || !changed || !ready} onClick={() => onSave(settings(), saved)}><Save size={16} />Save upload settings</button>
            <button type="button" disabled={busy} onClick={() => onSave({ enabled: false })}><Power size={16} />Turn off image uploads</button>
          </>
          : <button type="button" disabled={busy || !ready} onClick={() => onSave({ ...settings(), enabled: true }, saved)}><Check size={16} />Turn on image uploads</button>}
      </div>
      {!folder && <p className="director-muted">Choose a receive folder.</p>}
      {folder && !ready && <p className="director-muted">Enter the custom layout's folders.</p>}
    </>}
  </fieldset>;
}

