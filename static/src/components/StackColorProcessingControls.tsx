import { useMemo, useState } from 'react';
import type {
  StackBackgroundConfig,
  StackBackgroundExtraction,
  StackBackgroundFit,
  StackColorProcessing,
  StackColorRole,
  StackStretchRequest,
} from '../api/types';
import StackStretchStageEditor from './StackStretchStageEditor';
import { defaultStretchRequest, stretchModelLabels } from './stackStretchModels';
import {
  defaultBackgroundExtraction,
  defaultColorProcessing,
} from './stackColorProcessing';

const roleLabels: Record<StackColorRole, string> = {
  luminance: 'L', red: 'R', green: 'G', blue: 'B', ha: 'Hα', oiii: 'OIII', sii: 'SII',
};

function cloneProcessing(processing: StackColorProcessing): StackColorProcessing {
  return JSON.parse(JSON.stringify(processing)) as StackColorProcessing;
}

function hasInvalidNumbers(processing: StackColorProcessing): boolean {
  const stages = [
    ...Object.values(processing.input_stretches).flatMap((value) => value ?? []),
    ...processing.output_stretches,
  ];
  return stages.some((stage) => Object.values(stage.model).some(
    (value) => typeof value === 'number' && !Number.isFinite(value)
  ));
}

function validateBackground(extraction: StackBackgroundExtraction | null): string | null {
  if (!extraction) return null;
  const { config } = extraction;
  if (!Number.isInteger(config.model.degree) || config.model.degree < 0 || config.model.degree > 4) {
    return 'Background polynomial degree must be an integer from 0 to 4';
  }
  if (!Number.isFinite(config.model.ridge) || config.model.ridge < 0) {
    return 'Background ridge must be finite and non-negative';
  }
  if (!Number.isInteger(config.samples_per_axis) ||
      config.samples_per_axis < 3 || config.samples_per_axis > 512) {
    return 'Background samples per axis must be an integer from 3 to 512';
  }
  if (config.sample_radius !== null &&
      (!Number.isInteger(config.sample_radius) || config.sample_radius < 1)) {
    return 'Background sample radius must be blank for automatic or a positive integer';
  }
  if (!Number.isInteger(config.search_steps) || config.search_steps < 0 ||
      config.search_steps > 64) {
    return 'Background search steps must be an integer from 0 to 64';
  }
  if (!Number.isFinite(config.sample_rejection_sigma) || config.sample_rejection_sigma <= 0 ||
      !Number.isFinite(config.fit_rejection_sigma) || config.fit_rejection_sigma <= 0) {
    return 'Background rejection sigmas must be finite and greater than zero';
  }
  if (!Number.isInteger(config.fit_rejection_iterations) ||
      config.fit_rejection_iterations < 0 || config.fit_rejection_iterations > 16) {
    return 'Background rejection passes must be an integer from 0 to 16';
  }
  if (!Number.isFinite(config.border_fraction) ||
      config.border_fraction < 0 || config.border_fraction >= 0.45) {
    return 'Background border fraction must be in the range 0 to 0.45';
  }
  return null;
}

function BackgroundNumberField({
  label, value, min, max, step = 'any', disabled, onChange,
}: {
  label: string;
  value: number | null;
  min?: number;
  max?: number;
  step?: number | 'any';
  disabled: boolean;
  onChange: (value: number | null) => void;
}) {
  return (
    <label className="stack-stretch-field">
      <span>{label}</span>
      <input
        type="number"
        aria-label={`Background ${label}`}
        value={value ?? ''}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        placeholder={value === null ? 'Auto' : undefined}
        onChange={(event) => onChange(
          event.target.value === '' ? null : event.target.valueAsNumber
        )}
      />
    </label>
  );
}

function BackgroundControls({
  extraction, backgrounds, disabled, onChange,
}: {
  extraction: StackBackgroundExtraction | null;
  backgrounds: Partial<Record<StackColorRole, StackBackgroundFit>>;
  disabled: boolean;
  onChange: (extraction: StackBackgroundExtraction | null) => void;
}) {
  const updateConfig = (change: Partial<StackBackgroundConfig>) => {
    if (!extraction) return;
    onChange({ ...extraction, config: { ...extraction.config, ...change } });
  };
  const updateModel = (change: Partial<StackBackgroundConfig['model']>) => {
    if (!extraction) return;
    updateConfig({ model: { ...extraction.config.model, ...change } });
  };
  const fits = Object.entries(backgrounds) as Array<[StackColorRole, StackBackgroundFit]>;

  return (
    <section className="stack-background-controls" aria-label="Background extraction">
      <header>
        <label>
          <input
            type="checkbox"
            checked={extraction !== null}
            disabled={disabled}
            onChange={(event) => onChange(
              event.target.checked ? defaultBackgroundExtraction() : null
            )}
          />
          <strong>Background extraction</strong>
        </label>
        <span>{extraction ? 'Before registration' : 'Disabled'}</span>
      </header>
      <p>
        Fit and correct each linear channel independently before alignment. Subtraction removes
        additive gradients while preserving that channel&apos;s robust sky level.
      </p>
      {extraction && (
        <div className="stack-background-fields">
          <label className="stack-stretch-field">
            <span>Correction</span>
            <select
              aria-label="Background correction mode"
              value={extraction.correction_mode}
              disabled={disabled}
              onChange={(event) => onChange({
                ...extraction,
                correction_mode: event.target.value as 'subtract' | 'divide',
              })}
            >
              <option value="subtract">Subtract</option>
              <option value="divide">Divide</option>
            </select>
          </label>
          <BackgroundNumberField label="Polynomial degree" value={extraction.config.model.degree}
            min={0} max={4} step={1} disabled={disabled}
            onChange={(degree) => updateModel({ degree: degree ?? 0 })} />
          <BackgroundNumberField label="Samples per axis" value={extraction.config.samples_per_axis}
            min={3} max={512} step={1} disabled={disabled}
            onChange={(samples_per_axis) => updateConfig({ samples_per_axis: samples_per_axis ?? 3 })} />
          <BackgroundNumberField label="Sample radius" value={extraction.config.sample_radius}
            min={1} step={1} disabled={disabled}
            onChange={(sample_radius) => updateConfig({ sample_radius })} />
          <BackgroundNumberField label="Search steps" value={extraction.config.search_steps}
            min={0} max={64} step={1} disabled={disabled}
            onChange={(search_steps) => updateConfig({ search_steps: search_steps ?? 0 })} />
          <BackgroundNumberField label="Noise sigma" value={extraction.config.sample_rejection_sigma}
            min={0.1} step={0.1} disabled={disabled}
            onChange={(sample_rejection_sigma) => updateConfig({
              sample_rejection_sigma: sample_rejection_sigma ?? 0,
            })} />
          <BackgroundNumberField label="Fit sigma" value={extraction.config.fit_rejection_sigma}
            min={0.1} step={0.1} disabled={disabled}
            onChange={(fit_rejection_sigma) => updateConfig({
              fit_rejection_sigma: fit_rejection_sigma ?? 0,
            })} />
          <BackgroundNumberField label="Rejection passes"
            value={extraction.config.fit_rejection_iterations}
            min={0} max={16} step={1} disabled={disabled}
            onChange={(fit_rejection_iterations) => updateConfig({
              fit_rejection_iterations: fit_rejection_iterations ?? 0,
            })} />
          <BackgroundNumberField label="Border fraction" value={extraction.config.border_fraction}
            min={0} max={0.449} step={0.01} disabled={disabled}
            onChange={(border_fraction) => updateConfig({ border_fraction: border_fraction ?? 0 })} />
          <BackgroundNumberField label="Ridge" value={extraction.config.model.ridge}
            min={0} step="any" disabled={disabled}
            onChange={(ridge) => updateModel({ ridge: ridge ?? 0 })} />
        </div>
      )}
      {fits.length > 0 && (
        <div className="stack-background-diagnostics" aria-label="Background fit diagnostics">
          {fits.map(([role, fit]) => (
            <span key={role}>
              <strong>{roleLabels[role]}</strong>
              {fit.diagnostics.accepted_samples}/{fit.diagnostics.candidate_samples} samples
              <small>radius {fit.diagnostics.sample_radius}</small>
            </span>
          ))}
        </div>
      )}
    </section>
  );
}

function StageLane({
  label, stages, channels, disabled, onChange,
}: {
  label: string;
  stages: StackStretchRequest[];
  channels: 1 | 3;
  disabled: boolean;
  onChange: (stages: StackStretchRequest[]) => void;
}) {
  const replace = (index: number, stage: StackStretchRequest) => {
    const next = [...stages];
    next[index] = stage;
    onChange(next);
  };
  const move = (index: number, direction: -1 | 1) => {
    const destination = index + direction;
    if (destination < 0 || destination >= stages.length) return;
    const next = [...stages];
    [next[index], next[destination]] = [next[destination], next[index]];
    onChange(next);
  };

  return (
    <section className="stack-color-stage-lane" aria-label={`${label} stretch stack`}>
      <header>
        <strong>{label}</strong>
        <span>{stages.length} stage{stages.length === 1 ? '' : 's'}</span>
      </header>
      {stages.length === 0 && (
        <p>Normalized only — no stretch stages.</p>
      )}
      {stages.map((stage, index) => (
        <div className="stack-color-stage" key={`${index}:${stage.model.type}`}>
          <div className="stack-color-stage-heading">
            <span>Stage {index + 1} · {stretchModelLabels[stage.model.type]}</span>
            <div>
              <button type="button" disabled={disabled || index === 0}
                aria-label={`Move ${label} stage ${index + 1} earlier`}
                onClick={() => move(index, -1)}>↑</button>
              <button type="button" disabled={disabled || index === stages.length - 1}
                aria-label={`Move ${label} stage ${index + 1} later`}
                onClick={() => move(index, 1)}>↓</button>
              <button type="button" disabled={disabled}
                aria-label={`Remove ${label} stage ${index + 1}`}
                onClick={() => onChange(stages.filter((_, candidate) => candidate !== index))}>
                Remove
              </button>
            </div>
          </div>
          <StackStretchStageEditor
            label={`${label} stage ${index + 1}`}
            channels={channels}
            request={stage}
            disabled={disabled}
            onChange={(next) => replace(index, next)}
          />
        </div>
      ))}
      <button className="stack-color-add-stage" type="button" disabled={disabled}
        onClick={() => onChange([...stages, defaultStretchRequest('auto-mtf')])}>
        Add stage
      </button>
    </section>
  );
}

export default function StackColorProcessingControls({
  label, roles, applied, backgrounds, disabled, onApply,
}: {
  label: string;
  roles: StackColorRole[];
  applied: StackColorProcessing | null;
  backgrounds: Partial<Record<StackColorRole, StackBackgroundFit>>;
  disabled: boolean;
  onApply: (processing: StackColorProcessing) => void;
}) {
  const defaults = useMemo(() => defaultColorProcessing(roles), [roles]);
  const baseline = applied ?? defaults;
  const [draft, setDraft] = useState<StackColorProcessing>(() => cloneProcessing(baseline));
  const [error, setError] = useState<string | null>(null);
  const changed = JSON.stringify(draft) !== JSON.stringify(baseline);

  const updateInput = (role: StackColorRole, stages: StackStretchRequest[]) => setDraft((current) => ({
    ...current,
    input_stretches: { ...current.input_stretches, [role]: stages },
  }));
  const submit = () => {
    if (hasInvalidNumbers(draft)) {
      setError('Enter a finite value for every stretch parameter');
      return;
    }
    const backgroundError = validateBackground(draft.background_extraction);
    if (backgroundError) {
      setError(backgroundError);
      return;
    }
    setError(null);
    onApply(cloneProcessing(draft));
  };

  return (
    <details className="stack-color-processing" aria-label={`${label} processing stack`}>
      <summary>
        <span>Processing stack</span>
        <small>BG {draft.background_extraction ? draft.background_extraction.correction_mode : 'off'}
          {' · '}{roles.map((role) => `${roleLabels[role]} ${draft.input_stretches[role]?.length ?? 0}`).join(' · ')}
          {' · '}RGB {draft.output_stretches.length}</small>
      </summary>
      <div className="stack-color-processing-body">
        <p className="stack-stretch-note">
          Background correction runs on each linear input before registration. Normalization and
          input stretches follow alignment; the RGB output stack runs after composition.
        </p>
        <BackgroundControls
          extraction={draft.background_extraction}
          backgrounds={backgrounds}
          disabled={disabled}
          onChange={(background_extraction) => setDraft((current) => ({
            ...current, background_extraction,
          }))}
        />
        <div className="stack-color-stage-grid">
          {roles.map((role) => (
            <StageLane
              key={role}
              label={`${roleLabels[role]} input`}
              stages={draft.input_stretches[role] ?? []}
              channels={1}
              disabled={disabled}
              onChange={(stages) => updateInput(role, stages)}
            />
          ))}
          <StageLane
            label="RGB output"
            stages={draft.output_stretches}
            channels={3}
            disabled={disabled}
            onChange={(output_stretches) => setDraft((current) => ({
              ...current, output_stretches,
            }))}
          />
        </div>
        {error && <div className="stack-stretch-error" role="alert">{error}</div>}
        <div className="stack-stretch-actions">
          <button type="button" disabled={disabled || !changed} onClick={submit}>
            Apply processing stack
          </button>
          <button type="button" disabled={disabled || !changed}
            onClick={() => { setDraft(cloneProcessing(baseline)); setError(null); }}>
            Revert edits
          </button>
          <button type="button" disabled={disabled}
            onClick={() => { setDraft(cloneProcessing(defaults)); setError(null); }}>
            Reset defaults
          </button>
        </div>
      </div>
    </details>
  );
}
