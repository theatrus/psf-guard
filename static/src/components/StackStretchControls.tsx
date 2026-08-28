import { useState } from 'react';
import type {
  StackStretchPendingProgress,
  StackStretchPreview,
  StackViewProcessingRequest,
} from '../api/types';
import StackStretchStageEditor from './StackStretchStageEditor';
import StackDeconvolutionControls from './StackDeconvolutionControls';
import StackRcAstroControls from './StackRcAstroControls';
import ProcessingSetupsBar from './ProcessingSetupsBar';
import { builtinViewSetups } from './processingSetups';
import { validateDeconvolution } from './stackDeconvolution';
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
    onProgress?: (progress: StackStretchPendingProgress) => void
  ) => Promise<StackStretchPreview>;
  onApplied: (preview: StackStretchPreview) => void;
  onRevert: () => void;
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
}: StackStretchControlsProps) {
  const initialType = displayReferred ? 'identity' : 'auto-mtf';
  const [request, setRequest] = useState<StackViewProcessingRequest>(() =>
    ({ ...defaultStretchRequest(initialType), deconvolution: null, rc_astro: null })
  );
  const [pending, setPending] = useState(false);
  const [pendingProgress, setPendingProgress] =
    useState<StackStretchPendingProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  const revert = () => {
    setRequest({ ...defaultStretchRequest(initialType), deconvolution: null, rc_astro: null });
    setError(null);
    onRevert();
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
    // A cleared number field reads as NaN and would serialize as null.
    const badRcAstro = request.rc_astro?.steps.some((step) =>
      Object.values(step.parameters).some(
        (value) => typeof value === 'number' && !Number.isFinite(value)
      )
    );
    if (badRcAstro) {
      setError('Enter a finite value for every RC-Astro parameter');
      return;
    }
    setPending(true);
    setPendingProgress(null);
    setError(null);
    try {
      onApplied(await apply(request, setPendingProgress));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Stretch rendering failed');
    } finally {
      setPending(false);
      setPendingProgress(null);
    }
  };

  return (
    <details className="stack-stretch-controls">
      <summary>
        <span>View processing</span>
        <small>{applied
          ? `${applied.deconvolution ? `${applied.deconvolution.config.psf_fwhm_pixels}px deconv · ` : ''}${applied.rc_astro ? `RC-Astro ×${applied.rc_astro.steps.length} · ` : ''}${stretchModelLabels[applied.config.model.type]} applied`
          : 'Deconvolution off · default stretch'}</small>
      </summary>
      <div className="stack-stretch-body">
        <ProcessingSetupsBar
          kind="view"
          builtins={builtinViewSetups(displayReferred)}
          current={() => request}
          disabled={disabled || pending}
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
          disabled={disabled || pending || displayReferred}
          onChange={(deconvolution) => setRequest((current) => ({
            ...current,
            deconvolution,
          }))}
        />
        <StackRcAstroControls
          label={label}
          config={request.rc_astro}
          result={applied?.rc_astro ?? undefined}
          disabled={disabled || pending || displayReferred}
          onChange={(rc_astro) => setRequest((current) => ({
            ...current,
            rc_astro,
          }))}
        />
        <StackStretchStageEditor
          label={label}
          channels={channels}
          request={request}
          disabled={disabled || pending}
          onChange={setRequest}
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
          <button type="button" disabled={disabled || pending} onClick={submit}>
            {pending
              ? pendingProgress?.fraction !== undefined
                ? `Applying… ${Math.round(pendingProgress.fraction * 100)}%${
                    pendingProgress.stage
                      ? ` · ${pendingProgress.stage.replace('RC-Astro ', '')}`
                      : ''
                  }`
                : 'Applying…'
              : 'Apply processing'}
          </button>
          <button type="button" disabled={disabled || pending || !applied} onClick={revert}>
            Revert processing
          </button>
        </div>
      </div>
    </details>
  );
}
