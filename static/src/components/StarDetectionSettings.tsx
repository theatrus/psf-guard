import { useId, useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  HocusFocusVersion,
  RigStarDetection,
  StarDetectionRig,
  StarDetectionSettings as Settings,
  StarDetector,
  StarNoiseReduction,
  StarSensitivity,
} from '../api/types';
import { useAccess } from '../auth/access';
import UnsavedMark from './UnsavedMark';

const STAR_DETECTION_QUERY_KEY = ['settings', 'star-detection'] as const;

type Field = keyof RigStarDetection;

interface FieldSpec<T extends string> {
  label: string;
  hint: string;
  options: readonly (readonly [T, string])[];
}

const DETECTOR: FieldSpec<StarDetector> = {
  label: 'Detector',
  hint: 'Auto uses Hocus Focus where N.I.N.A. recorded an FWHM for the frame, and N.I.N.A.\'s detector elsewhere.',
  options: [['auto', 'Auto'], ['nina', 'N.I.N.A.'], ['hocus_focus', 'Hocus Focus']],
};
const SENSITIVITY: FieldSpec<StarSensitivity> = {
  label: 'Sensitivity',
  hint: 'As set in N.I.N.A.\'s imaging options. N.I.N.A. uses High unless changed.',
  options: [['normal', 'Normal'], ['high', 'High'], ['highest', 'Highest']],
};
const NOISE_REDUCTION: FieldSpec<StarNoiseReduction> = {
  label: 'Noise reduction',
  hint: 'As set in N.I.N.A.\'s imaging options. N.I.N.A. uses None unless changed.',
  options: [['none', 'None'], ['normal', 'Normal'], ['high', 'High'], ['highest', 'Highest'], ['median', 'Median']],
};
const HOCUS_FOCUS_VERSION: FieldSpec<HocusFocusVersion> = {
  label: 'Hocus Focus version',
  hint: 'The Hocus Focus release the rig ran.',
  options: [['v3', '3'], ['v4', '4']],
};

const FIELDS: { key: Field; spec: FieldSpec<string> }[] = [
  { key: 'detector', spec: DETECTOR },
  { key: 'sensitivity', spec: SENSITIVITY },
  { key: 'noise_reduction', spec: NOISE_REDUCTION },
  { key: 'hocus_focus_version', spec: HOCUS_FOCUS_VERSION },
];

/** Whether a field applies with the row's detector: sensitivity and noise
 *  reduction tune N.I.N.A.'s detector, the version Hocus Focus. */
function applies(field: Field, row: RigStarDetection): boolean {
  if (field === 'sensitivity' || field === 'noise_reduction') return row.detector !== 'hocus_focus';
  if (field === 'hocus_focus_version') return row.detector !== 'nina';
  return true;
}

function sameRow(left: RigStarDetection | null | undefined, right: RigStarDetection | null | undefined): boolean {
  if (!left || !right) return !left && !right;
  return FIELDS.every(({ key }) => left[key] === right[key]);
}

/** What the form edits: the default row, and each rig's own row (`null`
 *  when it uses the default). */
interface Draft {
  default: RigStarDetection;
  rigs: Record<string, RigStarDetection | null>;
}

function draftOf(settings: Settings): Draft {
  return {
    default: { ...settings.default },
    rigs: Object.fromEntries(settings.rigs.map((rig) => [rig.rig_uuid, rig.settings ? { ...rig.settings } : null])),
  };
}

function sameDraft(left: Draft, right: Draft): boolean {
  if (!sameRow(left.default, right.default)) return false;
  const keys = new Set([...Object.keys(left.rigs), ...Object.keys(right.rigs)]);
  return [...keys].every((key) => sameRow(left.rigs[key] ?? null, right.rigs[key] ?? null));
}

function rigMeta(rig: StarDetectionRig): string {
  return [rig.detail, rig.databases.length > 0 ? rig.databases.join(', ') : null].filter(Boolean).join(' · ');
}

/**
 * Settings › Review › Grading & display › Star detection: how the quality
 * scan detects stars on each rig's frames, so its star counts and HFR
 * compare with what the rig's N.I.N.A. recorded. A default row, and one row
 * per rig the open catalogs record, labelled as Settings › Processing ›
 * Calibration › Rigs labels them. A rig row uses the default until it is
 * given settings of its own. Only an editor may change them.
 */
export default function StarDetectionSettings() {
  const headingId = useId();
  const fieldId = useId();
  const queryClient = useQueryClient();
  const { canWrite } = useAccess();
  const query = useQuery({
    queryKey: STAR_DETECTION_QUERY_KEY,
    queryFn: apiClient.getStarDetectionSettings,
    retry: false,
  });
  const [base, setBase] = useState<Settings | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);

  const save = useMutation({
    mutationFn: async ({ saved, next }: { saved: Settings; next: Draft }) => {
      let latest = saved;
      if (!sameRow(saved.default, next.default)) {
        latest = await apiClient.updateStarDetectionDefault(next.default);
      }
      for (const rig of saved.rigs) {
        const before = rig.settings ?? null;
        const after = next.rigs[rig.rig_uuid] ?? null;
        if (sameRow(before, after)) continue;
        latest = after
          ? await apiClient.updateRigStarDetection(rig.rig_uuid, { name: rig.name, settings: after })
          : await apiClient.deleteRigStarDetection(rig.rig_uuid);
      }
      return latest;
    },
    onSuccess: (latest) => queryClient.setQueryData(STAR_DETECTION_QUERY_KEY, latest),
    // A save that failed part way may still have saved some rows.
    onError: () => queryClient.invalidateQueries({ queryKey: STAR_DETECTION_QUERY_KEY }),
  });

  const data = query.data;
  if (!data) return null;
  const edited = base !== null && draft !== null && !sameDraft(draft, draftOf(base));
  // Newer settings replace the form while nothing is being edited.
  if (data !== base && !edited) {
    setBase(data);
    setDraft(draftOf(data));
  }
  const current = draft ?? draftOf(data);
  const dirty = !sameDraft(current, draftOf(data));
  const busy = !canWrite || save.isPending;
  const atBuiltIn = sameRow(current.default, data.builtin);

  const setDefault = (field: Field, value: string) =>
    setDraft({ ...current, default: { ...current.default, [field]: value } });
  const setRig = (uuid: string, row: RigStarDetection | null) =>
    setDraft({ ...current, rigs: { ...current.rigs, [uuid]: row } });

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!dirty || busy) return;
    save.mutate({ saved: data, next: current }, {
      onSuccess: (latest) => {
        setBase(latest);
        setDraft(draftOf(latest));
      },
    });
  };

  const fields = (row: RigStarDetection, idPrefix: string, onChange: ((field: Field, value: string) => void) | null) => (
    <div className="star-detection-fields">
      {FIELDS.map(({ key, spec }) => {
        const id = `${idPrefix}-${key}`;
        return (
          <label key={key} className="star-detection-field" htmlFor={id}>
            <span className="scoring-default-label">{spec.label}</span>
            <select
              id={id}
              value={row[key]}
              disabled={!onChange || !applies(key, row)}
              aria-describedby={`${fieldId}-${key}-hint`}
              onChange={(event) => onChange?.(key, event.target.value)}
            >
              {spec.options.map(([value, label]) => (
                <option key={value} value={value}>{label}</option>
              ))}
            </select>
          </label>
        );
      })}
    </div>
  );

  return (
    <section className="scoring-defaults star-detection" aria-labelledby={headingId}>
      <UnsavedMark label="star detection" dirty={dirty} />
      <h3 id={headingId}>Star detection</h3>
      <p className="scoring-defaults-intro">
        The quality scan detects stars as each rig&apos;s N.I.N.A. did, so its star counts and HFR
        compare with N.I.N.A.&apos;s. After you save, the next quality scan measures the affected
        frames again.
      </p>
      {!canWrite && (
        <p className="scoring-defaults-intro">Only an editor can change these settings.</p>
      )}
      <ul className="star-detection-hints">
        {FIELDS.map(({ key, spec }) => (
          <li key={key} id={`${fieldId}-${key}-hint`}>
            <strong>{spec.label}:</strong> {spec.hint}
          </li>
        ))}
      </ul>
      <form className="scoring-defaults-form" onSubmit={submit} noValidate>
        <fieldset className="scoring-defaults-group" disabled={busy}>
          <legend>Default</legend>
          <p className="star-detection-meta">Rigs without settings of their own</p>
          {fields(current.default, `${fieldId}-default`, setDefault)}
        </fieldset>
        {data.rigs.length === 0 ? (
          <p className="scoring-defaults-intro">
            No rigs yet. Import lights, and their rigs appear here.
          </p>
        ) : (
          data.rigs.map((rig) => {
            const own = current.rigs[rig.rig_uuid] ?? null;
            const meta = rigMeta(rig);
            const idPrefix = `${fieldId}-rig-${rig.rig_uuid}`;
            return (
              <fieldset
                key={rig.rig_uuid}
                className="scoring-defaults-group star-detection-rig"
                disabled={busy}
                aria-label={`${rig.name} star detection`}
              >
                <legend>{rig.name}</legend>
                {meta && <p className="star-detection-meta">{meta}</p>}
                <label className="review-preference star-detection-inherit">
                  <input
                    type="checkbox"
                    checked={own === null}
                    onChange={(event) =>
                      setRig(rig.rig_uuid, event.target.checked ? null : { ...current.default })}
                  />
                  <span>Use the default</span>
                </label>
                {fields(
                  own ?? current.default,
                  idPrefix,
                  own ? (field, value) => setRig(rig.rig_uuid, { ...own, [field]: value }) : null,
                )}
              </fieldset>
            );
          })
        )}
        {/* aria-disabled, not disabled: a button that disables itself as it
            is pressed would drop keyboard focus out of the form. */}
        <div className="scoring-defaults-actions">
          <button type="submit" className="save-button" aria-disabled={!dirty || busy}>
            {save.isPending ? 'Saving…' : 'Save star detection'}
          </button>
          <button
            type="button"
            className="cancel-button"
            aria-disabled={atBuiltIn || busy}
            onClick={() => {
              if (atBuiltIn || busy) return;
              setDraft({ ...current, default: { ...data.builtin } });
            }}
          >
            Reset the default to N.I.N.A.&apos;s
          </button>
        </div>
        {save.isError && (
          <p className="scoring-default-error" role="alert">
            Could not save star detection: {(save.error as Error).message}
          </p>
        )}
      </form>
    </section>
  );
}
