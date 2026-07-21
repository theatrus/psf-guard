import { useMemo, useState } from 'react';
import type {
  StackColorProcessing,
  StackColorRole,
  StackStretchRequest,
} from '../api/types';
import StackStretchStageEditor from './StackStretchStageEditor';
import { defaultStretchRequest, stretchModelLabels } from './stackStretchModels';
import { defaultColorProcessing } from './stackColorProcessing';

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
  label, roles, applied, disabled, onApply,
}: {
  label: string;
  roles: StackColorRole[];
  applied: StackColorProcessing | null;
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
    setError(null);
    onApply(cloneProcessing(draft));
  };

  return (
    <details className="stack-color-processing" aria-label={`${label} processing stack`}>
      <summary>
        <span>Processing stack</span>
        <small>{roles.map((role) => `${roleLabels[role]} ${draft.input_stretches[role]?.length ?? 0}`).join(' · ')}
          {' · '}RGB {draft.output_stretches.length}</small>
      </summary>
      <div className="stack-color-processing-body">
        <p className="stack-stretch-note">
          Each input is registered and normalized first. Stages then run in order with floating-point
          intermediates; the RGB output stack runs after composition.
        </p>
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
