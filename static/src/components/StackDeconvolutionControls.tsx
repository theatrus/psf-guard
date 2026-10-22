import type {
  StackDeconvolutionConfig,
  StackDeconvolutionResult,
} from '../api/types';
import { defaultDeconvolutionConfig } from './stackDeconvolution';

function NumberField({
  label, value, min, max, step, disabled, onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  disabled: boolean;
  onChange: (value: number) => void;
}) {
  return (
    <label className="stack-stretch-field">
      <span>{label}</span>
      <input
        type="number"
        aria-label={`Deconvolution ${label}`}
        value={Number.isFinite(value) ? value : ''}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        onChange={(event) => onChange(event.target.valueAsNumber)}
      />
    </label>
  );
}

export default function StackDeconvolutionControls({
  label,
  config,
  result,
  disabled,
  onChange,
}: {
  label: string;
  config: StackDeconvolutionConfig | null | undefined;
  result?: StackDeconvolutionResult;
  disabled: boolean;
  onChange: (config: StackDeconvolutionConfig | null) => void;
}) {
  const update = (change: Partial<StackDeconvolutionConfig>) => {
    if (config) onChange({ ...config, ...change });
  };

  return (
    <section className="stack-deconvolution-controls" aria-label={`${label} deconvolution`}>
      <header>
        <label>
          <input
            type="checkbox"
            checked={config != null}
            disabled={disabled}
            onChange={(event) => onChange(
              event.target.checked ? defaultDeconvolutionConfig() : null
            )}
          />
          <strong>Deconvolution</strong>
        </label>
        <span>{config ? 'Linear · before stretch' : 'Off'}</span>
      </header>
      <p>
        Conservative damped Richardson–Lucy restoration using a measured stellar FWHM.
        Inspect bright stars for ringing; this is off unless enabled.
      </p>
      {config && (
        <div className="stack-deconvolution-fields">
          <NumberField label="PSF FWHM (px)" value={config.psf_fwhm_pixels}
            min={0.25} max={100} step={0.1} disabled={disabled}
            onChange={(psf_fwhm_pixels) => update({ psf_fwhm_pixels })} />
          <NumberField label="Iterations" value={config.iterations}
            min={1} max={50} step={1} disabled={disabled}
            onChange={(iterations) => update({ iterations })} />
          <NumberField label="Amount" value={config.amount}
            min={0} max={1} step={0.05} disabled={disabled}
            onChange={(amount) => update({ amount })} />
          <NumberField label="Noise fraction" value={config.noise_fraction}
            min={0} max={0.25} step={0.001} disabled={disabled}
            onChange={(noise_fraction) => update({ noise_fraction })} />
          <NumberField label="Max correction" value={config.max_correction}
            min={1} max={100} step={0.1} disabled={disabled}
            onChange={(max_correction) => update({ max_correction })} />
        </div>
      )}
      {result && <DeconvolutionDiagnostics result={result} />}
    </section>
  );
}

function DeconvolutionDiagnostics({ result }: { result: StackDeconvolutionResult }) {
  if (result.channels.length === 0) return null;
  return (
    <div className="stack-deconvolution-diagnostics">
      {result.channels.map((channel, index) => (
        <span key={index}>
          {result.channels.length > 1 && <strong>Channel {index + 1}</strong>}
          Peak {channel.input_peak.toPrecision(4)} → {channel.output_peak.toPrecision(4)}
          <small>
            flux Δ {(channel.output_flux - channel.input_flux).toExponential(2)}
          </small>
        </span>
      ))}
    </div>
  );
}

/** What a deconvolution ran with and did, read-only: for an input whose
 *  steps come from its channel card, where the settings are the card's. */
export function StackDeconvolutionSummary({
  label,
  result,
}: {
  label: string;
  result: StackDeconvolutionResult;
}) {
  const { config } = result;
  return (
    <section className="stack-deconvolution-controls" aria-label={`${label} deconvolution`}>
      <header>
        <strong>Deconvolution</strong>
        <span>From the channel card</span>
      </header>
      <p>
        PSF FWHM {config.psf_fwhm_pixels.toFixed(2)} px · {config.iterations} iterations ·
        amount {config.amount} · noise fraction {config.noise_fraction} · max correction{' '}
        {config.max_correction}
      </p>
      <DeconvolutionDiagnostics result={result} />
    </section>
  );
}
