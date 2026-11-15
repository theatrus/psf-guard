import { rangeProblem } from './rcAstroSchema';
import type { ExternalToolParameter, RcAstroParameterValue } from '../api/types';

/** One parameter of a processing step, drawn from its schema. */
export default function ParameterField({
  parameter,
  value,
  disabled,
  onChange,
}: {
  parameter: ExternalToolParameter;
  value: RcAstroParameterValue | undefined;
  disabled: boolean;
  onChange: (value: RcAstroParameterValue) => void;
}) {
  const { kind } = parameter;
  if (kind.type === 'bool') {
    return (
      <label className="stack-stretch-field stack-rc-astro-switch" title={parameter.description}>
        <input
          type="checkbox"
          checked={typeof value === 'boolean' ? value : kind.default}
          disabled={disabled}
          onChange={(event) => onChange(event.target.checked)}
          aria-label={parameter.label}
        />
        <span>{parameter.label}</span>
      </label>
    );
  }
  const numeric = typeof value === 'number' ? value : kind.default;
  const problem = value === undefined ? null : rangeProblem(parameter, value);
  return (
    <label className="stack-stretch-field" title={parameter.description}>
      <span>{parameter.label}</span>
      <input
        type="number"
        aria-label={parameter.label}
        aria-invalid={problem ? true : undefined}
        value={Number.isFinite(numeric) ? numeric : ''}
        min={kind.min}
        max={kind.max}
        step={kind.type === 'int' ? 1 : 0.05}
        disabled={disabled}
        onChange={(event) => onChange(event.target.valueAsNumber)}
      />
      {problem && (
        <small className="stack-rc-astro-field-error">{kind.min} to {kind.max}</small>
      )}
    </label>
  );
}
