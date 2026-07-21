import { useState } from 'react';
import type {
  StackStretchColorStrategy,
  StackStretchModel,
  StackStretchPreview,
  StackStretchRequest,
} from '../api/types';

type StretchModelType = StackStretchModel['type'];

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

const modelLabels: Record<StretchModelType, string> = {
  identity: 'Identity',
  linear: 'Linear range',
  asinh: 'Asinh',
  'percentile-asinh': 'Percentile asinh',
  mtf: 'MTF',
  ghs: 'Generalized hyperbolic',
  'auto-mtf': 'Auto MTF',
};

function defaultModel(type: StretchModelType): StackStretchModel {
  switch (type) {
    case 'identity':
      return { type };
    case 'linear':
      return { type, black: 0, white: 1 };
    case 'asinh':
      return { type, black: 0, white: 1, strength: 10 };
    case 'percentile-asinh':
      return { type, black_percentile: 0.01, white_percentile: 0.995, strength: 10 };
    case 'mtf':
      return { type, shadows: 0, midtone: 0.5, highlights: 1 };
    case 'ghs':
      return {
        type,
        stretch_factor: 1,
        local_intensity: 0,
        symmetry_point: 0,
        protect_shadows: 0,
        protect_highlights: 1,
        black: 0,
        white: 1,
      };
    case 'auto-mtf':
      return { type, target_median: 0.2, shadows_clip: -2.8 };
  }
}

function NumberField({
  controlLabel,
  label,
  value,
  onChange,
  min,
  max,
  step = 'any',
}: {
  controlLabel: string;
  label: string;
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number | 'any';
}) {
  return (
    <label className="stack-stretch-field">
      <span>{label}</span>
      <input
        type="number"
        aria-label={`${controlLabel} ${label}`}
        value={value}
        min={min}
        max={max}
        step={step}
        onChange={(event) => onChange(event.target.valueAsNumber)}
      />
    </label>
  );
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
  const initialType: StretchModelType = displayReferred ? 'identity' : 'auto-mtf';
  const [model, setModel] = useState<StackStretchModel>(() => defaultModel(initialType));
  const [colorStrategy, setColorStrategy] = useState<StackStretchColorStrategy>('linked');
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const update = <T extends StackStretchModel>(change: Partial<T>) => {
    setModel((current) => ({ ...current, ...change }) as StackStretchModel);
  };
  const revert = () => {
    setModel(defaultModel(initialType));
    setColorStrategy('linked');
    setError(null);
    onRevert();
  };
  const submit = async () => {
    if (Object.values(model).some((value) =>
      typeof value === 'number' && !Number.isFinite(value))) {
      setError('Enter a finite value for every stretch parameter');
      return;
    }
    setPending(true);
    setError(null);
    try {
      onApplied(await apply({ model, color_strategy: colorStrategy }));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Stretch rendering failed');
    } finally {
      setPending(false);
    }
  };

  return (
    <details className="stack-stretch-controls">
      <summary>
        <span>Display stretch</span>
        <small>{applied ? `${modelLabels[applied.config.model.type]} applied` : 'Default'}</small>
      </summary>
      <div className="stack-stretch-body">
        <div className="stack-stretch-fields">
          <label className="stack-stretch-field stack-stretch-model">
            <span>Model</span>
            <select
              aria-label={`${label} stretch model`}
              value={model.type}
              disabled={disabled || pending}
              onChange={(event) => setModel(defaultModel(event.target.value as StretchModelType))}
            >
              {Object.entries(modelLabels).map(([value, optionLabel]) => (
                <option key={value} value={value}>{optionLabel}</option>
              ))}
            </select>
          </label>
          {channels === 3 && (
            <label className="stack-stretch-field stack-stretch-strategy">
              <span>Color</span>
              <select
                aria-label={`${label} stretch color strategy`}
                value={colorStrategy}
                disabled={disabled || pending}
                onChange={(event) =>
                  setColorStrategy(event.target.value as StackStretchColorStrategy)}
              >
                <option value="linked">Linked</option>
                <option value="luminance-preserving">Preserve luminance color</option>
                <option value="unlinked">Unlinked channels</option>
              </select>
            </label>
          )}
          {model.type === 'linear' && (
            <>
              <NumberField controlLabel={label} label="Black" value={model.black}
                onChange={(black) => update({ black })} />
              <NumberField controlLabel={label} label="White" value={model.white}
                onChange={(white) => update({ white })} />
            </>
          )}
          {model.type === 'asinh' && (
            <>
              <NumberField controlLabel={label} label="Black" value={model.black}
                onChange={(black) => update({ black })} />
              <NumberField controlLabel={label} label="White" value={model.white}
                onChange={(white) => update({ white })} />
              <NumberField controlLabel={label} label="Strength" value={model.strength}
                min={0.01} step={0.5} onChange={(strength) => update({ strength })} />
            </>
          )}
          {model.type === 'percentile-asinh' && (
            <>
              <NumberField controlLabel={label} label="Black percentile"
                value={model.black_percentile} min={0} max={0.999} step={0.001}
                onChange={(black_percentile) => update({ black_percentile })} />
              <NumberField controlLabel={label} label="White percentile"
                value={model.white_percentile} min={0.001} max={1} step={0.001}
                onChange={(white_percentile) => update({ white_percentile })} />
              <NumberField controlLabel={label} label="Strength" value={model.strength}
                min={0.01} step={0.5} onChange={(strength) => update({ strength })} />
            </>
          )}
          {model.type === 'mtf' && (
            <>
              <NumberField controlLabel={label} label="Shadows" value={model.shadows}
                onChange={(shadows) => update({ shadows })} />
              <NumberField controlLabel={label} label="Midtone" value={model.midtone}
                min={0.001} max={0.999} step={0.01}
                onChange={(midtone) => update({ midtone })} />
              <NumberField controlLabel={label} label="Highlights" value={model.highlights}
                onChange={(highlights) => update({ highlights })} />
            </>
          )}
          {model.type === 'ghs' && (
            <>
              <NumberField controlLabel={label} label="Stretch factor" value={model.stretch_factor}
                min={0} max={20} step={0.25}
                onChange={(stretch_factor) => update({ stretch_factor })} />
              <NumberField controlLabel={label} label="Local intensity" value={model.local_intensity}
                min={-5} max={15} step={0.25}
                onChange={(local_intensity) => update({ local_intensity })} />
              <NumberField controlLabel={label} label="Symmetry" value={model.symmetry_point}
                min={0} max={1} step={0.01}
                onChange={(symmetry_point) => update({ symmetry_point })} />
              <NumberField controlLabel={label} label="Protect shadows" value={model.protect_shadows}
                min={0} max={model.symmetry_point} step={0.01}
                onChange={(protect_shadows) => update({ protect_shadows })} />
              <NumberField controlLabel={label} label="Protect highlights" value={model.protect_highlights}
                min={model.symmetry_point} max={1} step={0.01}
                onChange={(protect_highlights) => update({ protect_highlights })} />
              <NumberField controlLabel={label} label="Black" value={model.black}
                onChange={(black) => update({ black })} />
              <NumberField controlLabel={label} label="White" value={model.white}
                onChange={(white) => update({ white })} />
            </>
          )}
          {model.type === 'auto-mtf' && (
            <>
              <NumberField controlLabel={label} label="Target median" value={model.target_median}
                min={0.001} max={0.999} step={0.01}
                onChange={(target_median) => update({ target_median })} />
              <NumberField controlLabel={label} label="Shadows clip" value={model.shadows_clip}
                max={0} step={0.1} onChange={(shadows_clip) => update({ shadows_clip })} />
            </>
          )}
        </div>
        {(model.type === 'linear' || model.type === 'asinh' || model.type === 'mtf' || model.type === 'ghs') && (
          <p className="stack-stretch-note">Explicit points use normalized 0–1 display units.</p>
        )}
        {applied && (
          <div className="stack-stretch-stats">
            Source range {applied.linked_statistics.min.toPrecision(4)}–{applied.linked_statistics.max.toPrecision(4)}
            {' · '}median {applied.linked_statistics.median.toPrecision(4)}
            {applied.input_range && (
              <>
                {' · '}display normalization {applied.input_range.black.toPrecision(4)}–
                {applied.input_range.white.toPrecision(4)}
              </>
            )}
          </div>
        )}
        {error && <div className="stack-stretch-error" role="alert">{error}</div>}
        <div className="stack-stretch-actions">
          <button type="button" disabled={disabled || pending} onClick={submit}>
            {pending ? 'Applying…' : 'Apply stretch'}
          </button>
          <button type="button" disabled={disabled || pending || !applied} onClick={revert}>
            Revert stretch
          </button>
        </div>
      </div>
    </details>
  );
}
