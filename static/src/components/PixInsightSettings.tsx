import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { describePixInsight, formatFree } from '../utils/pixinsight';
import PathField from './PathField';
import UnsavedMark from './UnsavedMark';

/**
 * Where PixInsight is on the server, for stacking with WBPP from inside
 * PSF Guard. Server-wide, persisted in the registry. A run needs the
 * executable and a display; on a Linux server without one, xvfb-run gives
 * PixInsight a virtual screen.
 */
export default function PixInsightSettings() {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: ['pixinsight-settings'],
    queryFn: apiClient.getPixInsightSettings,
  });
  // A field without an edit (null) shows the server's value, so a refetch
  // updates it; an edited field keeps what was typed until Save.
  const [draft, setDraft] = useState<string | null>(null);
  const [runsDraft, setRunsDraft] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: (update: { binary: string | null; runs_dir: string | null }) =>
      apiClient.updatePixInsightSettings(update),
    onSuccess: (updated) => {
      queryClient.setQueryData(['pixinsight-settings'], updated);
      setDraft(null);
      setRunsDraft(null);
    },
  });

  if (settings.isLoading) return null;
  if (settings.isError) {
    return (
      <div className="pixinsight-settings">
        <h3>PixInsight</h3>
        <p className="muted">Could not load PixInsight settings.</p>
      </div>
    );
  }
  const current = settings.data!;
  const binary = draft ?? current.binary ?? '';
  const runsDir = runsDraft ?? current.runs_dir ?? '';
  const changed = binary !== (current.binary ?? '') || runsDir !== (current.runs_dir ?? '');
  const submit = () => save.mutate({ binary: binary.trim() || null, runs_dir: runsDir.trim() || null });

  return (
    <div className="pixinsight-settings">
      <UnsavedMark label="PixInsight settings" dirty={changed} />
      <h3>PixInsight</h3>
      <p className="review-preferences-note">
        Set the PixInsight executable and WBPP output folder to stack projects from the Library.
      </p>
      <p
        className={`pixinsight-status${current.ready ? ' is-ready' : ' is-missing'}`}
        role="status"
      >
        {describePixInsight(current)}
      </p>
      <fieldset className="calibration-settings-group calibration-matching-group">
        <legend>Install</legend>
        <label className="review-preference">
          <span>
            Executable
            <small>
              <code>PixInsight.sh</code> under <code>bin/</code> on Linux, the binary inside the
              app bundle on macOS, <code>PixInsight.exe</code> on Windows. Leave blank to search the standard installation folders.
            </small>
          </span>
          <PathField
            kind="file"
            dialogTitle="Select the PixInsight executable"
            aria-label="PixInsight executable"
            placeholder="/opt/PixInsight/bin/PixInsight.sh"
            value={binary}
            onChange={setDraft}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && changed) submit();
            }}
          />
        </label>
        <label className="review-preference">
          <span>
            WBPP runs folder
            <small>
              Save WBPP scripts and output here. Choose a disk with room for several gigabytes per run.
              Leave blank to use the database&apos;s export directory, or its stack folder if no export directory is set.
              {current.runs_dir && current.runs_dir_free_bytes != null && (
                <> {formatFree(current.runs_dir_free_bytes)} free there now.</>
              )}
            </small>
          </span>
          <PathField
            dialogTitle="Select the WBPP runs folder"
            aria-label="WBPP runs folder"
            placeholder="/data/wbpp-runs"
            value={runsDir}
            onChange={setRunsDraft}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && changed) submit();
            }}
          />
        </label>
      </fieldset>
      <div className="settings-actions">
        <button
          type="button"
          className="header-button"
          disabled={!changed || save.isPending}
          onClick={submit}
        >
          Save
        </button>
        <button
          type="button"
          className="header-button"
          disabled={save.isPending}
          onClick={() => settings.refetch()}
        >
          Check again
        </button>
      </div>
      {save.isError && <p className="error-text">{(save.error as Error).message}</p>}
    </div>
  );
}
