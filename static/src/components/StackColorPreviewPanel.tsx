import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  StackColorCrop,
  StackColorJob,
  StackColorKind,
  StackColorProcessing,
  StackColorRole,
  StackColorTargetAvailability,
  StackNarrowbandPalette,
} from '../api/types';
import StackPreviewInspector from './StackPreviewInspector';
import StackColorProcessingControls from './StackColorProcessingControls';
import {
  processingForColorBuild,
} from './stackColorProcessing';
import { isColorStackSkyOriented } from './stackOrientation';
import { cropLabels, cropOrder, describeCrop, offCenterChannels } from './stackColorCrop';
import { STACK_ACTIVITY_QUERY_KEY, useStackActivity } from '../hooks/useStackActivity';

interface StackColorPreviewPanelProps {
  dbId: string;
  projectId: number;
  sourceRevision: string;
  channelBuildRunning: boolean;
  outdatedTargetIds: ReadonlySet<number>;
  canCompute: boolean;
  onOpenImage: (imageId: number) => void;
}

interface ColorOperation {
  targetId: number;
  kind: StackColorKind;
  palette?: StackNarrowbandPalette;
  force: boolean;
  operationKey: string;
  crop: StackColorCrop;
  processing: StackColorProcessing;
}

const terminalStates = new Set(['completed', 'failed']);
const paletteOrder: StackNarrowbandPalette[] = [
  'sho', 'soh', 'hso', 'hos', 'osh', 'ohs', 'hoo', 'foraxx-sho', 'foraxx-hoo',
];

const paletteLabels: Record<StackNarrowbandPalette, string> = {
  sho: 'SHO · SII / Hα / OIII',
  soh: 'SOH · SII / OIII / Hα',
  hso: 'HSO · Hα / SII / OIII',
  hos: 'HOS · Hα / OIII / SII',
  osh: 'OSH · OIII / SII / Hα',
  ohs: 'OHS · OIII / Hα / SII',
  hoo: 'HOO · Hα / OIII / OIII',
  'foraxx-sho': 'Foraxx SHO',
  'foraxx-hoo': 'Foraxx HOO',
};

const roleLabels: Record<string, string> = {
  luminance: 'L', red: 'R', green: 'G', blue: 'B', ha: 'Hα', oiii: 'OIII', sii: 'SII',
};

function catalogQueryKey(dbId: string, projectId: number, sourceRevision: string) {
  return ['db', dbId, 'project', projectId, 'stack-color', 'catalog', sourceRevision] as const;
}

function jobQueryKey(dbId: string, projectId: number, jobId: string | null) {
  return ['db', dbId, 'project', projectId, 'stack-color', jobId] as const;
}

function operationKey(
  targetId: number,
  kind: StackColorKind,
  palette?: StackNarrowbandPalette
) {
  return `${targetId}:${kind}:${palette ?? ''}`;
}

function jobMatches(
  job: StackColorJob,
  targetId: number,
  kind: StackColorKind,
  palette?: StackNarrowbandPalette
) {
  return job.target_id === targetId && job.kind === kind && job.palette === (palette ?? null);
}

function defaultPalette(palettes: StackNarrowbandPalette[]): StackNarrowbandPalette | undefined {
  if (palettes.includes('sho')) return 'sho';
  if (palettes.includes('hoo')) return 'hoo';
  return palettes[0];
}

function expectedChannelCount(kind: StackColorKind, palette?: StackNarrowbandPalette): number {
  if (kind === 'rgb') return 3;
  if (kind === 'lrgb') return 4;
  return palette === 'hoo' || palette === 'foraxx-hoo' ? 2 : 3;
}

function requiredRoles(
  kind: StackColorKind,
  palette?: StackNarrowbandPalette
): StackColorRole[] {
  if (kind === 'rgb') return ['red', 'green', 'blue'];
  if (kind === 'lrgb') return ['luminance', 'red', 'green', 'blue'];
  return palette === 'hoo' || palette === 'foraxx-hoo'
    ? ['ha', 'oiii']
    : ['ha', 'oiii', 'sii'];
}

function ColorCard({
  dbId,
  target,
  kind,
  palette,
  paletteChoices,
  crop,
  artifact,
  activeJob,
  busy,
  operationPending,
  unavailable,
  sourceStacksOutdated,
  canCompute,
  onPaletteChange,
  onCropChange,
  onBuild,
  onInspect,
  onProcessingApply,
}: {
  dbId: string;
  target: StackColorTargetAvailability;
  kind: StackColorKind;
  palette?: StackNarrowbandPalette;
  paletteChoices: StackNarrowbandPalette[];
  crop: StackColorCrop;
  artifact?: StackColorJob;
  activeJob?: StackColorJob;
  busy: boolean;
  operationPending: boolean;
  unavailable: boolean;
  sourceStacksOutdated: boolean;
  canCompute: boolean;
  onPaletteChange?: (palette: StackNarrowbandPalette) => void;
  onCropChange: (crop: StackColorCrop) => void;
  onBuild: () => void;
  onInspect: (job: StackColorJob) => void;
  onProcessingApply: (processing: StackColorProcessing) => void;
}) {
  const current = activeJob ?? artifact;
  const state = activeJob?.state ?? (artifact ? 'completed' : 'not-built');
  const label = kind === 'rgb'
    ? 'RGB'
    : kind === 'lrgb'
      ? 'LRGB'
      : palette
        ? paletteLabels[palette].split(' · ')[0]
        : 'Narrowband';
  const detailedProgress = activeJob?.progress ?? artifact?.progress;
  const processed = detailedProgress?.total_units
    ? detailedProgress.completed_units
    : activeJob?.processed_channels ?? artifact?.total_channels ?? 0;
  const total = detailedProgress?.total_units
    ? detailedProgress.total_units
    : activeJob?.total_channels ?? artifact?.total_channels ?? expectedChannelCount(kind, palette);
  const percent = state === 'completed' ? 100 : total > 0 ? Math.min(100, processed / total * 100) : 0;
  const sourceFrames = artifact?.sources.reduce((sum, source) => sum + source.accepted_frames, 0) ?? 0;
  const stateLabel =
    state === 'queued' ? 'Waiting for color processor' :
      state === 'running' ? activeJob?.phase ?? 'Building color preview' :
        state === 'completed' ? current?.phase ?? 'Color preview ready' :
          state === 'failed' ? 'Color preview failed' : 'Not built';

  return (
    <article
      className={`stack-color-card ${artifact?.outdated || sourceStacksOutdated ? 'outdated' : ''}`}
      data-color-kind={kind}
      data-target-id={target.target_id}
    >
      <header>
        <div>
          <h3>{target.target_name}</h3>
          {kind === 'narrowband' ? (
            <label className="stack-color-palette">
              <span>Palette</span>
              <select
                aria-label={`${target.target_name} narrowband palette`}
                value={palette}
                disabled={busy}
                onChange={(event) => onPaletteChange?.(event.target.value as StackNarrowbandPalette)}
              >
                {paletteChoices.map((choice) => (
                  <option key={choice} value={choice}>{paletteLabels[choice]}</option>
                ))}
              </select>
            </label>
          ) : <span className="stack-preview-channel">{label}</span>}
          <label className="stack-color-crop">
            <span>Edges</span>
            <select
              aria-label={`${target.target_name} ${label} edge crop`}
              value={crop}
              disabled={busy}
              onChange={(event) => onCropChange(event.target.value as StackColorCrop)}
            >
              {cropOrder.map((choice) => (
                <option key={choice} value={choice}>{cropLabels[choice]}</option>
              ))}
            </select>
          </label>
        </div>
        <div className="stack-preview-card-actions">
          <span className={`stack-group-state ${state}`}>{state.replace('-', ' ')}</span>
          {artifact && (
            <button
              className="stack-preview-card-action"
              type="button"
              aria-label={`Inspect ${label} full size`}
              onClick={() => onInspect(artifact)}
            >
              Inspect
            </button>
          )}
          {artifact && (
            <a
              className="stack-preview-card-action"
              href={apiClient.getStackColorFitsUrl(dbId, artifact.job_id, artifact.artifact_revision)}
              download
              aria-label={label === 'RGB' ? 'Download RGB FITS' : `Download ${label} RGB FITS`}
            >
              FITS
            </a>
          )}
          <button
            className="stack-preview-card-action"
            type="button"
            disabled={!canCompute || busy || unavailable}
            title={canCompute
              ? undefined
              : 'This account can view cached color previews but cannot build them.'}
            aria-label={artifact ? `Rebuild ${label} color preview` : `Build ${label} color preview`}
            onClick={onBuild}
          >
            {operationPending ? 'Building…' : artifact ? 'Rebuild' : 'Build'}
          </button>
        </div>
      </header>

      {(artifact?.outdated || sourceStacksOutdated) && (
        <div className="stack-preview-outdated">
          Out of date — {artifact?.outdated
            ? artifact.outdated_reason ?? 'source stacks changed'
            : 'one or more channel stacks need rebuilding for the current inputs'}
        </div>
      )}

      {artifact ? (
        <div className="stack-preview-image stack-color-image">
          <img
            src={apiClient.getStackColorPreviewUrl(dbId, artifact.job_id, artifact.artifact_revision)}
            alt={`${target.target_name} ${label} color stack preview`}
          />
          {isColorStackSkyOriented(artifact) && (
            <span className="stack-preview-orientation">N ↑ · E ←</span>
          )}
        </div>
      ) : activeJob && !terminalStates.has(activeJob.state) ? (
        <div className="stack-preview-placeholder">
          <span className="stack-preview-spinner" aria-hidden="true" />
          {activeJob.phase}
        </div>
      ) : (
        <div className={`stack-preview-placeholder ${activeJob?.state === 'failed' ? 'error' : ''}`}>
          {activeJob?.error ?? (unavailable
            ? 'The required channel stacks are not currently available.'
            : `Build an on-demand ${label} quick look from the channel stacks.`)}
        </div>
      )}

      {artifact && describeCrop(artifact) && (
        <div className="stack-color-crop-summary">
          <span>{describeCrop(artifact)}</span>
          {offCenterChannels(artifact).map((channel) => (
            <strong key={channel.name} role="alert">
              {channel.role ? roleLabels[channel.role] : channel.name} sits
              {' '}{Math.round(channel.center_offset_pixels)}px off center from the other
              channels and bounds the crop
            </strong>
          ))}
        </div>
      )}

      <div
        className={`stack-preview-progress ${state}`}
        data-stack-color-state={state}
        role="status"
        aria-live="polite"
      >
        <div className="stack-preview-progress-copy">
          <span>{stateLabel}</span>
          <span>{processed}/{total} steps</span>
        </div>
        <div
          className="stack-preview-progress-track"
          role="progressbar"
          aria-label={`${target.target_name} ${label} color progress`}
          aria-valuemin={0}
          aria-valuemax={total}
          aria-valuenow={processed}
        >
          <span style={{ width: `${percent}%` }} />
        </div>
      </div>

      {!!detailedProgress?.phases.length && (
        <details className="stack-color-phase-details">
          <summary>Pipeline phases</summary>
          <ol>
            {detailedProgress.phases.map((phase) => (
              <li
                key={phase.phase}
                data-phase={phase.phase}
                data-phase-state={phase.state}
              >
                <span>{phase.label}</span>
                <small>{phase.state === 'skipped' || phase.state === 'reused'
                  ? phase.state
                  : `${phase.completed_units}/${phase.total_units}`}</small>
              </li>
            ))}
          </ol>
        </details>
      )}

      <StackColorProcessingControls
        key={`${artifact?.job_id ?? 'new'}:${artifact?.artifact_revision ?? label}`}
        label={`${target.target_name} ${label}`}
        roles={requiredRoles(kind, palette)}
        applied={artifact?.processing ?? null}
        backgrounds={artifact?.resolved_backgrounds ?? {}}
        protections={artifact?.resolved_background_protection ?? {}}
        fallbacks={artifact?.background_protection_fallbacks ?? {}}
        deconvolutions={artifact?.resolved_input_deconvolutions ?? {}}
        disabled={!canCompute || busy || unavailable}
        onApply={onProcessingApply}
      />

      {current && (
        <div className="stack-color-sources" aria-label={`${label} source stacks`}>
          {current.sources.map((source) => (
            <span key={`${source.role}:${source.job_id}:${source.group_index}`}>
              <strong>{roleLabels[source.role]}</strong>
              {source.filter_name}
              <small>{source.accepted_frames} frames</small>
            </span>
          ))}
          {artifact && <span className="stack-color-source-total"><strong>{sourceFrames}</strong> integrated inputs</span>}
        </div>
      )}
    </article>
  );
}

export default function StackColorPreviewPanel({
  dbId,
  projectId,
  sourceRevision,
  channelBuildRunning,
  outdatedTargetIds,
  canCompute,
  onOpenImage,
}: StackColorPreviewPanelProps) {
  const queryClient = useQueryClient();
  const [watchedJobIds, setWatchedJobIds] = useState<string[]>([]);
  const [paletteByTarget, setPaletteByTarget] = useState<Record<number, StackNarrowbandPalette>>({});
  const [cropByCard, setCropByCard] = useState<Record<string, StackColorCrop>>({});
  const [inspector, setInspector] = useState<StackColorJob | null>(null);

  const catalog = useQuery({
    queryKey: catalogQueryKey(dbId, projectId, sourceRevision),
    queryFn: () => apiClient.getStackColorCatalog(dbId, projectId),
  });

  const {
    mutate: startColor,
    isPending: startPending,
    variables: startVariables,
    error: startError,
    reset: resetStart,
  } = useMutation({
    mutationFn: (operation: ColorOperation) => apiClient.startStackColor(dbId, projectId, {
      target_id: operation.targetId,
      kind: operation.kind,
      palette: operation.palette,
      force: operation.force,
      crop: operation.crop,
      processing: operation.processing,
    }),
    onSuccess: (job) => {
      queryClient.setQueryData(jobQueryKey(dbId, projectId, job.job_id), job);
      setWatchedJobIds((current) =>
        current.includes(job.job_id) ? current : [...current, job.job_id]
      );
      queryClient.invalidateQueries({ queryKey: STACK_ACTIVITY_QUERY_KEY });
      if (terminalStates.has(job.state)) {
        queryClient.invalidateQueries({
          queryKey: catalogQueryKey(dbId, projectId, sourceRevision),
        });
      }
    },
  });

  const statuses = useQueries({
    queries: watchedJobIds.map((jobId) => ({
      queryKey: jobQueryKey(dbId, projectId, jobId),
      queryFn: () => apiClient.getStackColorJob(dbId, projectId, jobId),
      refetchInterval: (query: { state: { data?: StackColorJob } }) =>
        query.state.data && !terminalStates.has(query.state.data.state) ? 700 : false,
    })),
  });
  const watchedJobs = useMemo(
    () =>
      statuses
        .map((status) => status.data)
        .filter((job): job is StackColorJob => job !== undefined)
        .sort((left, right) => left.created_unix_seconds - right.created_unix_seconds),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- useQueries returns a new array each render; the join tracks content.
    [statuses.map((status) => status.dataUpdatedAt).join('|')]
  );
  const statusError = statuses.find((status) => status.error)?.error;
  const settledCount = watchedJobs.filter((job) => terminalStates.has(job.state)).length;
  useEffect(() => {
    if (settledCount > 0) {
      queryClient.invalidateQueries({
        queryKey: catalogQueryKey(dbId, projectId, sourceRevision),
      });
    }
  }, [settledCount, dbId, projectId, queryClient, sourceRevision]);

  useEffect(() => {
    setWatchedJobIds([]);
    setPaletteByTarget({});
    setCropByCard({});
    setInspector(null);
    resetStart();
  }, [dbId, projectId, resetStart]);

  // Re-attach to color builds still running on the server, so navigating away
  // and back does not hide them. Declared after the reset above so a project
  // change adopts that project's jobs.
  const { active } = useStackActivity();
  const adoptableIds = useMemo(
    () =>
      active
        .filter(
          (entry) =>
            entry.kind === 'color' && entry.database_id === dbId && entry.project_id === projectId
        )
        .map((entry) => entry.job_id),
    [active, dbId, projectId]
  );
  useEffect(() => {
    if (adoptableIds.length === 0) return;
    setWatchedJobIds((current) => {
      const additions = adoptableIds.filter((jobId) => !current.includes(jobId));
      return additions.length === 0 ? current : [...current, ...additions];
    });
  }, [adoptableIds]);

  const targets = useMemo(() => {
    const byId = new Map((catalog.data?.targets ?? []).map((target) => [target.target_id, target]));
    for (const job of catalog.data?.jobs ?? []) {
      if (!byId.has(job.target_id)) {
        byId.set(job.target_id, {
          target_id: job.target_id,
          target_name: job.target_name,
          available_roles: [],
          ambiguous_roles: [],
          unmapped_filters: [],
          rgb_available: false,
          lrgb_available: false,
          narrowband_palettes: [],
        });
      }
    }
    return [...byId.values()].filter((target) =>
      target.rgb_available || target.lrgb_available || target.narrowband_palettes.length > 0 ||
      (catalog.data?.jobs ?? []).some((job) => job.target_id === target.target_id)
    );
  }, [catalog.data]);

  if (targets.length === 0 && !catalog.error) return null;

  // Builds queue on the server, so other cards stay available while one runs.
  // channelBuildRunning no longer locks color out; a queued color job simply
  // waits its turn behind the mono builds on the shared permit.
  void channelBuildRunning;
  const error = startError ?? statusError ?? catalog.error;

  const newestWatched = (
    targetId: number,
    kind: StackColorKind,
    palette?: StackNarrowbandPalette
  ) => {
    const mine = watchedJobs.filter((job) => jobMatches(job, targetId, kind, palette));
    return mine[mine.length - 1];
  };

  return (
    <section className="stack-color-section" aria-labelledby="stack-color-title">
      <div className="stack-color-heading">
        <div>
          <div className="stack-preview-eyebrow">Color quick looks</div>
          <h3 id="stack-color-title">Combine channel stacks</h3>
          <p>Register completed mono stacks across filters, then compose an RGB, LRGB, or selectable narrowband preview.</p>
        </div>
        <span>On demand · cached by source revision</span>
      </div>
      {error && (
        <div className="stack-preview-message error" role="alert">
          {error instanceof Error ? error.message : 'Color preview failed'}
        </div>
      )}
      <div className="stack-color-grid">
        {targets.flatMap((target) => {
          const targetJobs = (catalog.data?.jobs ?? []).filter((job) => job.target_id === target.target_id);
          const cards = [];
          const broadbandKinds: Array<{ kind: 'rgb' | 'lrgb'; available: boolean }> = [
            { kind: 'rgb', available: target.rgb_available },
            { kind: 'lrgb', available: target.lrgb_available },
          ];
          for (const { kind, available } of broadbandKinds) {
            const watched = newestWatched(target.target_id, kind);
            const artifact = watched?.state === 'completed'
              ? watched
              : targetJobs.find((job) => jobMatches(job, target.target_id, kind));
            const cardActive = watched;
            if (available || artifact) {
              const key = operationKey(target.target_id, kind);
              const crop = cropByCard[key] ?? artifact?.crop ?? 'none';
              const operationPending = startPending &&
                (startVariables?.operationKey === key ||
                  startVariables?.operationKey === `${key}:processing`);
              const cardBusy = operationPending ||
                (watched !== undefined && !terminalStates.has(watched.state));
              cards.push(
                <ColorCard
                  key={key}
                  dbId={dbId}
                  target={target}
                  kind={kind}
                  paletteChoices={[]}
                  crop={crop}
                  artifact={artifact}
                  activeJob={cardActive}
                  busy={cardBusy}
                  operationPending={operationPending}
                  unavailable={!available}
                  sourceStacksOutdated={outdatedTargetIds.has(target.target_id)}
                  canCompute={canCompute}
                  onCropChange={(next) => setCropByCard((current) => ({
                    ...current, [key]: next,
                  }))}
                  onBuild={() => startColor({
                    targetId: target.target_id,
                    kind,
                    force: Boolean(artifact && !artifact.outdated),
                    operationKey: key,
                    crop,
                    processing: processingForColorBuild(artifact, requiredRoles(kind)),
                  })}
                  onInspect={setInspector}
                  onProcessingApply={(processing) => startColor({
                    targetId: target.target_id,
                    kind,
                    force: false,
                    operationKey: `${key}:processing`,
                    crop,
                    processing,
                  })}
                />
              );
            }
          }

          const paletteChoices = paletteOrder.filter((palette) =>
            target.narrowband_palettes.includes(palette) ||
            targetJobs.some((job) => job.kind === 'narrowband' && job.palette === palette)
          );
          const palette = paletteByTarget[target.target_id] ?? defaultPalette(paletteChoices);
          if (palette) {
            const watched = newestWatched(target.target_id, 'narrowband', palette);
            const artifact = watched?.state === 'completed'
              ? watched
              : targetJobs.find((job) => jobMatches(job, target.target_id, 'narrowband', palette));
            const cardActive = watched;
            const key = operationKey(target.target_id, 'narrowband', palette);
            const crop = cropByCard[key] ?? artifact?.crop ?? 'none';
            const operationPending = startPending &&
              (startVariables?.operationKey === key ||
                startVariables?.operationKey === `${key}:processing`);
            const cardBusy = operationPending ||
              (watched !== undefined && !terminalStates.has(watched.state));
            cards.push(
              <ColorCard
                key={`${target.target_id}:narrowband`}
                dbId={dbId}
                target={target}
                kind="narrowband"
                palette={palette}
                paletteChoices={paletteChoices}
                crop={crop}
                artifact={artifact}
                activeJob={cardActive}
                busy={cardBusy}
                operationPending={operationPending}
                unavailable={!target.narrowband_palettes.includes(palette)}
                sourceStacksOutdated={outdatedTargetIds.has(target.target_id)}
                canCompute={canCompute}
                onPaletteChange={(next) => setPaletteByTarget((current) => ({
                  ...current, [target.target_id]: next,
                }))}
                onCropChange={(next) => setCropByCard((current) => ({
                  ...current, [key]: next,
                }))}
                onBuild={() => startColor({
                  targetId: target.target_id,
                  kind: 'narrowband',
                  palette,
                  force: Boolean(artifact && !artifact.outdated),
                  operationKey: key,
                  crop,
                  processing: processingForColorBuild(
                    artifact, requiredRoles('narrowband', palette)
                  ),
                })}
                onInspect={setInspector}
                onProcessingApply={(processing) => startColor({
                  targetId: target.target_id,
                  kind: 'narrowband',
                  palette,
                  force: false,
                  operationKey: `${key}:processing`,
                  crop,
                  processing,
                })}
              />
            );
          }
          return cards;
        })}
      </div>
      {inspector && (
        <StackPreviewInspector
          eyebrow="Full-resolution color preview"
          title={inspector.target_name}
          label={inspector.label}
          summary={[
            `${inspector.sources.length} channel stacks`,
            `${inspector.sources.reduce((sum, source) => sum + source.accepted_frames, 0)} integrated inputs`,
            ...(isColorStackSkyOriented(inspector) ? ['North up · East left'] : []),
          ]}
          imageUrl={apiClient.getStackColorPreviewUrl(
            dbId, inspector.job_id, inspector.artifact_revision, 'original'
          )}
          fitsUrl={apiClient.getStackColorFitsUrl(
            dbId, inspector.job_id, inspector.artifact_revision
          )}
          imageAlt={`Full-resolution ${inspector.label} color preview for ${inspector.target_name}`}
          downloadLabel={inspector.label === 'RGB'
            ? 'Download RGB FITS'
            : `Download ${inspector.label} RGB FITS`}
          artifactSource={{
            kind: 'color',
            dbId,
            jobId: inspector.job_id,
            artifactRevision: inspector.artifact_revision,
          }}
          artifactEnabled={canCompute}
          onOpenImage={onOpenImage}
          onClose={() => setInspector(null)}
        />
      )}
    </section>
  );
}
