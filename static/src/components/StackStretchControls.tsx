import { useEffect, useRef, useState } from 'react';
import type {
  StackStretchPendingProgress,
  StackStretchPreview,
  StackViewProcessingRequest,
} from '../api/types';
import StackStretchStageEditor from './StackStretchStageEditor';
import StackDeconvolutionControls from './StackDeconvolutionControls';
import StackRcAstroControls from './StackRcAstroControls';
import { rcAstroProblem, useRcAstroCapabilities } from './rcAstroSchema';
import ProcessingSetupsBar from './ProcessingSetupsBar';
import ColorCalibrationSwitch from './ColorCalibrationSwitch';
import { builtinViewSetups } from './processingSetups';
import { validateDeconvolution } from './stackDeconvolution';
import { applyingLabel } from './stackProcessingLabel';
import {
  defaultStretchRequest,
  stretchModelLabels,
} from './stackStretchModels';

interface StackStretchControlsProps {
  label: string;
  channels: 1 | 3;
  displayReferred?: boolean;
  disabled?: boolean;
  applied?: StackStretchPreview;
  apply: (
    request: StackViewProcessingRequest,
    onProgress?: (progress: StackStretchPendingProgress) => void,
    signal?: AbortSignal
  ) => Promise<StackStretchPreview>;
  onApplied: (preview: StackStretchPreview) => Promise<void> | void;
  onRevert: () => Promise<void>;
  /**
   * The channel's processing run on the server while it waits or runs,
   * including one asked for before the page was last opened. Apply
   * processing stays busy and shows its progress until it ends.
   */
  serverRun?: StackStretchPendingProgress | null;
}

function requestForPreview(
  applied: StackStretchPreview | undefined,
  displayReferred: boolean
): StackViewProcessingRequest {
  return applied?.request ?? (applied ? {
    model: applied.config.model,
    color_strategy: applied.config.color_strategy,
    deconvolution: applied.deconvolution?.config ?? null,
    rc_astro: null,
  } : {
    ...defaultStretchRequest(displayReferred ? 'identity' : 'auto-mtf'),
    deconvolution: null,
    rc_astro: null,
  });
}

export default function StackStretchControls({
  label,
  channels,
  displayReferred = false,
  disabled = false,
  applied,
  apply,
  onApplied,
  onRevert,
  serverRun = null,
}: StackStretchControlsProps) {
  const [request, setRequest] = useState<StackViewProcessingRequest>(() =>
    requestForPreview(applied, displayReferred)
  );
  const [pending, setPending] = useState(false);
  const rcAstroTools = useRcAstroCapabilities().data?.tools;
  const [reverting, setReverting] = useState(false);
  const [pendingProgress, setPendingProgress] =
    useState<StackStretchPendingProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  // This page's own request, or a run already on the server for the channel.
  const busy = pending || serverRun !== null;
  const operation = useRef<AbortController | null>(null);
  const selected = useRef({ id: applied?.stretch_id, displayReferred });
  useEffect(() => () => operation.current?.abort(), []);
  useEffect(() => {
    if (selected.current.id === applied?.stretch_id &&
        selected.current.displayReferred === displayReferred) return;
    selected.current = { id: applied?.stretch_id, displayReferred };
    setRequest(requestForPreview(applied, displayReferred));
  }, [applied, displayReferred]);

  const revert = async () => {
    const controller = new AbortController();
    operation.current = controller;
    setPending(true);
    setReverting(true);
    setError(null);
    try {
      await onRevert();
      if (!controller.signal.aborted) setRequest(requestForPreview(undefined, displayReferred));
    } catch (cause) {
      if (!controller.signal.aborted) {
        setError(cause instanceof Error ? cause.message : 'Reverting processing failed');
      }
    } finally {
      if (!controller.signal.aborted) {
        setPending(false);
        setReverting(false);
      }
    }
  };
  const submit = async () => {
    if (Object.values(request.model).some((value) =>
      typeof value === 'number' && !Number.isFinite(value))) {
      setError('Enter a finite value for every stretch parameter');
      return;
    }
    const deconvolutionError = validateDeconvolution(request.deconvolution);
    if (deconvolutionError) {
      setError(deconvolutionError);
      return;
    }
    // A cleared number field reads as NaN and would serialize as null; a
    // value past the tool's range would only be refused by the server.
    const rcAstroError = rcAstroProblem(request.rc_astro, rcAstroTools);
    if (rcAstroError) {
      setError(rcAstroError);
      return;
    }
    setPending(true);
    const controller = new AbortController();
    operation.current = controller;
    setPendingProgress(null);
    setError(null);
    try {
      const preview = await apply(request, (progress) => {
        if (!controller.signal.aborted) setPendingProgress(progress);
      }, controller.signal);
      if (!controller.signal.aborted) await onApplied(preview);
    } catch (cause) {
      if (!controller.signal.aborted) {
        setError(cause instanceof Error ? cause.message : 'Stretch rendering failed');
      }
    } finally {
      if (!controller.signal.aborted) {
        setPending(false);
        setPendingProgress(null);
      }
    }
  };

  return (
    <details className="stack-stretch-controls">
      <summary>
        <span>View processing</span>
        <small>{applied
          ? `${applied.deconvolution ? `${applied.deconvolution.config.psf_fwhm_pixels}px deconv · ` : ''}${applied.rc_astro ? `RC-Astro ×${applied.rc_astro.steps.length} · ` : ''}${stretchModelLabels[applied.config.model.type]} applied${applied.reprocessing ? ' to the previous stack' : ''}`
          : 'Deconvolution off · default stretch'}</small>
      </summary>
      <div className="stack-stretch-body">
        <ProcessingSetupsBar
          kind="view"
          builtins={builtinViewSetups(displayReferred)}
          current={() => request}
          disabled={disabled || busy}
          onApply={(settings) => {
            setRequest({
              deconvolution: null,
              rc_astro: null,
              ...(settings as StackViewProcessingRequest),
            });
            setError(null);
          }}
        />
        <StackDeconvolutionControls
          label={label}
          config={request.deconvolution}
          result={applied?.deconvolution ?? undefined}
          disabled={disabled || busy || displayReferred}
          onChange={(deconvolution) => setRequest((current) => ({
            ...current,
            deconvolution,
          }))}
        />
        <StackRcAstroControls
          label={label}
          config={request.rc_astro}
          result={applied?.rc_astro ?? undefined}
          disabled={disabled || busy || displayReferred}
          onChange={(rc_astro) => setRequest((current) => ({
            ...current,
            rc_astro,
          }))}
        />
        {channels === 3 && !displayReferred && (
          <ColorCalibrationSwitch
            calibration={request.color_calibration}
            disabled={disabled || busy}
            onChange={(color_calibration) => setRequest((current) => ({
              ...current,
              color_calibration,
            }))}
          />
        )}
        <StackStretchStageEditor
          label={label}
          channels={channels}
          request={request}
          disabled={disabled || busy}
          onChange={(stretch) => setRequest((current) => ({ ...current, ...stretch }))}
        />
        {(['linear', 'asinh', 'mtf', 'ghs'] as string[]).includes(request.model.type) && (
          <p className="stack-stretch-note">Explicit points use normalized 0–1 display units.</p>
        )}
        {applied && (
          <div className="stack-stretch-stats">
            Source range {applied.linked_statistics.min.toPrecision(4)}–
            {applied.linked_statistics.max.toPrecision(4)}
            {' · '}median {applied.linked_statistics.median.toPrecision(4)}
            {applied.input_range && <>
              {' · '}display normalization {applied.input_range.black.toPrecision(4)}–
              {applied.input_range.white.toPrecision(4)}
            </>}
          </div>
        )}
        {error && <div className="stack-stretch-error" role="alert">{error}</div>}
        <div className="stack-stretch-actions">
          <button
            type="button"
            disabled={disabled || busy}
            onClick={submit}
            aria-busy={busy && !reverting}
          >
            {pending && !reverting
              ? applyingLabel(pendingProgress)
              : serverRun
                ? applyingLabel(serverRun)
                : 'Apply processing'}
          </button>
          <button type="button" disabled={disabled || busy || !applied} onClick={revert}>
            {reverting ? 'Reverting...' : 'Revert processing'}
          </button>
        </div>
      </div>
    </details>
  );
}
