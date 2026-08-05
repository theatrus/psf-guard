import { useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ProcessingSetupKind, ProcessingSetupsDocument } from '../api/types';
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
 * Save, apply, import, and export named processing setups. Setups are global —
 * one list serves every database — so this bar is the same everywhere it
 * appears; only `kind` scopes which setups it lists.
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
  const fileRef = useRef<HTMLInputElement>(null);
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
  const remove = useMutation({
    mutationFn: (name: string) => apiClient.deleteProcessingSetup(name),
    onSuccess: () => {
      refresh();
      setSelected('');
      report('Setup deleted');
    },
    onError: (cause) => fail(cause, 'Deleting the setup failed'),
  });
  const importSetups = useMutation({
    mutationFn: (document: ProcessingSetupsDocument) =>
      apiClient.importProcessingSetups(document),
    onSuccess: (result) => {
      refresh();
      report(`Imported ${result.imported} new, replaced ${result.replaced}`);
    },
    onError: (cause) => fail(cause, 'Importing setups failed'),
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

  const exportSetups = () => {
    const document_ = setups.data ?? { schema_version: 1, setups: [] };
    const blob = new Blob([`${JSON.stringify(document_, null, 2)}\n`], {
      type: 'application/json',
    });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = 'psf-guard-processing-setups.json';
    anchor.click();
    URL.revokeObjectURL(url);
  };

  const importFile = async (file: File) => {
    try {
      const parsed: unknown = JSON.parse(await file.text());
      if (
        typeof parsed !== 'object' || parsed === null ||
        !Array.isArray((parsed as ProcessingSetupsDocument).setups)
      ) {
        throw new Error('This file is not a processing setups export');
      }
      importSetups.mutate(parsed as ProcessingSetupsDocument);
    } catch (cause) {
      fail(cause, 'Importing setups failed');
    }
  };

  const selectedSaved = selected.startsWith(SAVED_PREFIX)
    ? selected.slice(SAVED_PREFIX.length)
    : null;
  const busy = disabled || save.isPending || remove.isPending || importSetups.isPending;

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
          <>
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
            {selectedSaved && (
              <button
                type="button"
                disabled={busy}
                onClick={() => remove.mutate(selectedSaved)}
              >
                Delete
              </button>
            )}
            <button
              type="button"
              disabled={busy}
              onClick={() => fileRef.current?.click()}
            >
              Import
            </button>
            <input
              ref={fileRef}
              type="file"
              accept="application/json,.json"
              hidden
              onChange={(event) => {
                const file = event.target.files?.[0];
                event.target.value = '';
                if (file) void importFile(file);
              }}
            />
          </>
        )}
        <button
          type="button"
          disabled={busy || (setups.data?.setups.length ?? 0) === 0}
          onClick={exportSetups}
        >
          Export
        </button>
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
