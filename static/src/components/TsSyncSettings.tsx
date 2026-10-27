import { useEffect, useState } from 'react';
import UnsavedMark from './UnsavedMark';
import { apiClient } from '../api/client';
import { type TsSyncSettings as Settings } from '../api/tsSyncTypes';

export default function TsSyncSettings({ dbId, settings, onSaved }: {
  dbId: string; settings: Settings; onSaved: () => Promise<void>;
}) {
  const [value, setValue] = useState(settings);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const saved = JSON.stringify(settings);
  const dirty = JSON.stringify(value) !== saved;
  useEffect(() => setValue(JSON.parse(saved) as Settings), [saved]);
  return <section aria-label="Target Scheduler sync" className="settings-section">
    <UnsavedMark label="Target Scheduler sync" dirty={dirty} />
    <h4>Target Scheduler sync</h4>
    <p>PSF Guard uses its own database. Sync imports new captures from <code>{settings.source_path}</code>.</p>
    <form onSubmit={event => {
      event.preventDefault();
      setBusy(true); setError('');
      void apiClient.updateDatabase(dbId, { ts_sync: value })
        .then(onSaved).catch(error => setError(error instanceof Error ? error.message : String(error)))
        .finally(() => setBusy(false));
    }}>
      <fieldset className="autoimport-fields" disabled={busy}>
        <legend>Automatic file sync</legend>
        <label className="checkbox-label"><input type="checkbox" checked={value.on_open} onChange={e => setValue({ ...value, on_open: e.target.checked })} /> Sync when the desktop app or server starts</label>
        <label className="checkbox-label"><input type="checkbox" checked={value.on_change} onChange={e => setValue({ ...value, on_change: e.target.checked })} /> Sync when new images or grades arrive</label>
        <label>Sync interval (minutes; 0 disables the schedule)<input type="number" min={0} max={10080} required value={value.interval_minutes}
          onChange={e => setValue({ ...value, interval_minutes: Number(e.target.value) })} /></label>
        <label className="checkbox-label"><input type="checkbox" checked={value.writeback} onChange={e => setValue({ ...value, writeback: e.target.checked })} /> Automatically write grade changes back to Target Scheduler</label>
        <p>Automatic writeback leaves conflicting TS grades unchanged. When this is off, you can still use Sync to preview and apply planning or grade transfers.</p>
        <button type="submit" disabled={!dirty}>Save sync settings</button>
      </fieldset>
    </form>
    {error && <p role="alert" className="error-text">{error}</p>}
  </section>;
}
