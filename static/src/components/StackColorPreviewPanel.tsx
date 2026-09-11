import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  StackColorCrop,
  StackColorJob,
  StackColorKind,
  StackColorProcessing,
  StackColorRole,
  StackColorInputSources,
  StackColorTargetAvailability,
  StackNarrowbandPalette,
} from '../api/types';
import StackPreviewInspector from './StackPreviewInspector';
import CalibrationMasterButton from './CalibrationMasterInspector';
import StackColorProcessingControls from './StackColorProcessingControls';
import {
  processingForColorBuild,
} from './stackColorProcessing';
import { isColorStackSkyOriented } from './stackOrientation';
import { cropLabels, cropOrder, describeCrop, offCenterChannels } from './stackColorCrop';
import { STACK_ACTIVITY_QUERY_KEY, useStackActivity } from '../hooks/useStackActivity';
import { buildColorExposureSets, colorSourceKey, colorSourcesLabel, completedColorArtifact, resolveColorSources, sameColorSourceFamily, type ColorExposureSet } from './stackColorSources';
import { withRetainedColorExposureSets } from './colorExposureCards';
import './StackColorSources.css';

interface StackColorPreviewPanelProps {
  dbId: string;
  projectId: number;
  sourceRevision: string;
  channelBuildRunning: boolean;
  outdatedSourceKeys: ReadonlySet<string>;
  canCompute: boolean;
  onOpenImage: (imageId: number) => void;
}

interface ColorOperation {
  dbId: string;
  projectId: number;
  targetId: number;
  kind: StackColorKind;
  palette?: StackNarrowbandPalette;
  force: boolean;
  operationKey: string;
  cardKey: string;
  crop: StackColorCrop;
  processing: StackColorProcessing;
  inputSources?: StackColorInputSources;
}

type ExposureCardSet = ColorExposureSet & { retained?: boolean };

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
  sourceControls,
  exposureSetKey,
  exposureLabel,
  custom,
  unavailableReason,
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
  sourceControls?: ReactNode;
  exposureSetKey?: string;
  exposureLabel?: string;
  custom?: boolean;
  unavailableReason?: string;
}) {
  const current = activeJob ?? artifact;
  const state = activeJob?.state ?? (artifact ? 'completed' : 'not-built');
  const baseLabel = kind === 'rgb'
    ? 'RGB'
    : kind === 'lrgb'
      ? 'LRGB'
      : palette
        ? paletteLabels[palette].split(' · ')[0]
        : 'Narrowband';
  const label = [baseLabel, exposureLabel, custom ? 'custom' : undefined].filter(Boolean).join(' ');
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
      data-source-family={artifact?.source_family_key ?? ''}
      data-exposure-set={exposureSetKey}
      data-custom-combination={custom ? 'true' : undefined}
    >
      <header>
        <div>
          <h3>{target.target_name}</h3>
          {kind === 'narrowband' ? (
            <label className="stack-color-palette">
              <span>Palette</span>
              <select
                aria-label={`${target.target_name} narrowband${exposureLabel ? ` ${exposureLabel}` : custom ? ' custom' : ''} palette`}
                value={palette}
                disabled={busy}
                onChange={(event) => onPaletteChange?.(event.target.value as StackNarrowbandPalette)}
              >
                {paletteChoices.map((choice) => (
                  <option key={choice} value={choice}>{paletteLabels[choice]}</option>
                ))}
              </select>
            </label>
          ) : <span className="stack-preview-channel">{baseLabel}</span>}
          {exposureLabel && <span className="stack-color-exposure">{exposureLabel}</span>}
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
              aria-label={kind === 'rgb' ? `Download ${label} FITS` : `Download ${label} RGB FITS`}
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
      {sourceControls}
      {unavailableReason && <div className="stack-color-unavailable">{unavailableReason}</div>}

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
            ? unavailableReason ? 'Not built' : 'The required channel stacks are not currently available.'
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
        rcAstro={artifact?.resolved_input_rc_astro ?? {}}
        disabled={!canCompute || busy || unavailable}
        onApply={onProcessingApply}
      />

      {current && (
        <div className="stack-color-sources" aria-label={`${label} source stacks`}>
          {current.sources.map((source) => (
            <span key={`${source.role}:${source.job_id}:${source.group_index}`}>
              <strong>{roleLabels[source.role]}</strong>
              {colorSourcesLabel([source])}
              <small>{source.accepted_frames} frames</small>
            </span>
          ))}
          {artifact && <span className="stack-color-source-total"><strong>{sourceFrames}</strong> integrated inputs</span>}
        </div>
      )}
      {artifact && (
        <div className="calibration-master-entry">
          <CalibrationMasterButton
            dbId={dbId}
            source={{ kind: 'color', jobId: artifact.job_id, artifactRevision: artifact.artifact_revision }}
            title={artifact.target_name}
            label={artifact.label}
          />
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
  outdatedSourceKeys,
  canCompute,
  onOpenImage,
}: StackColorPreviewPanelProps) {
  const queryClient = useQueryClient();
  const [watchedJobIds, setWatchedJobIds] = useState<string[]>([]);
  const [paletteByCard, setPaletteByCard] = useState<Record<string, StackNarrowbandPalette>>({});
  const [cropByCard, setCropByCard] = useState<Record<string, StackColorCrop>>({});
  const [sourcesByCard, setSourcesByCard] = useState<Record<string, Partial<Record<StackColorRole, string>>>>({});
  const [savedByCard, setSavedByCard] = useState<Record<string, string>>({});
  const [pendingCardKeys, setPendingCardKeys] = useState<ReadonlySet<string>>(new Set());
  const [inspector, setInspector] = useState<StackColorJob | null>(null);

  const catalog = useQuery({
    queryKey: catalogQueryKey(dbId, projectId, sourceRevision),
    queryFn: () => apiClient.getStackColorCatalog(dbId, projectId),
  });

  const {
    mutate: startColor,
    error: startError,
    reset: resetStart,
  } = useMutation({
    mutationFn: (operation: ColorOperation) => apiClient.startStackColor(operation.dbId, operation.projectId, {
      target_id: operation.targetId,
      kind: operation.kind,
      palette: operation.palette,
      force: operation.force,
      crop: operation.crop,
      processing: operation.processing,
      ...(operation.inputSources ? { input_sources: operation.inputSources } : {}),
    }),
    onMutate: (operation) => setPendingCardKeys((current) => new Set(current).add(operation.cardKey)),
    onSettled: (_data, _error, operation) => setPendingCardKeys((current) => {
      const next = new Set(current);
      next.delete(operation.cardKey);
      return next;
    }),
    onSuccess: (job, operation) => {
      queryClient.setQueryData(jobQueryKey(operation.dbId, operation.projectId, job.job_id), job);
      queryClient.invalidateQueries({ queryKey: STACK_ACTIVITY_QUERY_KEY });
      if (terminalStates.has(job.state)) {
        queryClient.invalidateQueries({
          queryKey: ['db', operation.dbId, 'project', operation.projectId, 'stack-color', 'catalog'],
        });
      }
      if (operation.dbId !== dbId || operation.projectId !== projectId) return;
      setSavedByCard((current) => {
        const next = { ...current };
        delete next[operation.cardKey];
        return next;
      });
      setWatchedJobIds((current) =>
        current.includes(job.job_id) ? current : [...current, job.job_id]
      );
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
    setPaletteByCard({});
    setCropByCard({});
    setSourcesByCard({});
    setSavedByCard({});
    setPendingCardKeys(new Set());
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
    const jobs = [...(catalog.data?.jobs ?? []), ...watchedJobs];
    const byId = new Map((catalog.data?.targets ?? []).map((target) => [target.target_id, target]));
    for (const job of jobs) {
      if (!byId.has(job.target_id)) {
        byId.set(job.target_id, {
          target_id: job.target_id,
          target_name: job.target_name,
          available_roles: [],
          source_candidates: [],
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
      target.source_candidates?.some((source) => source.exposure_group) ||
      jobs.some((job) => job.target_id === target.target_id)
    );
  }, [catalog.data, watchedJobs]);

  if (targets.length === 0 && !catalog.error) return null;

  // Builds queue on the server, so other cards stay available while one runs.
  // channelBuildRunning no longer locks color out; a queued color job simply
  // waits its turn behind the mono builds on the shared permit.
  void channelBuildRunning;
  const error = startError ?? statusError ?? catalog.error;

  const newestWatched = (
    targetId: number,
    kind: StackColorKind,
    palette?: StackNarrowbandPalette,
    matchesFamily: (job: StackColorJob) => boolean = () => true,
  ) => {
    const mine = watchedJobs.filter((job) => jobMatches(job, targetId, kind, palette) && matchesFamily(job));
    return mine[mine.length - 1];
  };

  const sourceSelection = (
    target: StackColorTargetAvailability,
    kind: StackColorKind,
    palette?: StackNarrowbandPalette,
    exposureSet?: ExposureCardSet,
  ) => {
    const key = dbId + ':' + projectId + ':' + operationKey(target.target_id, kind, palette)
      + (exposureSet ? `:exposure:${exposureSet.key}` : '');
    const roles = requiredRoles(kind, palette);
    const candidates = (target.source_candidates ?? []).filter((source) => !exposureSet
      || exposureSet.candidates.some((member) => sameColorSourceFamily(source, member)));
    const aware = target.source_candidates !== undefined;
    const resolved = resolveColorSources(candidates, roles, exposureSet ? {} : sourcesByCard[key] ?? {});
    const family = exposureSet ? resolveColorSources(exposureSet.candidates, roles, {}) : resolved;
    const saved = (catalog.data?.jobs ?? []).filter((job) => jobMatches(job, target.target_id, kind, palette));
    const selectedSaved = exposureSet ? undefined : saved.find((job) => job.job_id === savedByCard[key]);
    const matchesFamily = (job: StackColorJob) => (!exposureSet && (!aware
      || (candidates.length === 0 && (!selectedSaved || selectedSaved.job_id === job.job_id)))) || (family.complete
      && job.sources.length === roles.length
      && family.sources.every((source) => job.sources.some((previous) => sameColorSourceFamily(source, previous))));
    const showChoices = !exposureSet && (candidates.some((source) => source.exposure_group)
      || target.ambiguous_roles.some((role) => roles.includes(role))
      || (!resolved.complete && roles.some((role) => sourcesByCard[key]?.[role] !== undefined))
      || saved.length > 1);
    const resetCrop = () => setCropByCard((current) => {
      const next = { ...current };
      delete next[key];
      return next;
    });
    const controls = showChoices ? (
      <div className="stack-color-source-selectors">
        {saved.length > 0 && <label>
          <span>Saved preview</span>
          <select
            aria-label={`${target.target_name} ${palette ?? kind} saved preview`}
            value={selectedSaved?.job_id ?? ''}
            onChange={(event) => {
              resetCrop();
              const job = saved.find((entry) => entry.job_id === event.target.value);
              setSavedByCard((current) => ({ ...current, [key]: event.target.value }));
              if (job) setSourcesByCard((current) => ({
                ...current,
                [key]: Object.fromEntries(roles.map((role) => {
                  const previous = job.sources.find((source) => source.role === role);
                  const current = previous && candidates.find((source) => sameColorSourceFamily(source, previous));
                  return [role, current ? colorSourceKey(current) : ''];
                })),
              }));
            }}
          >
            <option value="">Current source selection</option>
            {saved.map((job) => <option key={job.job_id} value={job.job_id}>{colorSourcesLabel(job.sources)}</option>)}
          </select>
        </label>}
        {roles.map((role) => {
          const choices = candidates.filter((source) => source.role === role);
          const chosen = resolved.sources.find((source) => source.role === role);
          return <label key={role}>
            <span>{roleLabels[role]}</span>
            <select
              aria-label={`${target.target_name} ${palette ?? kind} ${roleLabels[role]} source stack`}
              value={chosen ? colorSourceKey(chosen) : ''}
              disabled={choices.length === 0}
              onChange={(event) => {
                resetCrop();
                setSourcesByCard((current) => ({ ...current, [key]: { ...current[key], [role]: event.target.value } }));
                setSavedByCard((current) => ({ ...current, [key]: '' }));
              }}
            >
              <option value="">{choices.length ? 'Choose stack' : 'Unavailable'}</option>
              {choices.map((source) => <option key={colorSourceKey(source)} value={colorSourceKey(source)}>
                {colorSourcesLabel([source])} · {source.accepted_frames} frames
              </option>)}
            </select>
          </label>;
        })}
      </div>
    ) : undefined;
    const missing = roles.filter((role) => !candidates.some((source) => source.role === role));
    const ambiguous = roles.filter((role) => candidates.filter((source) => source.role === role).length > 1);
    return {
      key,
      complete: (!aware || resolved.complete) && (!exposureSet || (exposureSet.known && !exposureSet.retained)),
      inputSources: (exposureSet || showChoices) && resolved.complete ? resolved.inputSources : undefined,
      matchesFamily,
      remembered: selectedSaved ?? saved.find(matchesFamily)
        ?? (!exposureSet && candidates.length === 0 ? saved[0] : undefined),
      controls,
      sources: resolved.sources,
      unavailableReason: !exposureSet ? undefined : exposureSet.retained ? 'Previous source set is no longer available'
        : !exposureSet.known ? 'Exposure duration unavailable'
        : [missing.length ? `Missing ${missing.map((role) => roleLabels[role]).join(', ')}` : '',
          ambiguous.length ? `Multiple ${ambiguous.map((role) => roleLabels[role]).join(', ')} stacks` : '']
          .filter(Boolean).join(' · ') || undefined,
    };
  };

  const renderCard = (
    target: StackColorTargetAvailability,
    kind: StackColorKind,
    available: boolean,
    exposureSet?: ExposureCardSet,
    palette?: StackNarrowbandPalette,
    paletteChoices: StackNarrowbandPalette[] = [],
    paletteStateKey?: string,
    custom = false,
  ) => {
    const sources = sourceSelection(target, kind, palette, exposureSet);
    const watched = newestWatched(target.target_id, kind, palette, sources.matchesFamily);
    const artifact = completedColorArtifact(watched, catalog.data?.jobs ?? [], sources.remembered);
    if (!available && !artifact && !watched && !exposureSet) return null;
    const key = sources.key;
    const crop = cropByCard[key] ?? artifact?.crop ?? 'none';
    const operationPending = pendingCardKeys.has(key);
    const cardBusy = operationPending || (watched !== undefined && !terminalStates.has(watched.state));
    const card = (
      <ColorCard
        key={key}
        dbId={dbId}
        target={target}
        kind={kind}
        palette={palette}
        paletteChoices={paletteChoices}
        crop={crop}
        artifact={artifact}
        activeJob={watched?.state === 'completed' ? undefined : watched}
        busy={cardBusy}
        operationPending={operationPending}
        unavailable={(!exposureSet && !available) || !sources.complete}
        unavailableReason={sources.unavailableReason}
        sourceControls={sources.controls}
        exposureSetKey={exposureSet?.key}
        exposureLabel={exposureSet ? (exposureSet.retained ? 'Previous ' : '') + exposureSet.label : undefined}
        custom={custom}
        sourceStacksOutdated={[...sources.sources, ...(artifact?.sources ?? [])]
          .some((source) => outdatedSourceKeys.has(colorSourceKey(source)))}
        canCompute={canCompute}
        onPaletteChange={paletteStateKey ? (next) => setPaletteByCard((current) => ({
          ...current, [paletteStateKey]: next,
        })) : undefined}
        onCropChange={(next) => setCropByCard((current) => ({ ...current, [key]: next }))}
        onBuild={() => startColor({
          dbId, projectId,
          targetId: target.target_id, kind, palette,
          force: Boolean(artifact && !artifact.outdated),
          operationKey: key, cardKey: key, crop,
          processing: processingForColorBuild(artifact, requiredRoles(kind, palette)),
          inputSources: sources.inputSources,
        })}
        onInspect={setInspector}
        onProcessingApply={(processing) => startColor({
          dbId, projectId,
          targetId: target.target_id, kind, palette, force: false,
          operationKey: key + ':processing', cardKey: key, crop, processing,
          inputSources: sources.inputSources,
        })}
      />
    );
    return custom ? (
      <details key={key + ':custom'} className="stack-color-custom"
        data-color-kind={kind} data-target-id={target.target_id}
        aria-label={target.target_name + ' ' + kind.toUpperCase() + ' custom combination'}>
        <summary><span>Custom combination</span><small>{target.target_name} · {kind.toUpperCase()}</small></summary>
        <div className="stack-color-grid">{card}</div>
      </details>
    ) : card;
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
          const jobs = [...targetJobs, ...watchedJobs.filter((job) => job.target_id === target.target_id)];
          const candidates = target.source_candidates ?? [];
          const exposureMode = candidates.length > 0
            ? candidates.some((source) => source.exposure_group)
            : jobs.some((job) => job.sources.some((source) => source.exposure_group));
          const cards: ReactNode[] = [];
          for (const kind of ['rgb', 'lrgb'] as const) {
            const available = kind === 'rgb' ? target.rgb_available : target.lrgb_available;
            // RGB-only targets do not need a permanently missing LRGB card.
            if (kind === 'lrgb' && !available
              && !candidates.some((source) => source.role === 'luminance')
              && !jobs.some((job) => job.kind === kind)) continue;
            const roles = requiredRoles(kind);
            const sets = exposureMode ? withRetainedColorExposureSets(
              buildColorExposureSets(candidates, roles),
              jobs.filter((job) => job.kind === kind), roles,
            ) : [];
            for (const set of sets) cards.push(renderCard(target, kind, available, set));
            cards.push(renderCard(target, kind, available, undefined, undefined, [], undefined,
              exposureMode && (sets.length > 0 || jobs.some((job) => job.kind === kind))));
          }

          const narrowbandRoles: StackColorRole[] = ['ha', 'oiii', 'sii'];
          const narrowbandJobs = jobs.filter((job) => job.kind === 'narrowband');
          const paletteChoices = paletteOrder.filter((palette) =>
            target.narrowband_palettes.includes(palette)
            || narrowbandJobs.some((job) => job.palette === palette)
          );
          if (exposureMode && paletteChoices.length === 0
            && candidates.some((source) => narrowbandRoles.includes(source.role))) {
            paletteChoices.push(candidates.some((source) => source.role === 'sii') ? 'sho' : 'hoo');
          }
          const sets = exposureMode ? withRetainedColorExposureSets(
            buildColorExposureSets(candidates, narrowbandRoles), narrowbandJobs, narrowbandRoles,
          ) : [];
          for (const set of sets) {
            const paletteKey = target.target_id + ':narrowband:' + set.key;
            const localPalettes = paletteChoices.filter((choice) =>
              resolveColorSources(set.candidates, requiredRoles('narrowband', choice), {}).complete);
            const rememberedPalette = [...narrowbandJobs]
              .sort((left, right) => Number(!terminalStates.has(right.state)) - Number(!terminalStates.has(left.state))
                || right.created_unix_seconds - left.created_unix_seconds)
              .find((job) => job.palette && job.sources.every((previous) => {
                const matching = set.candidates.filter((source) => source.role === previous.role);
                return matching.length === 1 && sameColorSourceFamily(matching[0], previous);
              }))?.palette;
            const palette = paletteByCard[paletteKey]
              ?? rememberedPalette
              ?? defaultPalette(localPalettes.length ? localPalettes : paletteChoices);
            if (palette) cards.push(renderCard(target, 'narrowband',
              target.narrowband_palettes.includes(palette), set, palette, paletteChoices, paletteKey));
          }
          const paletteKey = target.target_id + ':narrowband:custom';
          const palette = paletteByCard[paletteKey] ?? defaultPalette(paletteChoices);
          if (palette) cards.push(renderCard(target, 'narrowband',
            target.narrowband_palettes.includes(palette), undefined, palette, paletteChoices,
            paletteKey, exposureMode));
          return cards;
        })}
      </div>
      {inspector && (
        <StackPreviewInspector
          eyebrow="Full-resolution color preview"
          title={inspector.target_name}
          label={inspector.label}
          summary={[
            colorSourcesLabel(inspector.sources),
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
