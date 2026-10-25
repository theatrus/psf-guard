import type { RcAstroProcessing, StackLinearStarSeparation } from '../api/types';
import StackRcAstroControls from './StackRcAstroControls';

const defaultSeparation: RcAstroProcessing = { steps: [{ tool: 'sxt', parameters: {} }] };
const separationTools = ['sxt'] as const;
const outputParameters = ['stars', 'difference', 'unscreen'] as const;

/** Managed linear SXT: parameters are editable; the Stars policy controls its use. */
export default function StackStarSeparationControls({ label, config, result, disabled, onChange }: {
  label: string;
  config: RcAstroProcessing | null | undefined;
  result?: StackLinearStarSeparation | null;
  disabled: boolean;
  onChange: (config: RcAstroProcessing | null) => void;
}) {
  return <section className="stack-color-channel-tools">
    <StackRcAstroControls label={`${label} separation`} heading="Star separation" required
      config={config ?? defaultSeparation} result={result ?? undefined} disabled={disabled}
      toolKeys={separationTools} excludedParameters={outputParameters} onChange={onChange} />
    <p className="processing-setups-hint">Runs on linear data before stretching and keeps both the starless image and stars. Unscreen Stars stays off.
      Changing these settings runs this branch again. Stretch, palette and star-reduction edits reuse the cached linear outputs.</p>
    {result && <div className="stack-stretch-actions">
      <a href={result.fits_url} download>Linear {label} starless FITS</a>
      {result.stars_fits_url && <a href={result.stars_fits_url} download>Linear {label} stars FITS</a>}
    </div>}
  </section>;
}
