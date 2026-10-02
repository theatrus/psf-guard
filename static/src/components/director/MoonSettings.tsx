import type { DirectorMoonPolicy } from '../../api/directorTypes';
import { defaultMoonPolicy, moonProblem } from './moonPolicy';

export default function MoonSettings({ value, disabled, onChange }: {
  value: DirectorMoonPolicy | undefined; disabled: boolean; onChange: (value: DirectorMoonPolicy) => void;
}) {
  const moon = value ?? defaultMoonPolicy();
  const edit = (patch: Partial<DirectorMoonPolicy>) => onChange({ ...moon, ...patch });
  const fields: { key: keyof Pick<DirectorMoonPolicy, 'separation_degrees' | 'width_days' | 'relax_degrees_per_degree' | 'relax_min_altitude_degrees' | 'relax_max_altitude_degrees'>; label: string; min: number; max: number; unit: string }[] = [
    { key: 'separation_degrees', label: 'Separation at full Moon', min: 0, max: 180, unit: 'deg' },
    { key: 'width_days', label: 'Half-separation width', min: 1, max: 14, unit: 'days' },
    { key: 'relax_degrees_per_degree', label: 'Altitude relaxation', min: 0, max: 180, unit: 'deg/deg' },
    { key: 'relax_min_altitude_degrees', label: 'Relaxation lower altitude', min: -90, max: 90, unit: 'deg' },
    { key: 'relax_max_altitude_degrees', label: 'Relaxation upper / Moon-down altitude', min: -90, max: 90, unit: 'deg' },
  ];
  return <fieldset className="template-moon" disabled={disabled}>
    <legend>Moon avoidance</legend>
    <label><input type="checkbox" checked={moon.enabled} onChange={event => edit({ enabled: event.target.checked })} />Enable Moon avoidance</label>
    <label><input type="checkbox" checked={moon.moon_down} disabled={!moon.enabled} onChange={event => edit({ moon_down: event.target.checked })} />Moon must be down</label>
    <div className="template-moon-fields">{fields.map(field => <label key={field.key}>{field.label}
      <span><input type="number" step={field.key === 'width_days' ? 1 : 'any'} min={field.min} max={field.max} value={Number.isFinite(moon[field.key]) ? moon[field.key] : ''}
        disabled={!moon.enabled} onChange={event => edit({ [field.key]: event.target.value === '' ? Number.NaN : Number(event.target.value) })} />{field.unit}</span>
    </label>)}</div>
    {moonProblem(moon) && <p className="director-error" role="alert">{moonProblem(moon)}</p>}
  </fieldset>;
}
