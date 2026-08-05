import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ProcessingSetupKind } from '../api/types';
import { useAccess } from '../auth/access';

const SETUPS_QUERY_KEY = ['processing-setups'] as const;
const BUILTIN_PREFIX = 'builtin:';
const SAVED_PREFIX = 'saved:';

interface ProcessingSetupsBarProps {
  kind: ProcessingSetupKind;
  /** Built-ins for this editor, already reconciled to the current card. */
  builtins: { name: string; settings: unknown }[];
  /** The editor's current parameters, captured when Save is pressed. */
  current: () => unknown;
  /** Load stored settings into the editor. The parent reconciles roles. */
  onApply: (settings: unknown) => void;
  disabled?: boolean;
}

/**
 * Apply a named processing setup, or save the editor's current parameters as
 * one. Setups are global — one list serves every database — so this bar is
 * the same everywhere it appears; only `kind` scopes which setups it lists.
 * Deleting, importing, and exporting live in Settings → Setups.
 */
export default function ProcessingSetupsBar({
  kind,
  builtins,
  current,
  onApply,
  disabled = false,
}: ProcessingSetupsBarProps) {
  const access = useAccess();
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState('');
  const [saveName, setSaveName] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const setups = useQuery({
    queryKey: SETUPS_QUERY_KEY,
    queryFn: apiClient.getProcessingSetups,
    staleTime: 30_000,
  });
  const saved = (setups.data?.setups ?? []).filter((setup) => setup.kind === kind);

  const report = (message: string) => {
    setError(null);
    setNotice(message);
  };
  const fail = (cause: unknown, fallback: string) => {
    setNotice(null);
    setError(cause instanceof Error ? cause.message : fallback);
  };
  const refresh = () => queryClient.invalidateQueries({ queryKey: SETUPS_QUERY_KEY });

  const save = useMutation({
    mutationFn: (name: string) => apiClient.saveProcessingSetup(name, kind, current()),
    onSuccess: (setup) => {
      refresh();
      setSaveName(null);
      setSelected(`${SAVED_PREFIX}${setup.name}`);
      report(`Saved “${setup.name}”`);
    },
    onError: (cause) => fail(cause, 'Saving the setup failed'),
  });
  const apply = () => {
    if (selected.startsWith(BUILTIN_PREFIX)) {
      const setup = builtins.find(
        (candidate) => candidate.name === selected.slice(BUILTIN_PREFIX.length)
      );
      if (setup) {
        onApply(setup.settings);
        report(`Applied “${setup.name}” — press Apply processing to render it`);
      }
      return;
    }
    const setup = saved.find(
      (candidate) => candidate.name === selected.slice(SAVED_PREFIX.length)
    );
    if (setup) {
      onApply(setup.settings);
      report(`Applied “${setup.name}” — press Apply processing to render it`);
    }
  };

  const selectedSaved = selected.startsWith(SAVED_PREFIX)
    ? selected.slice(SAVED_PREFIX.length)
    : null;
  const busy = disabled || save.isPending;

  return (
    <div className="processing-setups-bar">
      <div className="processing-setups-row">
        <span className="processing-setups-label">Setups</span>
        <select
          aria-label={`Saved ${kind} processing setups`}
          value={selected}
          disabled={busy}
          onChange={(event) => {
            setSelected(event.target.value);
            setNotice(null);
            setError(null);
          }}
        >
          <option value="">Choose a setup…</option>
          <optgroup label="Built-in">
            {builtins.map((setup) => (
              <option key={setup.name} value={`${BUILTIN_PREFIX}${setup.name}`}>
                {setup.name}
              </option>
            ))}
          </optgroup>
          {saved.length > 0 && (
            <optgroup label="Saved">
              {saved.map((setup) => (
                <option key={setup.name} value={`${SAVED_PREFIX}${setup.name}`}>
                  {setup.name}
                </option>
              ))}
            </optgroup>
          )}
        </select>
        <button type="button" disabled={busy || !selected} onClick={apply}>
          Apply setup
        </button>
        {access.canWrite && (
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setSaveName(saveName === null ? (selectedSaved ?? '') : null);
              setNotice(null);
              setError(null);
            }}
          >
            Save as…
          </button>
        )}
        <span className="processing-setups-hint">
          Manage, import, and export in Settings → Setups
        </span>
      </div>
      {saveName !== null && (
        <form
          className="processing-setups-row"
          onSubmit={(event) => {
            event.preventDefault();
            if (saveName.trim()) save.mutate(saveName.trim());
          }}
        >
          <input
            type="text"
            value={saveName}
            maxLength={64}
            placeholder="Setup name"
            aria-label="New setup name"
            onChange={(event) => setSaveName(event.target.value)}
          />
          <button type="submit" disabled={busy || !saveName.trim()}>
            {save.isPending ? 'Saving…' : 'Save current settings'}
          </button>
          <button type="button" disabled={busy} onClick={() => setSaveName(null)}>
            Cancel
          </button>
        </form>
      )}
      {notice && <div className="processing-setups-notice">{notice}</div>}
      {error && <div className="processing-setups-error" role="alert">{error}</div>}
    </div>
  );
}
