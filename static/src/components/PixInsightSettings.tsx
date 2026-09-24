import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { describePixInsight, formatFree } from '../utils/pixinsight';

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
  const [draft, setDraft] = useState<string | null>(null);
  const [runsDraft, setRunsDraft] = useState<string | null>(null);
  useEffect(() => {
    if (settings.data && draft === null) {
      setDraft(settings.data.binary ?? '');
      setRunsDraft(settings.data.runs_dir ?? '');
    }
  }, [settings.data, draft]);

  const save = useMutation({
    mutationFn: (update: { binary: string | null; runs_dir: string | null }) =>
      apiClient.updatePixInsightSettings(update),
    onSuccess: (updated) => {
      queryClient.setQueryData(['pixinsight-settings'], updated);
      setDraft(updated.binary ?? '');
      setRunsDraft(updated.runs_dir ?? '');
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
  const changed =
    (draft ?? '') !== (current.binary ?? '') || (runsDraft ?? '') !== (current.runs_dir ?? '');
  const submit = () =>
    save.mutate({ binary: draft?.trim() || null, runs_dir: runsDraft?.trim() || null });

  return (
    <div className="pixinsight-settings">
      <h3>PixInsight</h3>
      <p className="review-preferences-note">
        Where PixInsight is on this server, for stacking a project with WBPP from the Overview.
        Leave the path empty to look in the standard places.
      </p>
      <p
        className={`pixinsight-status${current.ready ? ' is-ready' : ' is-missing'}`}
        role="status"
      >
        {describePixInsight(current)}
      </p>
      <div className="pixinsight-binary">
        <input
          type="text"
          aria-label="PixInsight executable"
          placeholder="/opt/PixInsight/bin/PixInsight.sh"
          value={draft ?? ''}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && changed) submit();
          }}
        />
      </div>
      <p className="review-preferences-note">
        Where runs put their script and WBPP&apos;s output. A run writes gigabytes, so pick a
        disk with room; empty means the database&apos;s export directory when it has one, else
        the cache.
        {current.runs_dir && current.runs_dir_free_bytes != null && (
          <> {formatFree(current.runs_dir_free_bytes)} free there now.</>
        )}
      </p>
      <div className="pixinsight-binary">
        <input
          type="text"
          aria-label="WBPP runs folder"
          placeholder="/data/wbpp-runs"
          value={runsDraft ?? ''}
          onChange={(event) => setRunsDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && changed) submit();
          }}
        />
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
