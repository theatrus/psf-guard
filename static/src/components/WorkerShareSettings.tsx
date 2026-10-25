import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { WorkerSettings } from '../api/types';

const QUERY_KEY = ['worker-settings'] as const;

type Share = 'interactive_ratio' | 'background_ratio';

const SHARES: Array<{ key: Share; label: string; help: string }> = [
  {
    key: 'interactive_ratio',
    label: 'Interactive work',
    help: 'Stack builds, color processing, stretches and quality scans you start, plus previews you are viewing.',
  },
  {
    key: 'background_ratio',
    label: 'Background work',
    help: 'Automatic stack refreshes, quality analysis and preview generation across all databases. Background jobs pause while jobs you start run.',
  },
];

function cores(ratio: number, logical: number): number {
  return Math.max(1, Math.round(ratio * logical));
}

/**
 * How much of the processor PSF Guard's work may take, as a share of the
 * logical cores. Each share limits all such work together: jobs running at
 * once split it. Saved on the server, over the config file's values, and used
 * by every job that starts from then on; work already running keeps its
 * threads. Memory can still lower the count.
 */
export default function WorkerShareSettings() {
  const queryClient = useQueryClient();
  const settings = useQuery({ queryKey: QUERY_KEY, queryFn: apiClient.getWorkerSettings });
  // The shares as moved. A slider not moved shows the saved share, so a
  // refetch updates it.
  const [draft, setDraft] = useState<Partial<Record<Share | 'memory_budget_fraction', number>>>({});
  const save = useMutation({
    mutationFn: apiClient.updateWorkerSettings,
    onSuccess: (updated) => {
      queryClient.setQueryData(QUERY_KEY, updated);
      setDraft({});
    },
  });

  if (settings.isLoading) return null;
  if (settings.isError || !settings.data) {
    return (
      <div className="stack-method-settings">
        <h3>Processor use</h3>
        <p className="muted" role="alert">Could not load processor limits.</p>
      </div>
    );
  }
  const current: WorkerSettings = settings.data;
  const shares: Record<Share, number> = {
    interactive_ratio: draft.interactive_ratio ?? current.interactive_ratio,
    background_ratio: draft.background_ratio ?? current.background_ratio,
  };
  const memory = draft.memory_budget_fraction ?? current.memory_budget_fraction ?? 0.5;
  const defaultMemory = current.default_memory_budget_fraction ?? 0.5;
  const knownMemory = current.available_memory_bytes;
  const gib = (bytes: number) => (bytes / 1024 ** 3).toFixed(1);
  const commitMemory = (fraction: number) => save.mutate({
    interactive_ratio: Math.abs(current.interactive_ratio-current.default_interactive_ratio)<1e-9 ? null : current.interactive_ratio,
    background_ratio: Math.abs(current.background_ratio-current.default_background_ratio)<1e-9 ? null : current.background_ratio,
    memory_budget_fraction: Math.abs(fraction-defaultMemory)<1e-9 ? null : fraction,
  });
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
        Set how many of this computer&apos;s {current.logical_cores} cores each type of work can use.
        Concurrent jobs share that limit. Changes apply to new jobs. Available memory may
        reduce the number of cores a job can use.
      </p>
      <fieldset className="calibration-settings-group" disabled={save.isPending}>
        {SHARES.map((share) => {
          const ratio = shares[share.key];
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
                          setDraft({ ...draft, [share.key]: defaultOf(share.key) });
                          commit({ ...shares, [share.key]: defaultOf(share.key) });
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
                onPointerUp={() => commit(shares)}
                onKeyUp={() => commit(shares)}
                onBlur={() => {
                  if (shares[share.key] !== current[share.key]) commit(shares);
                }}
              />
            </div>
          );
        })}
      </fieldset>
      <h3>Processing memory</h3>
      <p className="muted">Sets the estimated memory limit for stack builds, color compositions and stretches.
        A higher budget lets larger jobs start and leaves less room for other work. Changes apply to new jobs.</p>
      <fieldset className="calibration-settings-group" aria-label="Processing memory" disabled={save.isPending}>
        <div className="review-preference worker-share">
          <span>
            <span className="stack-method-label">Memory budget <em className="worker-share-value">
              {Math.round(memory*100)}%{knownMemory != null && ` · ${gib(knownMemory*memory)} of ${gib(knownMemory)} GiB ${current.memory_basis ?? 'available'} memory`}
            </em></span>
            <small>Default {Math.round(defaultMemory*100)}%. Release the slider or press Enter to save; moving focus also saves.
              {knownMemory == null && ' The server could not measure memory; the limit applies when a measurement is available.'}
              {Math.abs(memory-defaultMemory)>1e-9 && <>{' '}<button type="button" className="link-button"
                onClick={() => { setDraft({ ...draft, memory_budget_fraction: defaultMemory }); commitMemory(defaultMemory); }}>
                Use the default memory budget</button></>}
            </small>
          </span>
          <input type="range" min={5} max={100} step={5} value={Math.round(memory*100)}
            aria-label="Processing memory budget" aria-valuetext={`${Math.round(memory*100)}%`}
            onChange={(event) => setDraft({ ...draft, memory_budget_fraction: Number(event.target.value)/100 })}
            onPointerUp={(event) => commitMemory(Number(event.currentTarget.value)/100)}
            onKeyUp={(event) => { if(event.key === 'Enter') commitMemory(Number(event.currentTarget.value)/100); }}
            onBlur={(event) => { const value=Number(event.currentTarget.value)/100; if(!save.isPending && value !== (current.memory_budget_fraction ?? 0.5)) commitMemory(value); }} />
        </div>
      </fieldset>
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
    </div>
  );
}
