import { useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ProcessingSetupsDocument } from '../api/types';
import { useAccess } from '../auth/access';
import { formatRelativeTime } from '../utils/relativeTime';

const SETUPS_QUERY_KEY = ['processing-setups'] as const;

const kindLabels = { view: 'View processing', color: 'Color pipeline' } as const;

/**
 * The management side of named processing setups, in Settings: the full list
 * across both editors, deletion, and moving the collection between installs.
 * Saving and applying stay in the processing editors, next to the parameters
 * they capture.
 */
export default function ProcessingSetupsManager() {
  const access = useAccess();
  const queryClient = useQueryClient();
  const fileRef = useRef<HTMLInputElement>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const setups = useQuery({
    queryKey: SETUPS_QUERY_KEY,
    queryFn: apiClient.getProcessingSetups,
    staleTime: 30_000,
  });
  const list = setups.data?.setups ?? [];

  const report = (message: string) => {
    setError(null);
    setNotice(message);
  };
  const fail = (cause: unknown, fallback: string) => {
    setNotice(null);
    setError(cause instanceof Error ? cause.message : fallback);
  };
  const refresh = () => queryClient.invalidateQueries({ queryKey: SETUPS_QUERY_KEY });

  const remove = useMutation({
    mutationFn: (name: string) => apiClient.deleteProcessingSetup(name),
    onSuccess: (_, name) => {
      refresh();
      report(`Deleted “${name}”`);
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

  const busy = remove.isPending || importSetups.isPending;

  return (
    <div className="settings-section processing-setups-manager">
      <div className="user-management-heading">
        <div>
          <h3>Processing setups</h3>
          <p>
            Named parameter sets for the stack processing editors, shared by
            every database. Save and apply them from <strong>View
            processing</strong> and the color <strong>Processing stack</strong>;
            manage the collection here. One exported file moves all of them to
            another install.
          </p>
        </div>
        <div className="processing-setups-manager-actions">
          {access.canWrite && (
            <>
              <button
                type="button"
                className="add-directory-button"
                disabled={busy}
                onClick={() => fileRef.current?.click()}
              >
                Import…
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
            className="browse-button"
            disabled={busy || list.length === 0}
            onClick={exportSetups}
          >
            Export all
          </button>
        </div>
      </div>

      {notice && <div className="processing-setups-notice">{notice}</div>}
      {error && <div className="processing-setups-error" role="alert">{error}</div>}

      {list.length === 0 ? (
        <p className="processing-setups-empty">
          Nothing saved yet. Configure a stretch or a color pipeline on any
          stack card, then choose <strong>Save as…</strong> in its Setups bar.
        </p>
      ) : (
        <table className="processing-setups-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Editor</th>
              <th>Updated</th>
              {access.canWrite && <th aria-label="Actions" />}
            </tr>
          </thead>
          <tbody>
            {list.map((setup) => (
              <tr key={`${setup.kind}:${setup.name}`}>
                <td>{setup.name}</td>
                <td>{kindLabels[setup.kind]}</td>
                <td>{formatRelativeTime(setup.updated_unix_seconds)}</td>
                {access.canWrite && (
                  <td>
                    <button
                      type="button"
                      className="remove-button"
                      disabled={busy}
                      onClick={() => remove.mutate(setup.name)}
                    >
                      Delete
                    </button>
                  </td>
                )}
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
