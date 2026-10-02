import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { WorkerSettings } from '../api/types';

const QUERY_KEY = ['worker-settings'] as const;

type Share = 'interactive_ratio' | 'background_ratio';

const SHARES: Array<{ key: Share; label: string; help: string }> = [
  {
    key: 'interactive_ratio',
    label: 'Work you wait on',
    help: 'Stack builds you start, quality scans, and previews being looked at.',
  },
  {
    key: 'background_ratio',
    label: 'Background work',
    help: 'Automatic stack refreshes, preview pre-generation, and quality backfill.',
  },
];

function cores(ratio: number, logical: number): number {
  return Math.max(1, Math.round(ratio * logical));
}

/**
 * How much of the processor PSF Guard's work may take, as a share of the
 * logical cores. Saved on the server, over the config file's values, and used
 * by every pool sized from then on; work already running keeps its threads.
 * Memory can still lower the count.
 */
export default function WorkerShareSettings() {
  const queryClient = useQueryClient();
  const settings = useQuery({ queryKey: QUERY_KEY, queryFn: apiClient.getWorkerSettings });
  const [draft, setDraft] = useState<Record<Share, number> | null>(null);
  useEffect(() => {
    if (settings.data) {
      setDraft({
        interactive_ratio: settings.data.interactive_ratio,
        background_ratio: settings.data.background_ratio,
      });
    }
  }, [settings.data]);
  const save = useMutation({
    mutationFn: apiClient.updateWorkerSettings,
    onSuccess: (updated) => queryClient.setQueryData(QUERY_KEY, updated),
  });

  if (settings.isLoading || !draft) return null;
  if (settings.isError || !settings.data) {
    return (
      <div className="stack-method-settings">
        <h3>Processor use</h3>
        <p className="muted" role="alert">Could not load the processor shares.</p>
      </div>
    );
  }
  const current: WorkerSettings = settings.data;
  const defaultOf = (key: Share) =>
    key === 'interactive_ratio' ? current.default_interactive_ratio : current.default_background_ratio;
  const commit = (next: Record<Share, number>) =>
    save.mutate({
      interactive_ratio: next.interactive_ratio === current.default_interactive_ratio ? null : next.interactive_ratio,
      background_ratio: next.background_ratio === current.default_background_ratio ? null : next.background_ratio,
    });

  return (
    <div className="stack-method-settings worker-share-settings">
      <h3>Processor use</h3>
      <p className="muted">
        How many of this computer&apos;s {current.logical_cores} cores each kind of work may use.
        A change applies to work that starts after it. Memory can lower the count further.
      </p>
      <fieldset className="calibration-settings-group" disabled={save.isPending}>
        {SHARES.map((share) => {
          const ratio = draft[share.key];
          const isDefault = Math.abs(ratio - defaultOf(share.key)) < 1e-9;
          return (
            <div key={share.key} className="review-preference worker-share">
              <span>
                <span className="stack-method-label">
                  {share.label}
                  <em className="worker-share-value">
                    {Math.round(ratio * 100)}% · {cores(ratio, current.logical_cores)} of{' '}
                    {current.logical_cores} cores
                  </em>
                </span>
                <small>
                  {share.help} Default {Math.round(defaultOf(share.key) * 100)}%.
                  {!isDefault && (
                    <>
                      {' '}
                      <button
                        type="button"
                        className="link-button"
                        onClick={() => {
                          const next = { ...draft, [share.key]: defaultOf(share.key) };
                          setDraft(next);
                          commit(next);
                        }}
                      >
                        Use the default
                      </button>
                    </>
                  )}
                </small>
              </span>
              <input
                type="range"
                min={5}
                max={100}
                step={5}
                value={Math.round(ratio * 100)}
                aria-label={`${share.label} share of cores`}
                aria-valuetext={`${Math.round(ratio * 100)}%, ${cores(ratio, current.logical_cores)} cores`}
                onChange={(event) => setDraft({ ...draft, [share.key]: Number(event.target.value) / 100 })}
                onPointerUp={() => commit(draft)}
                onKeyUp={() => commit(draft)}
                onBlur={() => {
                  if (draft[share.key] !== current[share.key]) commit(draft);
                }}
              />
            </div>
          );
        })}
      </fieldset>
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
    </div>
  );
}
