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
 * Server-wide matching and master-building settings, persisted in the
 * registry and applied to the next stack without a restart.
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
  const [flatStarMasking, setFlatStarMasking] = useState(false);
  const [reachDraft, setReachDraft] = useState('');
  const [completeDraft, setCompleteDraft] = useState('');
  useEffect(() => {
    if (settings.data) {
      setDraft(
        settings.data.rotation_tolerance_deg === null
          ? ''
          : String(settings.data.rotation_tolerance_deg)
      );
      setPolicy(settings.data.external_masters);
      setFlatStarMasking(settings.data.flat_star_masking ?? false);
      setReachDraft(settings.data.dark_reach_days == null ? '' : String(settings.data.dark_reach_days));
      setCompleteDraft(settings.data.complete_dark_frames == null ? '' : String(settings.data.complete_dark_frames));
    }
  }, [settings.data]);

  const save = useMutation({
    mutationFn: (update: {
      rotation_tolerance_deg: number | null;
      external_masters: ExternalMasterPolicy;
      flat_star_masking: boolean;
      dark_reach_days: number | null;
      complete_dark_frames: number | null;
    }) => apiClient.updateCalibrationSettings(update),
    onSuccess: (updated) => {
      queryClient.setQueryData(['calibration-settings'], updated);
    },
  });

  if (settings.isLoading) return null;
  if (settings.isError) {
    return (
      <div className="calibration-matching-settings">
        <h3>Calibration</h3>
        <p className="muted" role="alert">Could not load calibration settings.</p>
      </div>
    );
  }

  const current = settings.data!;
  const parsed = draft.trim() === '' ? null : Number(draft);
  const invalid =
    parsed !== null && (!Number.isFinite(parsed) || parsed < 0 || parsed > 180);
  const reach = reachDraft.trim() === '' ? null : Number(reachDraft);
  const reachInvalid = reach !== null && (!Number.isFinite(reach) || reach < 1 || reach > 3650);
  const complete = completeDraft.trim() === '' ? null : Number(completeDraft);
  const completeInvalid = complete !== null && (!Number.isInteger(complete) || complete < 2 || complete > 64);
  const dirty =
    reach !== (current.dark_reach_days ?? null) ||
    complete !== (current.complete_dark_frames ?? null) ||
    (parsed === null) !== (current.rotation_tolerance_deg === null) ||
    (parsed !== null && parsed !== current.rotation_tolerance_deg) ||
    policy !== current.external_masters ||
    flatStarMasking !== (current.flat_star_masking ?? false);
  const policyHint =
    EXTERNAL_MASTER_OPTIONS.find((option) => option.value === policy)?.hint ?? '';

  return (
    <div className="calibration-matching-settings">
      <h3>Calibration</h3>
      <fieldset className="calibration-settings-group calibration-matching-group" disabled={save.isPending}>
        <legend>Matching</legend>
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
      </fieldset>
      <fieldset className="calibration-settings-group" disabled={save.isPending}>
        <legend>Dark masters</legend>
        <label className="review-preference">
          <span>
            Dark reach (days)
            <small>Darks this close to the lights. Marked darks follow their mark. Empty: {current.default_dark_reach_days ?? 183}.</small>
          </span>
          <input type="number" min={1} max={3650} step={1} value={reachDraft} placeholder={String(current.default_dark_reach_days ?? 183)}
            aria-label="Dark reach in days" aria-invalid={reachInvalid} onChange={(event) => setReachDraft(event.target.value)} />
        </label>
        {reachInvalid && <p className="error-text">Enter 1 to 3650 days.</p>}
        <label className="review-preference">
          <span>
            Complete night (darks)
            <small>The nearest night with this many matching darks is used alone. Fewer: nights pool. Empty: {current.default_complete_dark_frames ?? 10}.</small>
          </span>
          <input type="number" min={2} max={64} step={1} value={completeDraft} placeholder={String(current.default_complete_dark_frames ?? 10)}
            aria-label="Darks in a complete night" aria-invalid={completeInvalid} onChange={(event) => setCompleteDraft(event.target.value)} />
        </label>
        {completeInvalid && <p className="error-text">Enter 2 to 64 frames.</p>}
      </fieldset>
      <fieldset className="calibration-settings-group" disabled={save.isPending}>
        <legend>Flat masters</legend>
        <label className="review-preference">
          <input
            type="checkbox"
            checked={flatStarMasking}
            onChange={(event) => setFlatStarMasking(event.target.checked)}
          />
          <span>Mask stars in flats</span>
        </label>
      </fieldset>
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
      <button
        type="button"
        className="save-button"
        disabled={invalid || reachInvalid || completeInvalid || !dirty || save.isPending}
        onClick={() =>
          save.mutate({
            rotation_tolerance_deg: parsed,
            external_masters: policy,
            flat_star_masking: flatStarMasking,
            dark_reach_days: reach,
            complete_dark_frames: complete,
          })
        }
      >
        {save.isPending ? 'Saving…' : 'Save'}
      </button>
    </div>
  );
}
