import { X } from 'lucide-react';
import NumberInput from '../NumberInput';
import type { SchedulingOverrides, SchedulingValues } from '../../api/directorPreferences';
import { describeLimit, LIMITS, type LimitKey } from './schedulingModel';
import './SchedulingFields.css';

/** Target Scheduler's scheduling limits at one scope. An empty field
 *  inherits: `inherited` says what it would be and `inheritedFrom` where
 *  that comes from. */
export default function SchedulingFields({ overrides, onChange, inherited, inheritedFrom, disabled, label }: {
  overrides: SchedulingOverrides;
  onChange: (next: SchedulingOverrides) => void;
  inherited: SchedulingValues;
  inheritedFrom: (key: LimitKey) => string;
  disabled?: boolean;
  label: string;
}) {
  const set = (key: LimitKey, value: number | boolean | null) => onChange({ ...overrides, [key]: value });
  return <div className="scheduling-fields" role="group" aria-label={label}>
    {LIMITS.map(spec => {
      const own = overrides[spec.key];
      const setHere = own !== null && own !== undefined;
      const fallback = `${describeLimit(spec, inherited[spec.key])} · ${inheritedFrom(spec.key)}`;
      return <div key={spec.key} className={`scheduling-field${setHere ? ' is-set' : ''}`} title={spec.hint}>
        <label htmlFor={`${label}-${spec.key}`}>{spec.label}</label>
        <span className="scheduling-control">
          {spec.kind === 'flag'
            ? <select id={`${label}-${spec.key}`} aria-label={spec.label} disabled={disabled} value={setHere ? String(own) : ''}
                onChange={event => set(spec.key, event.target.value === '' ? null : event.target.value === 'true')}>
                <option value="">Inherit ({describeLimit(spec, inherited[spec.key])})</option>
                <option value="true">On</option>
                <option value="false">Off</option>
              </select>
            : <>
                <NumberInput id={`${label}-${spec.key}`} aria-label={spec.label} disabled={disabled} min={spec.min} max={spec.max} step={spec.step}
                  placeholder={describeLimit(spec, inherited[spec.key])} value={setHere ? Number(own) : ''}
                  onChange={event => set(spec.key, Number(event.target.value))} />
                {spec.unit && <small>{spec.unit}</small>}
              </>}
          {setHere && !disabled && <button type="button" className="scheduling-clear" aria-label={`Inherit ${spec.label.toLowerCase()}`} title={`Inherit: ${fallback}`} onClick={() => set(spec.key, null)}><X size={14} /></button>}
        </span>
        <small className="scheduling-source">{setHere ? 'set here' : fallback}</small>
      </div>;
    })}
  </div>;
}
