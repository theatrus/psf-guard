import type { StackColorCombination, StackColorRole, StackEmissionProcessing } from '../api/types';
const labels: Partial<Record<StackColorRole, string>> = { ha: 'Hα', oiii: 'OIII', sii: 'SII', red: 'R', green: 'G', blue: 'B' };

export default function StackEmissionControls({ value, combination, disabled, onChange }: {
  value: StackEmissionProcessing; combination: StackColorCombination; disabled: boolean;
  onChange: (value: StackEmissionProcessing) => void;
}) {
  const updateLine = (role: StackColorRole, patch: Partial<NonNullable<StackEmissionProcessing['lines'][StackColorRole]>>) => {
    const line = value.lines[role];
    if (line) onChange({ ...value, lines: { ...value.lines, [role]: { ...line, ...patch } } });
  };
  return <section className="stack-emission-controls" aria-label="Continuum subtraction">
    <header><strong>Continuum subtraction</strong><span>Before star removal</span></header>
    <label className="stack-stretch-field">Continuum mode <select aria-label="Continuum mode" disabled={disabled} value={value.mode}
      onChange={(event) => onChange({ ...value, mode: event.target.value as StackEmissionProcessing['mode'] })}>
      <option value="automatic">Automatic · validate stellar fit</option><option value="manual">Manual weights</option><option value="off">Off</option>
    </select></label>
    <p className="processing-setups-hint">Fit aligned linear images before star removal. Producer clipping maps exclude saturated stars. Missing evidence or an unreliable fit skips that line in RGB/LRGB; a mapped palette stops and keeps the previous preview.</p>
    <div className="stack-emission-lines">{Object.entries(value.lines).map(([key, line]) => {
      const role = key as StackColorRole;
      if (!line) return null;
      const references = role === 'oiii' ? ['G', 'B'] : ['R'];
      return <fieldset className="stack-emission-line" key={role} disabled={disabled}>
        <legend>{labels[role]} − {references.join(' / ')} continuum</legend>
        {combination === 'emission' && <label className="stack-stretch-field">Emission amount <input type="number" aria-label={`${labels[role]} emission amount`} min="0" max="5" step="0.05" value={line.amount}
          onChange={(event) => updateLine(role, { amount: event.target.valueAsNumber })} /></label>}
        {value.mode === 'manual' && references.map((reference, index) => <label className="stack-stretch-field" key={reference}>{reference} weight <input type="number" min="0" step="0.001"
          aria-label={`${labels[role]} ${reference} continuum weight`} value={line.coefficients[index] ?? ''}
          onChange={(event) => { const coefficients = [...line.coefficients]; coefficients[index] = event.target.valueAsNumber; updateLine(role, { coefficients }); }} /></label>)}
        {value.mode === 'manual' && role === 'oiii' && <label className="stack-stretch-field">Stellar B/G flux ratio <input type="number" min="0.000001" step="0.05" aria-label="OIII stellar B/G flux ratio"
          value={line.reference_flux_ratios[1] ?? ''} onChange={(event) => updateLine(role, { reference_flux_ratios: [1, event.target.valueAsNumber] })} />
          <small>Leave blank to use the measured ratio. Enter a ratio if automatic photometry cannot measure one.</small></label>}
      </fieldset>;
    })}</div>
    {combination === 'emission' ? <label className="stack-emission-check"><input type="checkbox" aria-label="Add positive emission only" checked={value.positive_contribution}
      disabled={disabled} onChange={(event) => onChange({ ...value, positive_contribution: event.target.checked })} />Add positive emission only; signed residual files retain negative pixels.</label>
      : <label className="stack-stretch-field">Broadband structure <input type="number" aria-label="Broadband structure amount" min="0" max="1" step="0.05" value={value.broadband_amount}
        disabled={disabled} onChange={(event) => onChange({ ...value, broadband_amount: event.target.valueAsNumber })} /><small>0 keeps mapped nebula; 1 uses the prepared broadband nebula. RGB stars are restored once.</small></label>}
    <details><summary>Advanced fit and saturation limits</summary>
      <label className="stack-stretch-field">Usable star budget <input type="number" aria-label="Continuum usable star budget" min="12" max="4096" value={value.fit?.maxStars ?? 400} disabled={disabled}
        onChange={(event) => onChange({ ...value, fit: { ...value.fit, maxStars: event.target.valueAsNumber } })} /></label>
      <p className="processing-setups-hint">Leave working-unit saturation limits blank to use producer clipping maps. A limit must describe these exact prepared pixels; an image maximum is not a saturation limit.</p>
      <div className="stack-stretch-fields">{[...new Set([...Object.keys(value.lines), 'red', ...(value.lines.oiii ? ['green', 'blue'] : [])])].map((key) => {
        const role = key as StackColorRole;
        return <label className="stack-stretch-field" key={role}>{labels[role]} saturation <input type="number" aria-label={`${labels[role]} continuum saturation`} min="0" value={value.saturation?.[role] ?? ''} disabled={disabled}
          onChange={(event) => { const saturation = { ...value.saturation }; if (event.target.value === '') delete saturation[role]; else saturation[role] = event.target.valueAsNumber; onChange({ ...value, saturation }); }} /></label>;
      })}</div>
    </details>
  </section>;
}
