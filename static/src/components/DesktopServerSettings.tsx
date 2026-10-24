import { useState, type ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { desktopServer, isTauriApp, tauriConfig } from '../utils/tauri';
import OpenLogsButton from './OpenLogsButton';
import './DesktopServerSettings.css';
import UnsavedMark from './UnsavedMark';

const KEY = ['desktopServerSettings'] as const;

function useServerSettings() {
  return useQuery({
    queryKey: KEY,
    queryFn: desktopServer.getSettings,
    enabled: isTauriApp(),
    retry: false,
    refetchInterval: query => query.state.data?.starting ? 250 : false,
  });
}

/** Runs before HTTP access checks, so an occupied port never connects this
 *  desktop app to a different process, and Settings can recover without HTTP. */
export function DesktopServerGate({ children }: { children: ReactNode }) {
  const settings = useServerSettings();
  const [logError, setLogError] = useState('');
  if (!isTauriApp()) return children;
  if (settings.isPending || settings.data?.starting) {
    return <div className="auth-page auth-loading">Starting desktop server…</div>;
  }
  if (settings.data?.server_url && !settings.data.error) return children;
  return <main className="auth-page"><div className="auth-card">
    <h1>Desktop server settings</h1>
    <p>The desktop server could not start. Choose another port or Automatic, then restart the server.</p>
    <DesktopServerSettings />
    <OpenLogsButton onError={setLogError} />
    {logError && <p role="alert" className="error-text">{logError}</p>}
  </div></main>;
}

export default function DesktopServerSettings({
  onRestarted = () => window.location.reload(),
}: { onRestarted?: () => void }) {
  const client = useQueryClient();
  const settings = useServerSettings();
  // A choice without an edit (null) shows the saved port, so a refetch
  // updates it; an edited one keeps what was chosen until Save.
  const [fixedDraft, setFixed] = useState<boolean | null>(null);
  const [valueDraft, setValue] = useState<string | null>(null);
  const [validation, setValidation] = useState('');
  const save = useMutation({
    mutationFn: desktopServer.savePort,
    onSuccess: saved => {
      client.setQueryData(KEY, saved);
      setFixed(null);
      setValue(null);
    },
  });
  const restart = useMutation({
    mutationFn: tauriConfig.restartServer,
    onSuccess: onRestarted,
    onError: () => { void client.invalidateQueries({ queryKey: KEY }); },
  });
  if (settings.isPending) return <p>Loading server settings…</p>;
  if (settings.isError) return <div className="settings-section">
    <p className="error-text" role="alert">Could not load desktop server settings: {settings.error.message}</p>
    <button type="button" onClick={() => settings.refetch()}>Try again</button>
  </div>;
  const current = settings.data;
  const fixed = fixedDraft ?? current.server_port !== null;
  const value = valueDraft ?? (current.server_port === null ? '' : String(current.server_port));
  const port = fixed ? Number(value) : null;
  const dirty = port !== current.server_port;
  const busy = save.isPending || restart.isPending || current.starting;
  const pending = !current.server_url || current.server_port !== current.active_server_port;
  const error = validation || save.error?.message || restart.error?.message || current.error;
  const clearError = () => { setValidation(''); save.reset(); restart.reset(); };

  return <section className="desktop-server-settings" aria-label="Desktop server">
      <UnsavedMark label="server port" dirty={dirty} />
    <h3>Server</h3>
    <p>The desktop app, browser access, API, and agents share this localhost port.
      Choose Fixed port to keep the address across restarts.</p>
    {current.server_url && <p>Running at <code>{current.server_url}</code></p>}
    {error && <p className="error-text" role="alert">{error}</p>}
    <form onSubmit={event => {
      event.preventDefault();
      if (fixed && (!value || !Number.isInteger(port) || port! < 1024 || port! > 65535)) {
        setValidation('Choose a server port from 1024 to 65535, or Automatic.');
        return;
      }
      setValidation('');
      save.mutate(port);
    }}>
      <fieldset className="calibration-settings-group" disabled={busy}>
        <legend>Port selection</legend>
        <label className="desktop-port-choice"><input type="radio" name="desktop-port-mode" checked={!fixed}
          onChange={() => { clearError(); setFixed(false); }} /> Automatic</label>
        <label className="desktop-port-choice"><input type="radio" name="desktop-port-mode" checked={fixed}
          onChange={() => { clearError(); setFixed(true); }} /> Fixed port</label>
        {fixed && <label className="desktop-port-number">Server port <input type="number" min={1024} max={65535} step={1} required
          value={value} onChange={event => { clearError(); setValue(event.target.value); }} /></label>}
        <p className="muted">Automatic chooses a free port on each start. A fixed port must be
          1024–65535 and available on this computer.</p>
        <button type="submit" className="save-button" disabled={!dirty && !current.error}>{save.isPending ? 'Saving…' : 'Save port'}</button>
      </fieldset>
    </form>
    {pending && <p role="status">{current.server_port === null ? 'Automatic port selection is saved.'
      : `Port ${current.server_port} is saved.`} Restart the server to apply it.</p>}
    <button type="button" className="cancel-button" disabled={busy || dirty} onClick={() => restart.mutate()}>
      {restart.isPending ? 'Restarting server…' : 'Restart server'}
    </button>
    <p className="muted">A restart waits for running work to finish safely, then reloads the interface.</p>
  </section>;
}
