import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ExternalMasterPolicy } from '../api/types';

const EXTERNAL_MASTER_OPTIONS: ReadonlyArray<{
  value: ExternalMasterPolicy;
  label: string;
  hint: string;
}> = [
  {
    value: 'prefer',
    label: 'Use them whenever one matches',
    hint: 'The nearest matching external master is used as-is, even when raw frames match too.',
  },
  {
    value: 'fallback',
    label: 'Only when raw frames cannot build one',
    hint: 'PSF Guard integrates its own master from raw frames when enough match.',
  },
  {
    value: 'ignore',
    label: 'Never',
    hint: 'External masters stay in the library but never calibrate a stack.',
  },
];

/**
 * The calibration matching knob: how far a flat's rotator angle may sit from
 * the light it corrects. Server-wide — it is a property of the rig, not of a
 * database — persisted in the registry and applied to the next stack
 * immediately, no restart.
 */
export default function CalibrationMatchingSettings() {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: ['calibration-settings'],
    queryFn: apiClient.getCalibrationSettings,
  });

  // The field edits as text so a half-typed "1." is not fought by the parser;
  // it commits on Save.
  const [draft, setDraft] = useState<string>('');
  const [policy, setPolicy] = useState<ExternalMasterPolicy>('prefer');
  useEffect(() => {
    if (settings.data) {
      setDraft(
        settings.data.rotation_tolerance_deg === null
          ? ''
          : String(settings.data.rotation_tolerance_deg)
      );
      setPolicy(settings.data.external_masters);
    }
  }, [settings.data]);

  const save = useMutation({
    mutationFn: (update: {
      rotation_tolerance_deg: number | null;
      external_masters: ExternalMasterPolicy;
    }) => apiClient.updateCalibrationSettings(update),
    onSuccess: (updated) => {
      queryClient.setQueryData(['calibration-settings'], updated);
    },
  });

  if (settings.isLoading) return null;
  if (settings.isError) {
    return (
      <div className="calibration-matching-settings">
        <h3>Calibration matching</h3>
        <p className="muted">Could not load calibration settings.</p>
      </div>
    );
  }

  const current = settings.data!;
  const parsed = draft.trim() === '' ? null : Number(draft);
  const invalid =
    parsed !== null && (!Number.isFinite(parsed) || parsed < 0 || parsed > 180);
  const dirty =
    (parsed === null) !== (current.rotation_tolerance_deg === null) ||
    (parsed !== null && parsed !== current.rotation_tolerance_deg) ||
    policy !== current.external_masters;
  const policyHint =
    EXTERNAL_MASTER_OPTIONS.find((option) => option.value === policy)?.hint ?? '';

  return (
    <div className="calibration-matching-settings">
      <h3>Calibration matching</h3>
      <label className="review-preference">
        <span>
          Rotation tolerance (degrees)
          <small>
            How far a flat's rotator angle may sit from the light it corrects.
            Wider accepts a rotator that re-homes loosely between nights;
            narrower keeps dust motes pinned. Empty uses the default of{' '}
            {current.default_rotation_tolerance_deg}°. Applies to every
            database on this server, starting with the next stack.
          </small>
        </span>
        <input
          type="number"
          min={0}
          max={180}
          step={0.1}
          value={draft}
          placeholder={String(current.default_rotation_tolerance_deg)}
          aria-label="Rotation tolerance in degrees"
          aria-invalid={invalid}
          onChange={(event) => setDraft(event.target.value)}
        />
      </label>
      {invalid && (
        <p className="error-text">Enter a value between 0 and 180 degrees.</p>
      )}
      <label className="review-preference">
        <span>
          Masters from other software
          <small>
            A master dark, bias, or flat integrated by PixInsight, Siril, or
            another tool is matched on what its header kept — such files
            usually drop gain, offset, and temperature — and used as-is rather
            than integrated again. {policyHint}
          </small>
        </span>
        <select
          value={policy}
          aria-label="Masters from other software"
          onChange={(event) => setPolicy(event.target.value as ExternalMasterPolicy)}
        >
          {EXTERNAL_MASTER_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      </label>
      {save.isError && (
        <p className="error-text">{(save.error as Error).message}</p>
      )}
      <button
        type="button"
        disabled={invalid || !dirty || save.isPending}
        onClick={() =>
          save.mutate({ rotation_tolerance_deg: parsed, external_masters: policy })
        }
      >
        {save.isPending ? 'Saving…' : 'Save'}
      </button>
    </div>
  );
}
