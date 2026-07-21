import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  StackColorJob,
  StackColorKind,
  StackColorTargetAvailability,
  StackNarrowbandPalette,
} from '../api/types';
import StackPreviewInspector from './StackPreviewInspector';

interface StackColorPreviewPanelProps {
  dbId: string;
  projectId: number;
  sourceRevision: number;
  channelBuildRunning: boolean;
  outdatedTargetIds: ReadonlySet<number>;
}

interface ColorOperation {
  targetId: number;
  kind: StackColorKind;
  palette?: StackNarrowbandPalette;
  force: boolean;
  operationKey: string;
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

function catalogQueryKey(dbId: string, projectId: number, sourceRevision: number) {
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

function ColorCard({
  dbId,
  target,
  kind,
  palette,
  paletteChoices,
  artifact,
  activeJob,
  busy,
  operationPending,
  unavailable,
  sourceStacksOutdated,
  onPaletteChange,
  onBuild,
  onInspect,
}: {
  dbId: string;
  target: StackColorTargetAvailability;
  kind: StackColorKind;
  palette?: StackNarrowbandPalette;
  paletteChoices: StackNarrowbandPalette[];
  artifact?: StackColorJob;
  activeJob?: StackColorJob;
  busy: boolean;
  operationPending: boolean;
  unavailable: boolean;
  sourceStacksOutdated: boolean;
  onPaletteChange?: (palette: StackNarrowbandPalette) => void;
  onBuild: () => void;
  onInspect: (job: StackColorJob) => void;
}) {
  const current = activeJob ?? artifact;
  const state = activeJob?.state ?? (artifact ? 'completed' : 'not-built');
  const label = kind === 'lrgb' ? 'LRGB' : palette ? paletteLabels[palette].split(' · ')[0] : 'Narrowband';
  const processed = activeJob?.processed_channels ?? artifact?.total_channels ?? 0;
  const total = activeJob?.total_channels ?? artifact?.total_channels ?? (kind === 'lrgb' ? 4 : 3);
  const percent = state === 'completed' ? 100 : total > 0 ? Math.min(100, processed / total * 100) : 0;
  const sourceFrames = artifact?.sources.reduce((sum, source) => sum + source.accepted_frames, 0) ?? 0;
  const stateLabel =
    state === 'queued' ? 'Waiting for color processor' :
      state === 'running' ? activeJob?.phase ?? 'Building color preview' :
        state === 'completed' ? 'Color preview ready' :
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
          ) : <span className="stack-preview-channel">LRGB</span>}
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
              aria-label={`Download ${label} RGB FITS`}
            >
              FITS
            </a>
          )}
          <button
            className="stack-preview-card-action"
            type="button"
            disabled={busy || unavailable}
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
            src={apiClient.getStackColorPreviewUrl(
              dbId, artifact.job_id, artifact.artifact_revision
            )}
            alt={`${target.target_name} ${label} color stack preview`}
          />
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

      <div
        className={`stack-preview-progress ${state}`}
        data-stack-color-state={state}
        role="status"
        aria-live="polite"
      >
        <div className="stack-preview-progress-copy">
          <span>{stateLabel}</span>
          <span>{processed}/{total} channels</span>
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
}: StackColorPreviewPanelProps) {
  const queryClient = useQueryClient();
  const [activeJobId, setActiveJobId] = useState<string | null>(null);
  const [paletteByTarget, setPaletteByTarget] = useState<Record<number, StackNarrowbandPalette>>({});
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
    }),
    onSuccess: (job) => {
      queryClient.setQueryData(jobQueryKey(dbId, projectId, job.job_id), job);
      setActiveJobId(job.job_id);
      if (terminalStates.has(job.state)) {
        queryClient.invalidateQueries({
          queryKey: catalogQueryKey(dbId, projectId, sourceRevision),
        });
      }
    },
  });

  const status = useQuery({
    queryKey: jobQueryKey(dbId, projectId, activeJobId),
    queryFn: () => apiClient.getStackColorJob(dbId, projectId, activeJobId!),
    enabled: activeJobId !== null,
    refetchInterval: (query) =>
      query.state.data && !terminalStates.has(query.state.data.state) ? 700 : false,
  });
  const activeJob = status.data;

  useEffect(() => {
    if (activeJob && terminalStates.has(activeJob.state)) {
      queryClient.invalidateQueries({
        queryKey: catalogQueryKey(dbId, projectId, sourceRevision),
      });
    }
  }, [activeJob, dbId, projectId, queryClient, sourceRevision]);

  useEffect(() => {
    setActiveJobId(null);
    setPaletteByTarget({});
    setInspector(null);
    resetStart();
  }, [dbId, projectId, resetStart]);

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
          lrgb_available: false,
          narrowband_palettes: [],
        });
      }
    }
    return [...byId.values()].filter((target) =>
      target.lrgb_available || target.narrowband_palettes.length > 0 ||
      (catalog.data?.jobs ?? []).some((job) => job.target_id === target.target_id)
    );
  }, [catalog.data]);

  if (targets.length === 0 && !catalog.error) return null;

  const colorBusy = startPending || activeJob?.state === 'queued' || activeJob?.state === 'running';
  const busy = channelBuildRunning || colorBusy;
  const error = startError ?? status.error ?? catalog.error;

  return (
    <section className="stack-color-section" aria-labelledby="stack-color-title">
      <div className="stack-color-heading">
        <div>
          <div className="stack-preview-eyebrow">Color quick looks</div>
          <h3 id="stack-color-title">Combine channel stacks</h3>
          <p>Register completed mono stacks across filters, then compose an LRGB or selectable narrowband preview.</p>
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
          const lrgbArtifact = activeJob?.state === 'completed' &&
            jobMatches(activeJob, target.target_id, 'lrgb')
            ? activeJob
            : targetJobs.find((job) => jobMatches(job, target.target_id, 'lrgb'));
          const lrgbActive = activeJob && jobMatches(activeJob, target.target_id, 'lrgb')
            ? activeJob : undefined;
          if (target.lrgb_available || lrgbArtifact) {
            const key = operationKey(target.target_id, 'lrgb');
            cards.push(
              <ColorCard
                key={key}
                dbId={dbId}
                target={target}
                kind="lrgb"
                paletteChoices={[]}
                artifact={lrgbArtifact}
                activeJob={lrgbActive}
                busy={busy}
                operationPending={startPending && startVariables?.operationKey === key}
                unavailable={!target.lrgb_available}
                sourceStacksOutdated={outdatedTargetIds.has(target.target_id)}
                onBuild={() => startColor({
                  targetId: target.target_id,
                  kind: 'lrgb',
                  force: Boolean(lrgbArtifact && !lrgbArtifact.outdated),
                  operationKey: key,
                })}
                onInspect={setInspector}
              />
            );
          }

          const paletteChoices = paletteOrder.filter((palette) =>
            target.narrowband_palettes.includes(palette) ||
            targetJobs.some((job) => job.kind === 'narrowband' && job.palette === palette)
          );
          const palette = paletteByTarget[target.target_id] ?? defaultPalette(paletteChoices);
          if (palette) {
            const artifact = activeJob?.state === 'completed' &&
              jobMatches(activeJob, target.target_id, 'narrowband', palette)
              ? activeJob
              : targetJobs.find((job) => jobMatches(job, target.target_id, 'narrowband', palette));
            const cardActive = activeJob &&
              jobMatches(activeJob, target.target_id, 'narrowband', palette)
              ? activeJob : undefined;
            const key = operationKey(target.target_id, 'narrowband', palette);
            cards.push(
              <ColorCard
                key={`${target.target_id}:narrowband`}
                dbId={dbId}
                target={target}
                kind="narrowband"
                palette={palette}
                paletteChoices={paletteChoices}
                artifact={artifact}
                activeJob={cardActive}
                busy={busy}
                operationPending={startPending && startVariables?.operationKey === key}
                unavailable={!target.narrowband_palettes.includes(palette)}
                sourceStacksOutdated={outdatedTargetIds.has(target.target_id)}
                onPaletteChange={(next) => setPaletteByTarget((current) => ({
                  ...current, [target.target_id]: next,
                }))}
                onBuild={() => startColor({
                  targetId: target.target_id,
                  kind: 'narrowband',
                  palette,
                  force: Boolean(artifact && !artifact.outdated),
                  operationKey: key,
                })}
                onInspect={setInspector}
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
          ]}
          imageUrl={apiClient.getStackColorPreviewUrl(
            dbId, inspector.job_id, inspector.artifact_revision, 'original'
          )}
          fitsUrl={apiClient.getStackColorFitsUrl(
            dbId, inspector.job_id, inspector.artifact_revision
          )}
          imageAlt={`Full-resolution ${inspector.label} color preview for ${inspector.target_name}`}
          downloadLabel={`Download ${inspector.label} RGB FITS`}
          onClose={() => setInspector(null)}
        />
      )}
    </section>
  );
}
