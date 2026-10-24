import { useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ProcessingSetup, ProcessingSetupsDocument } from '../api/types';
import { setupExportFilename } from './processingSetups';
import { PROCESSING_SETUPS_QUERY_KEY, processingSetupsQuery } from './defaultColorSetup';
import { useAccess } from '../auth/access';
import { formatRelativeTime } from '../utils/relativeTime';

const kindLabels = { view: 'View processing', color: 'Color pipeline' } as const;

/**
 * The management side of named processing setups, in Settings: the full list
 * across both editors, the default Color setup, deletion, and moving the
 * collection between installs. Saving and applying stay in the processing
 * editors, next to the parameters they capture.
 */
export default function ProcessingSetupsManager() {
  const access = useAccess();
  const queryClient = useQueryClient();
  const fileRef = useRef<HTMLInputElement>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const setups = useQuery(processingSetupsQuery());
  const list = setups.data?.setups ?? [];
  const defaultColor = setups.data?.default_color ?? null;
  const isDefault = (setup: ProcessingSetup) =>
    setup.kind === 'color' && defaultColor?.toLowerCase() === setup.name.toLowerCase();

  const report = (message: string) => {
    setError(null);
    setNotice(message);
  };
  const fail = (cause: unknown, fallback: string) => {
    setNotice(null);
    setError(cause instanceof Error ? cause.message : fallback);
  };
  const refresh = () => queryClient.invalidateQueries({ queryKey: PROCESSING_SETUPS_QUERY_KEY });

  const remove = useMutation({
    mutationFn: (name: string) => apiClient.deleteProcessingSetup(name),
    onSuccess: (_, name) => {
      refresh();
      report(`Deleted “${name}”`);
    },
    onError: (cause) => fail(cause, 'Deleting the setup failed'),
  });
  const markDefault = useMutation({
    mutationFn: (name: string) => apiClient.markDefaultColorSetup(name),
    onSuccess: (_, name) => {
      refresh();
      report(`New color previews now start from “${name}”`);
    },
    onError: (cause) => fail(cause, 'Marking the default setup failed'),
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

  const download = (document_: ProcessingSetupsDocument, filename: string) => {
    const blob = new Blob([`${JSON.stringify(document_, null, 2)}\n`], {
      type: 'application/json',
    });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = filename;
    anchor.click();
    URL.revokeObjectURL(url);
  };

  const exportAll = () => {
    download(
      { schema_version: setups.data?.schema_version ?? 1, setups: list },
      'psf-guard-processing-setups.json'
    );
  };

  // One setup in the same document shape, so any export feeds any import.
  const exportOne = (setup: ProcessingSetup) => {
    download(
      { schema_version: setups.data?.schema_version ?? 1, setups: [setup] },
      setupExportFilename(setup.name)
    );
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

  const busy = remove.isPending || importSetups.isPending || markDefault.isPending;

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
          <p>
            New color previews start from the default Color setup, including
            the first color preview an automatic refresh composes for a
            target. To change that pipeline, apply the setup on a color card,
            edit it, and save it under the same name, or make another Color
            setup the default. The default setup cannot be deleted until
            another one is the default.
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
            onClick={exportAll}
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
              <th aria-label="Actions" />
            </tr>
          </thead>
          <tbody>
            {list.map((setup) => (
              <tr key={`${setup.kind}:${setup.name}`}>
                <td>
                  {setup.name}
                  {isDefault(setup) && (
                    <span className="processing-setups-default">
                      Default for new color previews
                    </span>
                  )}
                </td>
                <td>{kindLabels[setup.kind]}</td>
                <td>
                  {/* The seeded setup has no time until someone saves over it. */}
                  {setup.updated_unix_seconds > 0
                    ? formatRelativeTime(setup.updated_unix_seconds)
                    : 'Not edited'}
                </td>
                <td className="processing-setups-row-actions">
                  {access.canWrite && setup.kind === 'color' && !isDefault(setup) && (
                    <button
                      type="button"
                      className="browse-button"
                      disabled={busy}
                      aria-label={`Make “${setup.name}” the default for new color previews`}
                      onClick={() => markDefault.mutate(setup.name)}
                    >
                      Make default
                    </button>
                  )}
                  <button
                    type="button"
                    className="browse-button"
                    disabled={busy}
                    onClick={() => exportOne(setup)}
                  >
                    Export
                  </button>
                  {access.canWrite && (
                    <button
                      type="button"
                      className="remove-button"
                      disabled={busy || isDefault(setup)}
                      title={isDefault(setup)
                        ? 'Make another Color setup the default before deleting this one'
                        : undefined}
                      onClick={() => remove.mutate(setup.name)}
                    >
                      Delete
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
