import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { StackGpu } from '../api/types';

const STACK_GPU_QUERY_KEY = ['stack-gpu'] as const;

type Pass = 'debayer' | 'detection' | 'resampling' | 'accumulator' | 'integration';

const PASSES: Array<{ key: Pass; label: string; help: string }> = [
  {
    key: 'debayer',
    label: 'Debayering',
    help: "Demosaics a color camera's frames by VNG in the live pass.",
  },
  { key: 'detection', label: 'Star detection', help: "Finds each frame's stars for registration." },
  {
    key: 'resampling',
    label: 'Resampling',
    help: "Aligns each frame onto the reference frame's grid in the live pass.",
  },
  {
    key: 'accumulator',
    label: 'Live stack',
    help: 'Keeps the running stack on the GPU, and normalizes and adds each frame there.',
  },
  {
    key: 'integration',
    label: 'Final pass',
    help: 'Rejects outliers and integrates the frames when the final pass reintegrates them.',
  },
];

const sameGpu = (left: StackGpu, right: StackGpu) =>
  (Object.keys(left) as Array<keyof StackGpu>).every((key) => left[key] === right[key]);

/**
 * How many frames the GPU works on at once. Saved when the field loses focus
 * or Enter is pressed, not on every keystroke.
 */
function ConcurrencyField({
  gpu,
  maximum,
  onChange,
}: {
  gpu: StackGpu;
  maximum: number;
  onChange: (next: StackGpu) => void;
}) {
  const saved = gpu.concurrency;
  const [draft, setDraft] = useState(String(saved));
  useEffect(() => setDraft(String(saved)), [saved]);
  const value = Number(draft);
  const valid = Number.isInteger(value) && value >= 1 && value <= maximum;
  const commit = () => {
    if (valid && value !== saved) onChange({ ...gpu, concurrency: value });
    else if (!valid) setDraft(String(saved));
  };
  return (
    <fieldset className="calibration-settings-group stack-method-group">
      <legend>Frames at once</legend>
      <label className="review-preference">
        <input
          type="number"
          min={1}
          max={maximum}
          step={1}
          value={draft}
          aria-invalid={!valid}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === 'Enter') commit();
          }}
        />
        <span>
          Frames, 1 to {maximum}
          <small>
            How many frames the GPU works on at the same time. The others wait without using GPU
            memory. The default is 3. Lower it if a stack runs out of memory; raising it seldom
            makes a stack faster.
          </small>
        </span>
      </label>
    </fieldset>
  );
}

/**
 * Which of a stack's passes run on the GPU. Server-wide, saved as soon as a
 * choice changes, and applied to the next build. It changes where the
 * arithmetic runs, not what is stacked, so built stacks stay current.
 */
export default function StackGpuSettings() {
  const queryClient = useQueryClient();
  const settings = useQuery({ queryKey: STACK_GPU_QUERY_KEY, queryFn: apiClient.getStackGpu });
  const save = useMutation({
    mutationFn: apiClient.updateStackGpu,
    onSuccess: (updated) => queryClient.setQueryData(STACK_GPU_QUERY_KEY, updated),
  });

  if (settings.isLoading) return null;
  if (settings.isError || !settings.data) {
    return (
      <div className="stack-method-settings">
        <h3>GPU</h3>
        <p className="muted" role="alert">Could not load the GPU settings.</p>
      </div>
    );
  }

  const { gpu, recommended, max_concurrency: maxConcurrency } = settings.data;
  const choose = (next: StackGpu) => save.mutate(next);

  return (
    <div className="stack-method-settings">
      <h3>GPU</h3>
      <p className="muted">
        Stack previews run their heaviest passes on the GPU: Metal on a Mac, Vulkan on Linux and
        Direct3D 12 on Windows. Frames under about a million pixels, and computers without a
        supported GPU, stack on the CPU. The GPU calculates in single precision, so its stacks
        differ slightly from the CPU&apos;s: about 0.005 of the noise on average, and more at a
        pixel where an outlier is rejected on one and kept on the other. A change applies to the
        next build. Built stacks stay current. After a change to Use the GPU, Debayering, Star
        detection, Resampling or Live stack, a refresh that would add frames to a stack restacks
        every frame once.
      </p>
      <fieldset className="calibration-settings-group stack-method-group" disabled={save.isPending}>
        <legend>GPU stacking</legend>
        <label className="review-preference">
          <input
            type="checkbox"
            checked={gpu.enabled}
            onChange={(event) => choose({ ...gpu, enabled: event.target.checked })}
          />
          <span>
            <span className="stack-method-label">
              Use the GPU
              <em className="stack-method-default"> recommended</em>
            </span>
            <small>
              Off keeps the live pass (debayering, star detection, resampling and the running
              stack) and the final pass&apos;s outlier rejection on the CPU. The final pass still
              debayers color camera frames and resamples every frame on the GPU. Artifact
              searches and color preview alignment do too. If a GPU pass fails, the build
              continues on the CPU, and PSF Guard keeps every pass on the CPU until it restarts.
            </small>
          </span>
        </label>
        <div className="stack-method-preset-buttons">
          <button
            type="button"
            disabled={sameGpu(gpu, recommended)}
            onClick={() => choose(recommended)}
          >
            Restore defaults
          </button>
        </div>
      </fieldset>
      <fieldset className="stack-method-choices" disabled={save.isPending || !gpu.enabled}>
        <fieldset className="calibration-settings-group stack-method-group">
          <legend>Passes on the GPU</legend>
          {PASSES.map((pass) => (
            <label key={pass.key} className="review-preference">
              <input
                type="checkbox"
                checked={gpu[pass.key]}
                onChange={(event) => choose({ ...gpu, [pass.key]: event.target.checked })}
              />
              <span>
                {pass.label}
                <small>{pass.help}</small>
              </span>
            </label>
          ))}
        </fieldset>
        <div>
          <ConcurrencyField gpu={gpu} maximum={maxConcurrency} onChange={choose} />
          <fieldset className="calibration-settings-group stack-method-group">
            <legend>Diagnostics</legend>
            <label className="review-preference">
              <input
                type="checkbox"
                checked={gpu.trace}
                onChange={(event) => choose({ ...gpu, trace: event.target.checked })}
              />
              <span>
                Log GPU timings
                <small>
                  Writes each GPU call&apos;s stages and times to the server&apos;s standard error,
                  to show where a stack spends its time. The desktop app does not show this output.
                </small>
              </span>
            </label>
          </fieldset>
        </div>
      </fieldset>
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
    </div>
  );
}
