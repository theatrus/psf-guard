import { isValidElement, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { formatIntegration, totalIntegration } from '../utils/integrationTime';
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { httpStatus } from '../utils/httpStatus';
import type {
  StackColorSource,
  StackColorCrop,
  StackColorJob,
  StackColorKind,
  StackColorCombination,
  StackColorStars,
  StackColorProcessing,
  StackColorRole,
  StackColorInputSources,
  StackColorTargetAvailability,
  StackNarrowbandPalette,
} from '../api/types';
import StackPreviewInspector, { type FitsDownload } from './StackPreviewInspector';
import { stackSolutionNote } from './viewer/useViewerReadout';
import CalibrationMasterButton from './CalibrationMasterInspector';
import StackColorProcessingControls from './StackColorProcessingControls';
import StackContinuumResults from './StackContinuumResults';
import StackColorCalibrationLine from './StackColorCalibrationLine';
import {
  appliedProcessingQueryKey,
  channelProcessingFrom,
  colorCatalogQueryPrefix,
} from './channelProcessing';
import {
  processingForColorBuild,
  combinedColorProcessing,
  automaticNarrowbandProcessing,
  retainColorInputRoles,
} from './stackColorProcessing';
import { isColorStackSkyOriented } from './stackOrientation';
import { cropLabels, cropOrder, describeCrop, offCenterChannels } from './stackColorCrop';
import { STACK_ACTIVITY_QUERY_KEY, useStackActivity } from '../hooks/useStackActivity';
import { JOBS_QUERY_KEY } from '../api/jobs';
import { buildColorExposureSets, colorSourceKey, colorSourcesLabel, completedColorArtifact, resolveColorSources, sameColorSourceFamily, wbppColorSet, type ColorExposureSet } from './stackColorSources';
import { withRetainedColorExposureSets } from './colorExposureCards';
import { fetchDefaultColorFor } from './defaultColorSetup';
import './StackColorSources.css';

/** A channel's frame count, or that WBPP made it and recorded none. */
function sourceFramesText(source: Pick<StackColorSource, 'accepted_frames' | 'wbpp'>): string {
  return source.wbpp ? 'WBPP master' : `${source.accepted_frames} frames`;
}

interface StackColorPreviewPanelProps {
  dbId: string;
  projectId: number;
  sourceRevision: string;
  channelBuildRunning: boolean;
  outdatedSourceKeys: ReadonlySet<string>;
  canCompute: boolean;
  onOpenImage: (imageId: number) => void;
  /** The target chosen in the header; null shows every target's compositions. */
  targetId?: number | null;
}

interface ColorOperation {
  automaticPalette?: boolean;
  stars?: StackColorStars;
  dbId: string;
  projectId: number;
  targetId: number;
  kind: StackColorKind;
  combination?: StackColorCombination;
  palette?: StackNarrowbandPalette;
  force: boolean;
  operationKey: string;
  cardKey: string;
  crop: StackColorCrop;
  /** The processing to run, or, for a Build, how to choose it once the
   *  default Color setup has been read from the server. */
  processing: StackColorProcessing
    | ((defaults: (roles: StackColorRole[]) => StackColorProcessing) => StackColorProcessing);
  inputSources?: StackColorInputSources;
}

type PaletteSelection = StackNarrowbandPalette | 'automatic';
type StarsSelection = StackColorStars | 'channels';

type ExposureCardSet = ColorExposureSet & { retained?: boolean };

/** A refused start, kept on the card that asked so it shows where the person looked. */
interface CardStartError {
  message: string;
  /** Whether Apply processing stack asked, rather than Build. */
  processing: boolean;
}

// A stopped composition is over too: its card frees its buttons and stops
// asking after it. `replaced`: a composition that was being prepared became
// the one it names, which is followed instead.
const terminalStates = new Set(['completed', 'failed', 'cancelled', 'replaced']);
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

/**
 * Every FITS a colour stack's card offers, as it labels them: the colour
 * stack, each channel's linear stack, and what its processing made.
 */
function colorStackDownloads(dbId: string, job: StackColorJob): FitsDownload[] {
  const downloads: FitsDownload[] = [{
    label: job.label === 'RGB' ? 'Download RGB FITS' : `Download ${job.label} RGB FITS`,
    href: apiClient.getStackColorFitsUrl(dbId, job.job_id, job.artifact_revision),
  }];
  for (const source of job.sources) {
    downloads.push({
      label: `Download ${roleLabels[source.role]} channel linear FITS`,
      href: apiClient.getStackFitsUrl(dbId, source.job_id, source.group_index, source.artifact_revision),
    });
  }
  for (const [role, result] of Object.entries(job.resolved_input_rc_astro ?? {})) {
    if (!result) continue;
    const label = `${roleLabels[role]} input`;
    downloads.push({ label: `Download ${label} ${result.has_stars ? 'starless FITS' : 'processed FITS'}`, href: result.fits_url });
    if (result.stars_fits_url) downloads.push({ label: `Download ${label} stars FITS`, href: result.stars_fits_url });
  }
  if (job.resolved_color_rc_astro) {
    downloads.push({ label: 'Download processed color channels FITS', href: job.resolved_color_rc_astro.fits_url });
  }
  for (const [label, separation] of [
    ['Narrowband nebula', job.narrowband_resolution?.nebula_star_separation],
    ['RGB', job.narrowband_resolution?.rgb_star_separation],
  ] as const) {
    if (!separation) continue;
    downloads.push({ label: `Download linear ${label} starless FITS`, href: separation.fits_url });
    if (separation.stars_fits_url) downloads.push({ label: `Download linear ${label} stars FITS`, href: separation.stars_fits_url });
  }
  for (const line of job.continuum ?? []) {
    if (line.flux_fits_url) {
      downloads.push({
        label: `Download calibrated ${roleLabels[line.role]} line flux FITS (${line.flux_unit})`,
        href: line.flux_fits_url,
      });
    }
    if (line.residual_fits_url) {
      downloads.push({ label: `Download signed ${roleLabels[line.role]} residual FITS`, href: line.residual_fits_url });
    }
  }
  return downloads;
}

function catalogQueryKey(dbId: string, projectId: number, sourceRevision: string) {
  return [...colorCatalogQueryPrefix(dbId, projectId), sourceRevision] as const;
}

function jobQueryKey(dbId: string, projectId: number, jobId: string | null) {
  return ['db', dbId, 'project', projectId, 'stack-color', jobId] as const;
}

function operationKey(
  targetId: number,
  kind: StackColorKind,
  palette?: PaletteSelection
) {
  return `${targetId}:${kind}:${palette ?? ''}`;
}

function jobMatches(
  job: StackColorJob,
  targetId: number,
  kind: StackColorKind,
  palette?: StackNarrowbandPalette,
  automaticPalette = false,
  combination?: StackColorCombination,
) {
  return (job.combination ?? undefined) === combination && job.target_id === targetId && job.kind === kind && (automaticPalette
    ? job.automatic_palette === true : job.palette === (palette ?? null));
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
  projectId,
  channelSources,
  target,
  kind,
  palette,
  automaticPalette = false,
  stars = 'channels',
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
  onStarsChange,
  onCropChange,
  onBuild,
  onInspect,
  onProcessingApply,
  startError,
  sourceControls,
  starSourceControls,
  exposureSetKey,
  exposureLabel,
  custom,
  unavailableReason,
  combination,
}: {
  dbId: string;
  projectId: number;
  /** The channel stacks the next build composes. */
  channelSources: StackColorSource[];
  target: StackColorTargetAvailability;
  kind: StackColorKind;
  combination?: StackColorCombination;
  palette?: StackNarrowbandPalette;
  automaticPalette?: boolean;
  stars?: StarsSelection;
  paletteChoices: StackNarrowbandPalette[];
  crop: StackColorCrop;
  artifact?: StackColorJob;
  activeJob?: StackColorJob;
  busy: boolean;
  operationPending: boolean;
  unavailable: boolean;
  sourceStacksOutdated: boolean;
  canCompute: boolean;
  onPaletteChange?: (palette: PaletteSelection) => void;
  onStarsChange?: (stars: StarsSelection) => void;
  onCropChange: (crop: StackColorCrop) => void;
  onBuild: () => void;
  onInspect: (job: StackColorJob) => void;
  onProcessingApply: (processing: StackColorProcessing) => void;
  /** Why the server refused this card's last build or Apply. */
  startError?: CardStartError;
  sourceControls?: ReactNode;
  starSourceControls?: ReactNode;
  exposureSetKey?: string;
  exposureLabel?: string;
  custom?: boolean;
  unavailableReason?: string;
}) {
  const current = activeJob ?? artifact;
  const state = activeJob?.state ?? (artifact ? 'completed' : 'not-built');
  // What each channel card shows, read from the queries the stacks panel
  // already keeps.
  const channelPreviews = useQueries({
    queries: channelSources.map((source) => ({
      queryKey: appliedProcessingQueryKey(
        dbId, projectId, source.job_id, source.group_index, source.artifact_revision
      ),
      queryFn: ({ signal }: { signal: AbortSignal }) => apiClient.getAppliedStackStretch(
        dbId, source.job_id, source.group_index, source.artifact_revision, signal
      ),
      retry: false,
    })),
  });
  const channelProcessing = channelProcessingFrom(
    channelSources, channelPreviews.map((query) => query.data)
  );
  const baseLabel = kind === 'rgb'
    ? 'RGB'
    : kind === 'lrgb'
      ? 'LRGB'
      : palette
        ? paletteLabels[palette].split(' · ')[0]
        : 'Narrowband';
  const label = [baseLabel, combination === 'emission' ? '+ emission' : combination ? '+ broadband structure' : undefined, exposureLabel, custom ? 'custom' : undefined].filter(Boolean).join(' ');
  const detailedProgress = activeJob?.progress ?? artifact?.progress;
  const processed = detailedProgress?.total_units
    ? detailedProgress.completed_units
    : activeJob?.processed_channels ?? artifact?.total_channels ?? 0;
  const total = detailedProgress?.total_units
    ? detailedProgress.total_units
    : activeJob?.total_channels ?? artifact?.total_channels ?? expectedChannelCount(kind, palette);
  const percent = state === 'completed' ? 100 : total > 0 ? Math.min(100, processed / total * 100) : 0;
  const sourceFrames = artifact?.sources.reduce((sum, source) => sum + source.accepted_frames, 0) ?? 0;
  const sourceSeconds = totalIntegration((artifact?.sources ?? []).map((source) => source.total_exposure_seconds));
  const stateLabel =
    state === 'preparing' ? activeJob?.phase || 'Preparing' :
    state === 'queued' ? 'Waiting for color processor' :
      state === 'running' ? activeJob?.phase ?? 'Building color preview' :
        state === 'completed' ? current?.phase ?? 'Color preview ready' :
          state === 'failed' ? 'Color preview failed' :
            state === 'cancelled' ? activeJob?.phase || 'Stopped' : 'Not built';

  return (
    <article
      className={`stack-color-card ${artifact?.outdated || sourceStacksOutdated ? 'outdated' : ''}`}
      data-color-kind={kind}
      data-color-combination={combination}
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
                aria-label={`${target.target_name} ${combination ? 'combined ' : ''}narrowband${exposureLabel ? ` ${exposureLabel}` : custom ? ' custom' : ''} palette`}
                value={automaticPalette ? 'automatic' : palette}
                disabled={busy}
                onChange={(event) => onPaletteChange?.(event.target.value as PaletteSelection)}
              >
                {!combination && <option value="automatic">Automatic ({baseLabel})</option>}
                {paletteChoices.map((choice) => (
                  <option key={choice} value={choice}>{paletteLabels[choice]}</option>
                ))}
              </select>
            </label>
          ) : <span className="stack-preview-channel">{baseLabel}</span>}
          {kind === 'narrowband' && <label className="stack-color-palette stack-color-star-policy">
            <span><span aria-hidden="true">✨</span> Stars</span>
            <select aria-label={`${target.target_name} ${label} stars`} value={stars} disabled={busy}
              onChange={(event) => onStarsChange?.(event.target.value as StarsSelection)}>
              {!combination && stars === 'channels' && <option value="channels">From channel cards</option>}
              <option value="automatic">Automatic</option>
              <option value="rgb">RGB</option>
              {!combination && <option value="narrowband">Narrowband</option>}
              <option value="none">None</option>
            </select>
          </label>}
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
            {operationPending ? 'Building…' : state === 'preparing' ? 'Preparing…' : artifact ? 'Rebuild' : 'Build'}
          </button>
        </div>
      </header>
      {startError && !startError.processing && (
        <div className="stack-preview-message error" role="alert">{startError.message}</div>
      )}
      {stars === 'channels' && !combination && sourceControls}
      {kind === 'narrowband' && current?.narrowband_resolution && <div className="stack-color-unavailable">
        {current.narrowband_resolution.detail}
      </div>}
      {kind === 'narrowband' && current?.narrowband_resolution?.stars === 'rgb'
        && current.narrowband_resolution.color_calibration && <section aria-label="RGB star color calibration">
          <span>RGB star colors</span>
          <StackColorCalibrationLine outcome={current.narrowband_resolution.color_calibration} canInstall={canCompute} />
        </section>}
      {current?.narrowband_resolution?.rgb_star_processing && <p className="processing-setups-hint">
        {combination ? 'RGB branch' : 'RGB stars'} processed with {current.narrowband_resolution.rgb_star_processing.steps.map((step) => step.name).join(' → ')} before separation.
      </p>}
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

      {current && <StackContinuumResults dbId={dbId} job={current} />}

      {artifact?.color_calibration && (
        <StackColorCalibrationLine outcome={artifact.color_calibration} canInstall={canCompute} />
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
        roles={combination ? channelSources.map((source) => source.role) : requiredRoles(kind, palette)}
        combination={combination}
        applied={artifact?.processing ?? null}
        automaticNarrowband={kind === 'narrowband' && !artifact}
        palette={palette}
        balanceReport={current?.narrowband_balance}
        managesStars={stars !== 'channels'}
        stars={stars === 'channels' ? undefined : stars}
        resolvedStars={current?.narrowband_resolution?.stars}
        starSeparation={artifact?.narrowband_resolution}
        backgrounds={artifact?.resolved_backgrounds ?? {}}
        protections={artifact?.resolved_background_protection ?? {}}
        fallbacks={artifact?.background_protection_fallbacks ?? {}}
        deconvolutions={artifact?.resolved_input_deconvolutions ?? {}}
        rcAstro={artifact?.resolved_input_rc_astro ?? {}}
        colorRcAstro={artifact?.resolved_color_rc_astro}
        channelProcessing={channelProcessing}
        followed={artifact?.followed_channel_processing}
        disabled={!canCompute || busy || unavailable}
        applyError={startError?.processing ? startError.message : null}
        onApply={onProcessingApply}
      />

      {(stars !== 'channels' || combination) && (sourceControls || starSourceControls) && <details className="stack-color-combination">
        <summary>Custom combination</summary>
        {sourceControls}
        {starSourceControls}
      </details>}

      {current && (
        <div className="stack-color-sources" aria-label={`${label} source stacks`}>
          {current.sources.map((source) => (
            <span key={`${source.role}:${source.job_id}:${source.group_index}`}>
              <strong>{roleLabels[source.role]}</strong>
              {colorSourcesLabel([source])}
              <small>
                {sourceFramesText(source)}
                {(source.total_exposure_seconds ?? 0) > 0 && ` · ${formatIntegration(source.total_exposure_seconds)}`}
              </small>
            </span>
          ))}
          {artifact && !artifact.sources.some((source) => source.wbpp) && (
            <span className="stack-color-source-total">
              <strong>{sourceFrames}</strong> integrated inputs
              {sourceSeconds > 0 && <> · <strong>{formatIntegration(sourceSeconds)}</strong> integrated</>}
            </span>
          )}
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
  targetId = null,
}: StackColorPreviewPanelProps) {
  const queryClient = useQueryClient();
  const [watchedJobIds, setWatchedJobIds] = useState<string[]>([]);
  // The card that asked for each composition still being prepared, which
  // names no channel stacks yet to match a card by, and whether it came
  // from **Apply processing stack**.
  const preparingCards = useRef(new Map<string, { card: string; processing: boolean }>());
  const [paletteByCard, setPaletteByCard] = useState<Record<string, PaletteSelection>>({});
  const [starsByCard, setStarsByCard] = useState<Record<string, StarsSelection>>({});
  const [rgbSourcesByCard, setRgbSourcesByCard] = useState<Record<string, Partial<Record<StackColorRole, string>>>>({});
  const [cropByCard, setCropByCard] = useState<Record<string, StackColorCrop>>({});
  const [sourcesByCard, setSourcesByCard] = useState<Record<string, Partial<Record<StackColorRole, string>>>>({});
  const [savedByCard, setSavedByCard] = useState<Record<string, string>>({});
  const [pendingCardKeys, setPendingCardKeys] = useState<ReadonlySet<string>>(new Set());
  const [inspector, setInspector] = useState<StackColorJob | null>(null);
  // The stack's plate solution, for the inspector's RA/Dec and sky overlay.
  const inspectorSolution = useQuery({
    queryKey: ['db', dbId, 'stack-solution', inspector?.job_id, inspector?.artifact_revision],
    queryFn: () => apiClient.getStackColorSolution(dbId, inspector!.job_id, inspector!.artifact_revision),
    enabled: inspector !== null,
    retry: false,
    staleTime: 60_000,
  });

  const catalog = useQuery({
    queryKey: catalogQueryKey(dbId, projectId, sourceRevision),
    queryFn: () => apiClient.getStackColorCatalog(dbId, projectId),
  });

  const [startErrorByCard, setStartErrorByCard] = useState<Record<string, CardStartError>>({});
  const {
    mutate: startColor,
    reset: resetStart,
  } = useMutation({
    // The card shows the build as pending from the click, while the
    // default Color setup is read and while the server queues the build.
    mutationFn: async (operation: ColorOperation) => apiClient.startStackColor(operation.dbId, operation.projectId, {
      target_id: operation.targetId,
      kind: operation.kind,
      ...(operation.combination ? { combination: operation.combination } : {}),
      palette: operation.palette,
      ...(operation.automaticPalette ? { automatic_palette: true } : {}),
      ...(operation.stars ? { stars: operation.stars } : {}),
      force: operation.force,
      crop: operation.crop,
      processing: typeof operation.processing === 'function'
        ? operation.processing(await fetchDefaultColorFor(queryClient))
        : operation.processing,
      ...(operation.inputSources ? { input_sources: operation.inputSources } : {}),
    }),
    onMutate: (operation) => {
      setPendingCardKeys((current) => new Set(current).add(operation.cardKey));
      setStartErrorByCard((current) => {
        const next = { ...current };
        delete next[operation.cardKey];
        return next;
      });
    },
    onError: (error, operation) => {
      if (operation.dbId !== dbId || operation.projectId !== projectId) return;
      setStartErrorByCard((current) => ({
        ...current,
        [operation.cardKey]: {
          message: error instanceof Error ? error.message : 'Color preview failed',
          processing: operation.operationKey.endsWith(':processing'),
        },
      }));
    },
    onSettled: (_data, _error, operation) => setPendingCardKeys((current) => {
      const next = new Set(current);
      next.delete(operation.cardKey);
      return next;
    }),
    onSuccess: (job, operation) => {
      queryClient.setQueryData(jobQueryKey(operation.dbId, operation.projectId, job.job_id), job);
      queryClient.invalidateQueries({ queryKey: STACK_ACTIVITY_QUERY_KEY });
      queryClient.invalidateQueries({ queryKey: JOBS_QUERY_KEY });
      if (terminalStates.has(job.state)) {
        queryClient.invalidateQueries({
          queryKey: colorCatalogQueryPrefix(operation.dbId, operation.projectId),
        });
      }
      if (operation.dbId !== dbId || operation.projectId !== projectId) return;
      if (job.state === 'preparing') {
        preparingCards.current.set(job.job_id, {
          card: operation.cardKey,
          processing: operation.operationKey.endsWith(':processing'),
        });
      }
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
        .filter((job): job is StackColorJob => job !== undefined && job.state !== 'replaced')
        .sort((left, right) => left.created_unix_seconds - right.created_unix_seconds),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- useQueries returns a new array each render; the join tracks content.
    [statuses.map((status) => status.dataUpdatedAt).join('|')]
  );
  // A composition that was prepared is followed as the one it became.
  const replacements = statuses
    .map((status) => status.data)
    .filter((job): job is StackColorJob => job?.state === 'replaced');
  const replacementKey = replacements
    .map((job) => `${job.job_id}>${(job.replaced_by ?? []).join(',')}`)
    .join('|');
  useEffect(() => {
    if (replacements.length === 0) return;
    const replacedBy = new Map(replacements.map((job) => [job.job_id, job.replaced_by ?? []]));
    for (const job of replacements) {
      preparingCards.current.delete(job.job_id);
      for (const next of job.replaced_by ?? []) {
        queryClient.invalidateQueries({ queryKey: jobQueryKey(dbId, projectId, next) });
      }
    }
    setWatchedJobIds((current) => {
      const next = current.flatMap((jobId) => replacedBy.get(jobId) ?? [jobId]);
      return [...new Set(next)];
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the key tracks the replacements' content.
  }, [replacementKey, dbId, projectId, queryClient]);
  // One the server no longer keeps, as after a restart, is let go quietly.
  const goneKey = statuses
    .map((status, index) => (httpStatus(status.error) === 404 ? watchedJobIds[index] : ''))
    .filter(Boolean)
    .join('|');
  useEffect(() => {
    if (!goneKey) return;
    const gone = new Set(goneKey.split('|'));
    for (const jobId of gone) preparingCards.current.delete(jobId);
    setWatchedJobIds((current) => current.filter((jobId) => !gone.has(jobId)));
  }, [goneKey]);
  const statusError = statuses.find((status) => status.error && httpStatus(status.error) !== 404)?.error;
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
    setStarsByCard({});
    setRgbSourcesByCard({});
    setCropByCard({});
    setSourcesByCard({});
    setSavedByCard({});
    setPendingCardKeys(new Set());
    setStartErrorByCard({});
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
      // With a target chosen in the header, only its compositions.
      (targetId === null || target.target_id === targetId) && (
      target.rgb_available || target.lrgb_available || target.narrowband_palettes.length > 0 ||
      target.source_candidates?.some((source) => source.exposure_group) ||
      jobs.some((job) => job.target_id === target.target_id))
    );
  }, [catalog.data, watchedJobs, targetId]);

  if (targets.length === 0 && !catalog.error) return null;

  // Builds queue on the server, so other cards stay available while one runs.
  // channelBuildRunning no longer locks color out; a queued color job simply
  // waits its turn behind the mono builds on the shared permit.
  void channelBuildRunning;
  const error = statusError ?? catalog.error;

  // Why a composition this page asked for could not be prepared, on a card
  // that still shows its previous preview, placed as a refused start is:
  // beside **Apply processing stack** when it came from there, at the top
  // of the card otherwise. A card without a preview shows it in place of
  // the image.
  const preparationFailure = (job: StackColorJob | undefined): CardStartError | undefined => {
    const asked = job && preparingCards.current.get(job.job_id);
    return job?.state === 'failed' && asked && job.error
      ? { message: job.error, processing: asked.processing }
      : undefined;
  };

  const newestWatched = (
    targetId: number,
    kind: StackColorKind,
    palette?: StackNarrowbandPalette,
    matchesFamily: (job: StackColorJob) => boolean = () => true,
    automaticPalette = false,
    combination?: StackColorCombination,
  ) => {
    const mine = watchedJobs.filter((job) => jobMatches(job, targetId, kind, palette, automaticPalette, combination) && matchesFamily(job));
    return mine[mine.length - 1];
  };

  const sourceSelection = (
    target: StackColorTargetAvailability,
    kind: StackColorKind,
    palette?: StackNarrowbandPalette,
    exposureSet?: ExposureCardSet,
    automaticPalette = false,
    combination?: StackColorCombination,
  ) => {
    const key = dbId + ':' + projectId + ':' + operationKey(target.target_id, kind, automaticPalette ? 'automatic' : palette)
      + (exposureSet ? `:exposure:${exposureSet.key}` : '') + (combination ? `:${combination}` : '');
    const roles = [...requiredRoles(kind, palette), ...(combination === 'emission'
      ? (['ha','oiii','sii'] as const).filter((role) => target.source_candidates?.some((source) => source.role === role))
      : combination ? ['red','green','blue'] as StackColorRole[] : [])];
    const candidates = (target.source_candidates ?? []).filter((source) => !exposureSet
      || exposureSet.candidates.some((member) => sameColorSourceFamily(source, member)));
    const aware = target.source_candidates !== undefined;
    const resolved = resolveColorSources(candidates, roles, exposureSet ? {} : sourcesByCard[key] ?? {});
    const family = exposureSet ? resolveColorSources(exposureSet.candidates, roles, {}) : resolved;
    const saved = (catalog.data?.jobs ?? []).filter((job) => jobMatches(job, target.target_id, kind, palette, automaticPalette, combination));
    const selectedSaved = exposureSet ? undefined : saved.find((job) => job.job_id === savedByCard[key]);
    // One still being prepared names no stacks yet: it shows on the card
    // that asked for it, or, picked up from the header, on the plain card.
    const preparingMatches = (job: StackColorJob) => {
      const asked = preparingCards.current.get(job.job_id)?.card;
      return asked === undefined ? !exposureSet : asked === key;
    };
    // One this page asked for that failed while it was prepared lists no
    // source stacks yet, and the card that asked for it shows why.
    const unprepared = (job: StackColorJob) => job.state === 'preparing'
      || (job.state === 'failed' && preparingCards.current.has(job.job_id));
    const matchesFamily = (job: StackColorJob) => unprepared(job) ? preparingMatches(job) : (!exposureSet && (!aware
      || (candidates.length === 0 && (!selectedSaved || selectedSaved.job_id === job.job_id)))) || (family.complete
      && (automaticPalette && job.automatic_palette
        ? job.sources.filter((source) => roles.includes(source.role))
          .every((previous) => family.sources.some((source) => sameColorSourceFamily(source, previous)))
        : family.sources.every((source) => job.sources.some((previous) => sameColorSourceFamily(source, previous)))));
    const showChoices = !exposureSet && (Boolean(combination) || candidates.some((source) => source.exposure_group)
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
                {colorSourcesLabel([source])} · {sourceFramesText(source)}
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
    automaticPalette = false,
    combination?: StackColorCombination,
  ) => {
    const sources = sourceSelection(target, kind, palette, exposureSet, automaticPalette, combination);
    const watched = newestWatched(target.target_id, kind, palette, sources.matchesFamily, automaticPalette, combination);
    const artifact = completedColorArtifact(watched, catalog.data?.jobs ?? [], sources.remembered);
    if (!available && !artifact && !watched && !exposureSet) return null;
    const key = sources.key;
    const starKey = paletteStateKey ?? key;
    const stars: StarsSelection = kind === 'narrowband'
      ? starsByCard[starKey] ?? artifact?.stars ?? (artifact ? 'channels' : 'automatic') : 'channels';
    const usesRgbStars = !combination && kind === 'narrowband' && (stars === 'rgb' || stars === 'automatic');
    const rgbKey = `${dbId}:${projectId}:${starKey}`;
    const rgbRoles: StackColorRole[] = ['red', 'green', 'blue'];
    const rgbCandidates = (target.source_candidates ?? []).filter((source) => rgbRoles.includes(source.role));
    const rgbChoices = { ...Object.fromEntries((artifact?.sources ?? []).filter((source) => rgbRoles.includes(source.role)).map((previous) => {
      const current = rgbCandidates.filter((candidate) => sameColorSourceFamily(candidate, previous));
      return [previous.role, current.length === 1 ? colorSourceKey(current[0]) : ''];
    })), ...rgbSourcesByCard[rgbKey] };
    const chosenRgb = resolveColorSources(rgbCandidates, rgbRoles, rgbChoices);
    const staleRgbChoice = Object.entries(rgbChoices).some(([role, value]) => value && !rgbCandidates.some((source) => source.role === role && colorSourceKey(source) === value));
    const rgbInputs = Object.fromEntries(Object.entries(rgbChoices).flatMap(([role, value]) => {
      const source = rgbCandidates.find((candidate) => candidate.role === role && colorSourceKey(candidate) === value);
      return source ? [[role, { job_id: source.job_id, group_index: source.group_index, artifact_revision: source.artifact_revision }]] : [];
    }));
    const inputSources = usesRgbStars
      ? { ...sources.inputSources, ...rgbInputs } : sources.inputSources;
    const processingRoles = [...new Set([...requiredRoles(kind, palette),
      ...(combination ? sources.sources.map((source) => source.role) : []),
      ...(automaticPalette ? ['sii' as const] : []),
      ...(usesRgbStars ? rgbRoles : [])])];
    const starSourceControls = usesRgbStars ? <section className="stack-color-source-selectors" aria-label="RGB star inputs">
      <h4>RGB star inputs</h4>
      {rgbRoles.map((role) => {
        const choices = rgbCandidates.filter((source) => source.role === role);
        const chosen = chosenRgb.sources.find((source) => source.role === role);
        return choices.length <= 1 ? <span key={role}>{roleLabels[role]}: {chosen ? colorSourcesLabel([chosen]) : 'Unavailable'}</span> : <label key={role}>
          <span>{roleLabels[role]}</span>
          <select aria-label={`${target.target_name} ${roleLabels[role]} RGB stars source stack`} value={chosen ? colorSourceKey(chosen) : ''}
            onChange={(event) => setRgbSourcesByCard((current) => ({ ...current, [rgbKey]: { ...current[rgbKey], [role]: event.target.value } }))}>
            <option value="">Automatic source choice</option>
            {choices.map((source) => <option key={colorSourceKey(source)} value={colorSourceKey(source)}>{colorSourcesLabel([source])}</option>)}
          </select>
        </label>;
      })}
    </section> : undefined;
    const crop = cropByCard[key] ?? artifact?.crop ?? 'none';
    const operationPending = pendingCardKeys.has(key);
    const cardBusy = operationPending || (watched !== undefined && !terminalStates.has(watched.state));
    const card = (
      <ColorCard
        key={key}
        dbId={dbId}
        projectId={projectId}
        channelSources={usesRgbStars ? [...sources.sources, ...chosenRgb.sources] : sources.sources}
        target={target}
        kind={kind}
        combination={combination}
        palette={palette}
        automaticPalette={automaticPalette}
        stars={stars}
        paletteChoices={paletteChoices}
        crop={crop}
        artifact={artifact}
        activeJob={watched?.state === 'completed' ? undefined : watched}
        busy={cardBusy}
        operationPending={operationPending}
        unavailable={(!exposureSet && !available) || !sources.complete || (usesRgbStars && staleRgbChoice)}
        unavailableReason={usesRgbStars && staleRgbChoice ? 'A selected RGB star source is no longer current; choose it again in Custom combination' : sources.unavailableReason}
        sourceControls={sources.controls}
        starSourceControls={starSourceControls}
        exposureSetKey={exposureSet?.key}
        exposureLabel={exposureSet ? (exposureSet.retained ? 'Previous ' : '') + exposureSet.label : undefined}
        custom={custom}
        sourceStacksOutdated={[...sources.sources, ...(artifact?.sources ?? [])]
          .some((source) => outdatedSourceKeys.has(colorSourceKey(source)))}
        canCompute={canCompute}
        onPaletteChange={paletteStateKey ? (next) => setPaletteByCard((current) => ({
          ...current, [paletteStateKey]: next,
        })) : undefined}
        onStarsChange={(next) => setStarsByCard((current) => ({ ...current, [starKey]: next }))}
        onCropChange={(next) => setCropByCard((current) => ({ ...current, [key]: next }))}
        onBuild={() => startColor({
          dbId, projectId,
          targetId: target.target_id, kind, palette, combination,
          automaticPalette, stars: kind === 'narrowband' && stars !== 'channels' ? stars : undefined,
          force: Boolean(artifact && !artifact.outdated),
          operationKey: key, cardKey: key, crop,
          // A new preview starts from the default Color setup.
          processing: (defaults) => {
            const processing = processingForColorBuild(artifact, requiredRoles(kind, palette), defaults);
            const selected = retainColorInputRoles(processing, processingRoles);
            return combinedColorProcessing(kind === 'narrowband' && !artifact ? automaticNarrowbandProcessing(selected) : selected, combination, processingRoles);
          },
          inputSources,
        })}
        onInspect={setInspector}
        startError={startErrorByCard[key] ?? (artifact ? preparationFailure(watched) : undefined)}
        onProcessingApply={(processing) => startColor({
          dbId, projectId,
          targetId: target.target_id, kind, palette, combination, force: false,
          automaticPalette, stars: kind === 'narrowband' && stars !== 'channels' ? stars : undefined,
          operationKey: key + ':processing', cardKey: key, crop, processing: combinedColorProcessing(retainColorInputRoles(processing, processingRoles), combination, processingRoles),
          inputSources,
        })}
      />
    );
    return custom ? (
      <details key={key + ':custom'} className="stack-color-custom"
        data-color-kind={kind} data-target-id={target.target_id}
        aria-label={target.target_name + ' ' + kind.toUpperCase() + (combination ? ' ' + combination : '') + ' custom combination'}>
        <summary><span>{combination === 'emission' ? `${kind.toUpperCase()} + emission` : combination ? 'Narrowband + broadband structure' : 'Custom combination'}</span><small>{target.target_name} · {kind.toUpperCase()}</small></summary>
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
              jobs.filter((job) => job.kind === kind && !job.combination), roles,
            ) : [];
            const wbpp = wbppColorSet(candidates, roles, { exposureMode });
            for (const set of [...sets, ...(wbpp ? [{ ...wbpp, retained: false }] : [])]) {
              cards.push(renderCard(target, kind, available, set));
            }
            cards.push(renderCard(target, kind, available, undefined, undefined, [], undefined,
              exposureMode && (sets.length > 0 || jobs.some((job) => job.kind === kind))));
          }

          const narrowbandRoles: StackColorRole[] = ['ha', 'oiii', 'sii'];
          const narrowbandJobs = jobs.filter((job) => job.kind === 'narrowband' && !job.combination);
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
          // Ha and OIII make an HOO palette; SII is optional.
          const wbppNarrowband = wbppColorSet(candidates, narrowbandRoles, {
            exposureMode, needed: ['ha', 'oiii'],
          });
          for (const set of [...sets, ...(wbppNarrowband ? [{ ...wbppNarrowband, retained: false }] : [])]) {
            const paletteKey = target.target_id + ':narrowband:' + set.key;
            const localPalettes = paletteChoices.filter((choice) =>
              resolveColorSources(set.candidates, requiredRoles('narrowband', choice), {}).complete);
            const remembered = [...narrowbandJobs]
              .sort((left, right) => Number(!terminalStates.has(right.state)) - Number(!terminalStates.has(left.state))
                || right.created_unix_seconds - left.created_unix_seconds)
              .find((job) => job.palette && job.sources.filter((source) => narrowbandRoles.includes(source.role)).every((previous) => {
                const matching = set.candidates.filter((source) => source.role === previous.role);
                return matching.length === 1 && sameColorSourceFamily(matching[0], previous);
              }));
            const selection = paletteByCard[paletteKey]
              ?? (remembered && !remembered.automatic_palette ? remembered.palette : 'automatic');
            const automaticPalette = selection === 'automatic';
            const palette = automaticPalette ? defaultPalette(localPalettes.length ? localPalettes : paletteChoices) : selection ?? undefined;
            if (palette) cards.push(renderCard(target, 'narrowband',
              target.narrowband_palettes.includes(palette), set, palette, paletteChoices, paletteKey, false, automaticPalette));
          }
          const paletteKey = target.target_id + ':narrowband:custom';
          const remembered = [...narrowbandJobs].sort((left, right) => right.created_unix_seconds - left.created_unix_seconds)[0];
          const selection = paletteByCard[paletteKey]
            ?? (remembered && !remembered.automatic_palette ? remembered.palette : 'automatic');
          const automaticPalette = selection === 'automatic';
          const usablePalettes = paletteChoices.filter((choice) => resolveColorSources(candidates, requiredRoles('narrowband', choice), {}).complete);
          const palette = automaticPalette ? defaultPalette(usablePalettes.length ? usablePalettes : paletteChoices) : selection ?? undefined;
          if (palette) cards.push(renderCard(target, 'narrowband',
            target.narrowband_palettes.includes(palette), undefined, palette, paletteChoices,
            paletteKey, exposureMode, automaticPalette));
          const hasRgb = ['red','green','blue'].every((role) => candidates.some((source) => source.role === role));
          const hasEmission = candidates.some((source) => ['ha','oiii','sii'].includes(source.role));
          if (hasRgb && hasEmission) {
            cards.push(renderCard(target, 'rgb', true, undefined, undefined, [], undefined, true, false, 'emission'));
            if (candidates.some((source) => source.role === 'luminance')) cards.push(renderCard(target, 'lrgb', true, undefined, undefined, [], undefined, true, false, 'emission'));
            const combinedPaletteKey = target.target_id + ':broadband-structure';
            const savedPalette = jobs.filter((job) => job.combination === 'broadband_structure')
              .sort((a, b) => b.created_unix_seconds - a.created_unix_seconds)[0]?.palette;
            const combinedSelection = paletteByCard[combinedPaletteKey] ?? savedPalette ?? palette;
            const combinedPalette = combinedSelection === 'automatic' ? palette : combinedSelection;
            if (combinedPalette) cards.push(renderCard(target, 'narrowband', true, undefined, combinedPalette, paletteChoices,
              combinedPaletteKey, true, false, 'broadband_structure'));
          }
          // A custom combination takes a whole row; set after the target's
          // cards, so the cards still flow several to a row.
          const custom = (node: ReactNode) => isValidElement(node) && String(node.key).endsWith(':custom');
          return [...cards.filter((node) => !custom(node)), ...cards.filter(custom)];
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
            ...(inspector.sources.some((source) => source.wbpp)
              ? []
              : [`${inspector.sources.reduce((sum, source) => sum + source.accepted_frames, 0)} integrated inputs`]),
            ...(totalIntegration(inspector.sources.map((source) => source.total_exposure_seconds)) > 0
              ? [`${formatIntegration(totalIntegration(inspector.sources.map((source) => source.total_exposure_seconds)))} integrated`]
              : []),
            ...(isColorStackSkyOriented(inspector) ? ['North up · East left'] : []),
          ]}
          imageUrl={apiClient.getStackColorPreviewUrl(
            dbId, inspector.job_id, inspector.artifact_revision, 'original'
          )}
          fitsUrl={apiClient.getStackColorFitsUrl(
            dbId, inspector.job_id, inspector.artifact_revision
          )}
          imageAlt={`Full-resolution ${inspector.label} color preview for ${inspector.target_name}`}
          downloads={colorStackDownloads(dbId, inspector)}
          analysisFitsUrl={apiClient.getStackColorFitsUrl(
            dbId, inspector.job_id, inspector.artifact_revision
          )}
          artifactSource={{
            kind: 'color',
            dbId,
            jobId: inspector.job_id,
            artifactRevision: inspector.artifact_revision,
          }}
          artifactEnabled={canCompute}
          onOpenImage={onOpenImage}
          solution={inspectorSolution.data ?? null}
          solutionUnavailable={stackSolutionNote(inspectorSolution)}
          onClose={() => setInspector(null)}
        />
      )}
    </section>
  );
}
