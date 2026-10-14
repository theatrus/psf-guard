import type { CalibrationRuleField } from '../api/types';
import { checkOptions, type RuleDraft } from '../utils/calibrationRules';

interface CalibrationRuleControlProps {
  field: CalibrationRuleField;
  draft: RuleDraft;
  onChange: (draft: RuleDraft) => void;
  /** What an empty number field means: the default, or the inherited value. */
  placeholder?: string;
  invalid?: boolean;
  /** Names the control for assistive technology; defaults to the field's label. */
  label?: string;
}

/**
 * The input for one tunable calibration check: a number with its unit, a
 * whole count, a switch, a choice, a check's mode, or a frame grade's two
 * limits. The draft is the field's text, parsed on save.
 */
export default function CalibrationRuleControl({
  field,
  draft,
  onChange,
  placeholder,
  invalid = false,
  label = field.label,
}: CalibrationRuleControlProps) {
  const kind = field.kind;
  switch (kind.type) {
    case 'number':
    case 'count':
      return (
        <div className="calibration-rule-number">
          <input
            type="number"
            min={kind.min}
            max={kind.max}
            step={kind.type === 'count' ? 1 : kind.step}
            value={typeof draft === 'string' ? draft : ''}
            placeholder={placeholder}
            aria-label={label}
            aria-invalid={invalid}
            onChange={(event) => onChange(event.target.value)}
          />
          <span className="calibration-rule-unit" aria-hidden="true">{kind.unit}</span>
        </div>
      );
    case 'toggle':
      return (
        <select
          value={draft === true ? 'on' : 'off'}
          aria-label={label}
          onChange={(event) => onChange(event.target.value === 'on')}
        >
          <option value="on">On</option>
          <option value="off">Off</option>
        </select>
      );
    case 'choice':
      return (
        <select value={String(draft)} aria-label={label} onChange={(event) => onChange(event.target.value)}>
          {kind.options.map((option) => (
            <option key={option.value} value={option.value}>{option.label}</option>
          ))}
        </select>
      );
    case 'check':
      return (
        <select value={String(draft)} aria-label={label} onChange={(event) => onChange(event.target.value)}>
          {checkOptions(field).map((option) => (
            <option key={option.value} value={option.value}>{option.label}</option>
          ))}
        </select>
      );
    case 'limits': {
      const limits = typeof draft === 'object' ? draft : { suspect: '', poor: '' };
      const unit = kind.percent ? '%' : kind.unit;
      const step = kind.percent ? kind.step * 100 : kind.step;
      return (
        <div className="calibration-rule-limits">
          {(['suspect', 'poor'] as const).map((name) => (
            <div key={name} className="calibration-rule-limit">
              <span className="calibration-rule-limit-name" aria-hidden="true">{name === 'suspect' ? 'Suspect' : 'Poor'}</span>
              <input
                type="number"
                min={0}
                step={step}
                value={limits[name]}
                placeholder="never"
                aria-label={`${label}: ${name} at`}
                aria-invalid={invalid}
                onChange={(event) => onChange({ ...limits, [name]: event.target.value })}
              />
              <span className="calibration-rule-unit" aria-hidden="true">{unit}</span>
            </div>
          ))}
        </div>
      );
    }
  }
}
