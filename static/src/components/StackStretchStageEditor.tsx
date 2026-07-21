import type {
  StackStretchColorStrategy,
  StackStretchModel,
  StackStretchRequest,
} from '../api/types';
import {
  defaultStretchModel,
  stretchModelLabels,
  type StretchModelType,
} from './stackStretchModels';

function NumberField({
  controlLabel, label, value, onChange, min, max, step = 'any', disabled,
}: {
  controlLabel: string;
  label: string;
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number | 'any';
  disabled?: boolean;
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
        disabled={disabled}
        onChange={(event) => onChange(event.target.valueAsNumber)}
      />
    </label>
  );
}

export default function StackStretchStageEditor({
  label, channels, request, disabled = false, onChange,
}: {
  label: string;
  channels: 1 | 3;
  request: StackStretchRequest;
  disabled?: boolean;
  onChange: (request: StackStretchRequest) => void;
}) {
  const model = request.model;
  const updateModel = (change: Partial<StackStretchModel>) =>
    onChange({ ...request, model: { ...model, ...change } as StackStretchModel });
  const updateStrategy = (color_strategy: StackStretchColorStrategy) =>
    onChange({ ...request, color_strategy });

  return (
    <div className="stack-stretch-fields">
      <label className="stack-stretch-field stack-stretch-model">
        <span>Model</span>
        <select
          aria-label={`${label} stretch model`}
          value={model.type}
          disabled={disabled}
          onChange={(event) => onChange({
            ...request,
            model: defaultStretchModel(event.target.value as StretchModelType),
          })}
        >
          {Object.entries(stretchModelLabels).map(([value, optionLabel]) => (
            <option key={value} value={value}>{optionLabel}</option>
          ))}
        </select>
      </label>
      {channels === 3 && (
        <label className="stack-stretch-field stack-stretch-strategy">
          <span>Color</span>
          <select
            aria-label={`${label} stretch color strategy`}
            value={request.color_strategy}
            disabled={disabled}
            onChange={(event) => updateStrategy(event.target.value as StackStretchColorStrategy)}
          >
            <option value="linked">Linked</option>
            <option value="luminance-preserving">Preserve luminance color</option>
            <option value="unlinked">Unlinked channels</option>
          </select>
        </label>
      )}
      {model.type === 'linear' && <>
        <NumberField controlLabel={label} label="Black" value={model.black} disabled={disabled}
          onChange={(black) => updateModel({ black })} />
        <NumberField controlLabel={label} label="White" value={model.white} disabled={disabled}
          onChange={(white) => updateModel({ white })} />
      </>}
      {model.type === 'asinh' && <>
        <NumberField controlLabel={label} label="Black" value={model.black} disabled={disabled}
          onChange={(black) => updateModel({ black })} />
        <NumberField controlLabel={label} label="White" value={model.white} disabled={disabled}
          onChange={(white) => updateModel({ white })} />
        <NumberField controlLabel={label} label="Strength" value={model.strength} min={0.01}
          step={0.5} disabled={disabled} onChange={(strength) => updateModel({ strength })} />
      </>}
      {model.type === 'percentile-asinh' && <>
        <NumberField controlLabel={label} label="Black percentile" value={model.black_percentile}
          min={0} max={0.999} step={0.001} disabled={disabled}
          onChange={(black_percentile) => updateModel({ black_percentile })} />
        <NumberField controlLabel={label} label="White percentile" value={model.white_percentile}
          min={0.001} max={1} step={0.001} disabled={disabled}
          onChange={(white_percentile) => updateModel({ white_percentile })} />
        <NumberField controlLabel={label} label="Strength" value={model.strength} min={0.01}
          step={0.5} disabled={disabled} onChange={(strength) => updateModel({ strength })} />
      </>}
      {model.type === 'mtf' && <>
        <NumberField controlLabel={label} label="Shadows" value={model.shadows} disabled={disabled}
          onChange={(shadows) => updateModel({ shadows })} />
        <NumberField controlLabel={label} label="Midtone" value={model.midtone} min={0.001}
          max={0.999} step={0.01} disabled={disabled}
          onChange={(midtone) => updateModel({ midtone })} />
        <NumberField controlLabel={label} label="Highlights" value={model.highlights}
          disabled={disabled} onChange={(highlights) => updateModel({ highlights })} />
      </>}
      {model.type === 'ghs' && <>
        <NumberField controlLabel={label} label="Stretch factor" value={model.stretch_factor}
          min={0} max={20} step={0.25} disabled={disabled}
          onChange={(stretch_factor) => updateModel({ stretch_factor })} />
        <NumberField controlLabel={label} label="Local intensity" value={model.local_intensity}
          min={-5} max={15} step={0.25} disabled={disabled}
          onChange={(local_intensity) => updateModel({ local_intensity })} />
        <NumberField controlLabel={label} label="Symmetry" value={model.symmetry_point}
          min={0} max={1} step={0.01} disabled={disabled}
          onChange={(symmetry_point) => updateModel({ symmetry_point })} />
        <NumberField controlLabel={label} label="Protect shadows" value={model.protect_shadows}
          min={0} max={model.symmetry_point} step={0.01} disabled={disabled}
          onChange={(protect_shadows) => updateModel({ protect_shadows })} />
        <NumberField controlLabel={label} label="Protect highlights" value={model.protect_highlights}
          min={model.symmetry_point} max={1} step={0.01} disabled={disabled}
          onChange={(protect_highlights) => updateModel({ protect_highlights })} />
        <NumberField controlLabel={label} label="Black" value={model.black} disabled={disabled}
          onChange={(black) => updateModel({ black })} />
        <NumberField controlLabel={label} label="White" value={model.white} disabled={disabled}
          onChange={(white) => updateModel({ white })} />
      </>}
      {model.type === 'auto-mtf' && <>
        <NumberField controlLabel={label} label="Target median" value={model.target_median}
          min={0.001} max={0.999} step={0.01} disabled={disabled}
          onChange={(target_median) => updateModel({ target_median })} />
        <NumberField controlLabel={label} label="Shadows clip" value={model.shadows_clip}
          max={0} step={0.1} disabled={disabled}
          onChange={(shadows_clip) => updateModel({ shadows_clip })} />
      </>}
    </div>
  );
}
