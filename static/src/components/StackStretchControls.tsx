import { useState } from 'react';
import type { StackStretchPreview, StackStretchRequest } from '../api/types';
import StackStretchStageEditor from './StackStretchStageEditor';
import StackDeconvolutionControls from './StackDeconvolutionControls';
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
  apply: (request: StackStretchRequest) => Promise<StackStretchPreview>;
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
  const [request, setRequest] = useState<StackStretchRequest>(() =>
    defaultStretchRequest(initialType)
  );
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const revert = () => {
    setRequest(defaultStretchRequest(initialType));
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
    setPending(true);
    setError(null);
    try {
      onApplied(await apply(request));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Stretch rendering failed');
    } finally {
      setPending(false);
    }
  };

  return (
    <details className="stack-stretch-controls">
      <summary>
        <span>View processing</span>
        <small>{applied
          ? `${applied.deconvolution ? `${applied.deconvolution.config.psf_fwhm_pixels}px deconv · ` : ''}${stretchModelLabels[applied.config.model.type]} applied`
          : 'Deconvolution off · default stretch'}</small>
      </summary>
      <div className="stack-stretch-body">
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
            {pending ? 'Applying…' : 'Apply processing'}
          </button>
          <button type="button" disabled={disabled || pending || !applied} onClick={revert}>
            Revert processing
          </button>
        </div>
      </div>
    </details>
  );
}
