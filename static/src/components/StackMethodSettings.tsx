import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { StackMethod } from '../api/types';
import { STACK_METHOD_QUERY_KEY, methodName, sameMethod } from '../utils/stackMethod';

type Choice<K extends keyof StackMethod> = {
  value: StackMethod[K];
  label: string;
  help: string;
};

const NORMALIZATION: Choice<'normalization'>[] = [
  {
    value: 'local_background',
    label: 'Local background',
    help:
      'One gain per frame, from star photometry, and a smoothed grid of background ' +
      'offsets. Frames whose sky gradients differ leave no seams at their edges.',
  },
  {
    value: 'global',
    label: 'Global',
    help: 'One gain and one offset per frame. Fast, but a drifting session can show its frame edges.',
  },
  {
    value: 'local',
    label: 'Local',
    help:
      'A gain and an offset per tile. Removes seams too, but a tile of cloud or nebula ' +
      'can push a frame far enough to reject it.',
  },
];

const WEIGHTING: Choice<'weighting'>[] = [
  {
    value: 'noise',
    label: 'By noise',
    help:
      'Each frame counts by the inverse of its noise variance, so frames shot through ' +
      'haze or moonlight count for less. Weights stay between 0.05 and 20.',
  },
  { value: 'equal', label: 'Equal', help: 'Every admitted frame counts the same.' },
];

const REGISTRATION: Choice<'registration'>[] = [
  {
    value: 'quadratic',
    label: 'Quadratic',
    help: "Follows a wide field's lens distortion, which turns against the sky after a meridian flip.",
  },
  { value: 'affine', label: 'Affine', help: 'Adds shear and unequal scale to shift, rotation and scale.' },
  { value: 'similarity', label: 'Similarity', help: 'Shift, rotation and uniform scale only.' },
];

const INTERPOLATION: Choice<'interpolation'>[] = [
  {
    value: 'lanczos3',
    label: 'Lanczos-3',
    help: 'Sharper stars, clamped against ringing at bright edges. Slower.',
  },
  { value: 'bilinear', label: 'Bilinear', help: 'Faster and softer.' },
];

const REFERENCE: Choice<'reference'>[] = [
  {
    value: 'auto',
    label: 'Chosen by Seiza',
    help:
      "Seiza scores every frame by its own stars and sky noise, then takes the flattest " +
      'sky among frames close to the best. Reads each frame once more before stacking; ' +
      'the scores are kept, so a rebuild reads only new frames.',
  },
  {
    value: 'best_graded',
    label: 'Best graded',
    help: "The frame with PSF Guard's best quality grade.",
  },
];

const FINAL_PASS: Choice<'final_pass'>[] = [
  {
    value: 'reintegrate',
    label: 'Reintegrate',
    help:
      'Reads every admitted frame three more times and rejects outliers against all ' +
      'the others, so a satellite trail in the reference or the first frames is removed too.',
  },
  {
    value: 'draft',
    label: 'Draft',
    help:
      'Publishes the live stack as it stands. Much faster, but a trail in one of the ' +
      'first few frames can survive.',
  },
];

function ChoiceGroup<K extends keyof StackMethod>({
  name,
  legend,
  choices,
  method,
  onChange,
}: {
  name: K;
  legend: string;
  choices: Choice<K>[];
  method: StackMethod;
  onChange: (next: StackMethod) => void;
}) {
  return (
    <fieldset className="calibration-settings-group stack-method-group">
      <legend>{legend}</legend>
      {choices.map((choice, index) => (
        <label key={String(choice.value)} className="review-preference">
          <input
            type="radio"
            name={`stack-method-${String(name)}`}
            checked={method[name] === choice.value}
            onChange={() => onChange({ ...method, [name]: choice.value })}
          />
          <span>
            <span className="stack-method-label">
              {choice.label}
              {index === 0 && <em className="stack-method-default"> recommended</em>}
            </span>
            <small>{choice.help}</small>
          </span>
        </label>
      ))}
    </fieldset>
  );
}

/**
 * The method every stack preview integrates its frames with. Server-wide,
 * saved as soon as a choice changes, and applied to the next build,
 * including automatic refreshes. Built stacks keep the method they record.
 */
export default function StackMethodSettings() {
  const queryClient = useQueryClient();
  const settings = useQuery({ queryKey: STACK_METHOD_QUERY_KEY, queryFn: apiClient.getStackMethod });
  const save = useMutation({
    mutationFn: apiClient.updateStackMethod,
    onSuccess: (updated) => queryClient.setQueryData(STACK_METHOD_QUERY_KEY, updated),
  });

  if (settings.isLoading) return null;
  if (settings.isError || !settings.data) {
    return (
      <div className="stack-method-settings">
        <h3>Stacking method</h3>
        <p className="muted" role="alert">Could not load the stacking method.</p>
      </div>
    );
  }

  const current = settings.data;
  const method = current.method;
  const choose = (next: StackMethod) => save.mutate(next);
  const presets: Array<{ label: string; method: StackMethod; help: string }> = [
    {
      label: 'Recommended',
      method: current.recommended,
      help: "Seiza's defaults, which match or beat WBPP on sharpness and signal-to-noise.",
    },
    {
      label: 'Draft',
      method: { ...current.recommended, final_pass: 'draft' },
      help: 'The recommended method without the final pass, for a quick look.',
    },
    {
      label: 'Classic',
      method: current.classic,
      help: 'The method PSF Guard used before these choices existed.',
    },
  ];
  const name = methodName(method, current);

  return (
    <div className="stack-method-settings">
      <h3>Stacking method</h3>
      <p className="muted">
        How stack previews integrate their frames, on every database on this server. A change
        applies to the next build, automatic refreshes included. Stacks built before it show as
        out of date.
      </p>
      <fieldset className="calibration-settings-group stack-method-presets" disabled={save.isPending}>
        <legend>
          Preset <span className="stack-method-current">Now: {name}</span>
        </legend>
        <div className="stack-method-preset-buttons">
          {presets.map((preset) => (
            <button
              key={preset.label}
              type="button"
              aria-pressed={sameMethod(method, preset.method)}
              title={preset.help}
              onClick={() => choose(preset.method)}
            >
              {preset.label}
            </button>
          ))}
        </div>
      </fieldset>
      <fieldset className="stack-method-choices" disabled={save.isPending}>
        <ChoiceGroup name="final_pass" legend="Final pass" choices={FINAL_PASS} method={method} onChange={choose} />
        <ChoiceGroup name="normalization" legend="Normalization" choices={NORMALIZATION} method={method} onChange={choose} />
        <ChoiceGroup name="weighting" legend="Frame weights" choices={WEIGHTING} method={method} onChange={choose} />
        <ChoiceGroup name="reference" legend="Reference frame" choices={REFERENCE} method={method} onChange={choose} />
        <ChoiceGroup name="registration" legend="Registration" choices={REGISTRATION} method={method} onChange={choose} />
        <ChoiceGroup name="interpolation" legend="Resampling" choices={INTERPOLATION} method={method} onChange={choose} />
        <fieldset className="calibration-settings-group stack-method-group">
          <legend>Color cameras</legend>
          <label className="review-preference">
            <input
              type="checkbox"
              checked={method.bayer_drizzle}
              onChange={(event) => choose({ ...method, bayer_drizzle: event.target.checked })}
            />
            <span>
              Bayer drizzle
              <small>
                Integrates each frame&apos;s photosites in their own color instead of
                demosaicing, so nothing is interpolated. Pays only when frames are dithered by
                several pixels; with little movement it lowers signal-to-noise. A drizzled
                stack skips the final pass for now.
              </small>
            </span>
          </label>
        </fieldset>
      </fieldset>
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
    </div>
  );
}
