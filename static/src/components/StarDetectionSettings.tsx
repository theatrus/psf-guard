import { useId, useState, type FormEvent, type ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  HocusFocusVersion,
  ImportProfileResult,
  RigStarDetection,
  StarDetectionProfile,
  StarDetectionRig,
  StarDetectionSettings as Settings,
  StarDetector,
  StarNoiseReduction,
  StarSensitivity,
} from '../api/types';
import { useAccess } from '../auth/access';
import {
  recordOrigin,
  recordSpans,
  recordsThatStillApply,
  recordSummary,
  startOfLocalDay,
} from '../utils/starDetection';
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
  hint: 'Auto: Hocus Focus where N.I.N.A. recorded an FWHM, N.I.N.A.\'s detector elsewhere.',
  options: [['auto', 'Auto'], ['nina', 'N.I.N.A.'], ['hocus_focus', 'Hocus Focus']],
};
const SENSITIVITY: FieldSpec<StarSensitivity> = {
  label: 'Sensitivity',
  hint: 'From N.I.N.A.\'s imaging options. N.I.N.A. default: High.',
  options: [['normal', 'Normal'], ['high', 'High'], ['highest', 'Highest']],
};
const NOISE_REDUCTION: FieldSpec<StarNoiseReduction> = {
  label: 'Noise reduction',
  hint: 'From N.I.N.A.\'s imaging options. N.I.N.A. default: None.',
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

/** What the form edits: the default row, each rig's own row (`null` when
 *  it uses the default), and each profile's manual row (`null` when its
 *  records apply). */
interface Draft {
  default: RigStarDetection;
  rigs: Record<string, RigStarDetection | null>;
  profiles: Record<string, RigStarDetection | null>;
}

function draftOf(settings: Settings): Draft {
  return {
    default: { ...settings.default },
    rigs: Object.fromEntries(settings.rigs.map((rig) => [rig.rig_uuid, rig.settings ? { ...rig.settings } : null])),
    profiles: Object.fromEntries((settings.profiles ?? []).map((profile) => [
      profile.profile_id,
      profile.manual ? { ...profile.manual } : null,
    ])),
  };
}

function sameRows(left: Record<string, RigStarDetection | null>, right: Record<string, RigStarDetection | null>): boolean {
  const keys = new Set([...Object.keys(left), ...Object.keys(right)]);
  return [...keys].every((key) => sameRow(left[key] ?? null, right[key] ?? null));
}

function sameDraft(left: Draft, right: Draft): boolean {
  return sameRow(left.default, right.default)
    && sameRows(left.rigs, right.rigs)
    && sameRows(left.profiles, right.profiles);
}

function profileName(profile: StarDetectionProfile): string {
  return profile.name ?? `Profile ${profile.profile_id.slice(0, 8)}`;
}

function profileMeta(profile: StarDetectionProfile): string {
  return [
    profile.frames > 0 ? `${profile.frames.toLocaleString()} frames` : null,
    profile.databases.length > 0 ? profile.databases.join(', ') : null,
  ].filter(Boolean).join(' · ');
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
      for (const profile of saved.profiles ?? []) {
        const before = profile.manual ?? null;
        const after = next.profiles[profile.profile_id] ?? null;
        if (sameRow(before, after)) continue;
        latest = after
          ? await apiClient.updateProfileStarDetection(profile.profile_id, { name: profile.name, settings: after })
          : await apiClient.deleteProfileStarDetection(profile.profile_id);
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
  const setProfile = (id: string, row: RigStarDetection | null) =>
    setDraft({ ...current, profiles: { ...current.profiles, [id]: row } });
  const adopt = (latest: Settings) => {
    queryClient.setQueryData(STAR_DETECTION_QUERY_KEY, latest);
  };

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
          <label key={key} className="star-detection-field" htmlFor={id} title={spec.hint}>
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
      {!canWrite && (
        <p className="scoring-defaults-intro">Only an editor can change these settings.</p>
      )}
      {/* Each field's hint, as its description and its label's tooltip. */}
      <div className="visually-hidden">
        {FIELDS.map(({ key, spec }) => (
          <span key={key} id={`${fieldId}-${key}-hint`}>{spec.hint}</span>
        ))}
      </div>
      <form className="scoring-defaults-form" onSubmit={submit} noValidate>
        <fieldset className="scoring-defaults-group" disabled={busy}>
          <legend>Default</legend>
          {fields(current.default, `${fieldId}-default`, setDefault)}
        </fieldset>
        <h4 className="star-detection-subheading">N.I.N.A. profiles</h4>
        {(data.profiles ?? []).length === 0 ? (
          <p className="scoring-defaults-intro">No profiles yet.</p>
        ) : (
          (data.profiles ?? []).map((profile) => (
            <ProfileRow
              key={profile.profile_id}
              profile={profile}
              manual={current.profiles[profile.profile_id] ?? null}
              fallback={current.default}
              busy={busy}
              onManual={(row) => setProfile(profile.profile_id, row)}
              onDeleted={adopt}
              renderFields={(row, onChange) =>
                fields(row, `${fieldId}-profile-${profile.profile_id}`, onChange)}
            />
          ))
        )}
        <h4 className="star-detection-subheading">Rigs</h4>
        {data.rigs.length === 0 ? (
          <p className="scoring-defaults-intro">No rigs yet.</p>
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
          <button
            type="submit"
            className="save-button"
            aria-disabled={!dirty || busy}
            title="The next quality scan measures the affected frames again"
          >
            {save.isPending ? 'Saving…' : 'Save star detection'}
          </button>
          <button
            type="button"
            className="cancel-button"
            aria-disabled={atBuiltIn || busy}
            title="Auto · High · None · Hocus Focus 4"
            onClick={() => {
              if (atBuiltIn || busy) return;
              setDraft({ ...current, default: { ...data.builtin } });
            }}
          >
            Reset default to N.I.N.A.&apos;s
          </button>
        </div>
        {save.isError && (
          <p className="scoring-default-error" role="alert">
            Could not save star detection: {(save.error as Error).message}
          </p>
        )}
      </form>
      {canWrite && <ImportProfile onImported={adopt} />}
    </section>
  );
}

interface ProfileRowProps {
  profile: StarDetectionProfile;
  manual: RigStarDetection | null;
  fallback: RigStarDetection;
  busy: boolean;
  onManual: (row: RigStarDetection | null) => void;
  onDeleted: (latest: Settings) => void;
  renderFields: (
    row: RigStarDetection,
    onChange: ((field: Field, value: string) => void) | null,
  ) => ReactNode;
}

/** One N.I.N.A. profile: its recorded settings, their history, and a
 *  manual row that beats them. */
function ProfileRow({ profile, manual, fallback, busy, onManual, onDeleted, renderFields }: ProfileRowProps) {
  const name = profileName(profile);
  const meta = profileMeta(profile);
  const remove = useMutation({
    mutationFn: (recordId: string) => apiClient.deleteProfileRecord(profile.profile_id, recordId),
    onSuccess: onDeleted,
  });
  const latest = profile.records[0];
  const spans = recordSpans(profile.records);
  return (
    <fieldset
      className="scoring-defaults-group star-detection-rig"
      disabled={busy}
      aria-label={`${name} star detection`}
    >
      <legend>{name}</legend>
      {meta && <p className="star-detection-meta">{meta}</p>}
      {latest ? (
        <p className="star-detection-record">
          <span className="star-detection-record-values">{recordSummary(latest)}</span>
          <small>{spans.get(latest.id)} · {recordOrigin(latest)}</small>
        </p>
      ) : (
        <p className="star-detection-meta">No settings recorded</p>
      )}
      {profile.records.length > 1 && (
        <details className="star-detection-history">
          <summary>History ({profile.records.length})</summary>
          <ul>
            {profile.records.map((record) => (
              <li key={record.id}>
                <span>
                  {spans.get(record.id)}: {recordSummary(record)}
                  <small> {recordOrigin(record)}</small>
                </span>
                <button
                  type="button"
                  className="link-button"
                  aria-label={`Delete the record for ${spans.get(record.id)?.toLowerCase()}`}
                  onClick={() => remove.mutate(record.id)}
                >
                  Delete
                </button>
              </li>
            ))}
          </ul>
        </details>
      )}
      {profile.records.length === 1 && (
        <button
          type="button"
          className="link-button star-detection-delete"
          onClick={() => remove.mutate(latest.id)}
        >
          Delete record
        </button>
      )}
      {latest?.notes.map((note) => (
        <p key={note} className="star-detection-meta">{note}</p>
      ))}
      {remove.isError && (
        <p className="scoring-default-error" role="alert">
          Could not delete the record: {(remove.error as Error).message}
        </p>
      )}
      <label className="review-preference star-detection-inherit">
        <input
          type="checkbox"
          checked={manual !== null}
          onChange={(event) => onManual(event.target.checked ? { ...fallback } : null)}
        />
        <span>Set by hand</span>
      </label>
      {manual && renderFields(manual, (field, value) => onManual({ ...manual, [field]: value }))}
    </fieldset>
  );
}

/** Settings › Review › Star detection › Import N.I.N.A. profile: read a
 *  `.profile` file, show what it holds, then store it as a record. */
function ImportProfile({ onImported }: { onImported: (latest: Settings) => void }) {
  const id = useId();
  const [file, setFile] = useState<File | null>(null);
  const [allFrames, setAllFrames] = useState(true);
  const [from, setFrom] = useState('');
  const [version, setVersion] = useState<HocusFocusVersion>('v4');
  const [read, setRead] = useState<ImportProfileResult | null>(null);
  const [stored, setStored] = useState<ImportProfileResult | null>(null);
  const effectiveFrom = allFrames ? undefined : startOfLocalDay(from);
  const datedWithoutDate = !allFrames && !effectiveFrom;
  const run = useMutation({
    mutationFn: ({ preview }: { preview: boolean }) =>
      apiClient.importStarDetectionProfile(file!, { effectiveFrom, hocusFocusVersion: version, preview }),
    onSuccess: (result, { preview }) => {
      if (preview) {
        setRead(result);
        setStored(null);
      } else {
        setRead(null);
        setStored(result);
        onImported(result.settings);
      }
    },
  });
  const changed = () => {
    setRead(null);
    setStored(null);
  };
  const unavailable = !file || datedWithoutDate || run.isPending;
  return (
    <section className="star-detection-import" aria-labelledby={`${id}-heading`}>
      <h4 id={`${id}-heading`} className="star-detection-subheading">Import N.I.N.A. profile</h4>
      <div className="star-detection-fields">
        <label
          className="star-detection-field"
          htmlFor={`${id}-file`}
          title="A .profile file from N.I.N.A.'s profiles folder"
        >
          <span className="scoring-default-label">Profile file</span>
          <input
            id={`${id}-file`}
            type="file"
            accept=".profile,.xml"
            onChange={(event) => {
              setFile(event.target.files?.[0] ?? null);
              changed();
            }}
          />
        </label>
        <label className="star-detection-field" htmlFor={`${id}-version`}>
          <span className="scoring-default-label">Hocus Focus version</span>
          <select
            id={`${id}-version`}
            value={version}
            onChange={(event) => {
              setVersion(event.target.value as HocusFocusVersion);
              changed();
            }}
          >
            <option value="v3">3</option>
            <option value="v4">4</option>
          </select>
        </label>
      </div>
      <fieldset className="star-detection-span">
        <legend className="scoring-default-label">Applies to</legend>
        <label>
          <input
            type="radio"
            name={`${id}-span`}
            checked={allFrames}
            onChange={() => {
              setAllFrames(true);
              changed();
            }}
          />
          All frames
        </label>
        <label>
          <input
            type="radio"
            name={`${id}-span`}
            checked={!allFrames}
            onChange={() => {
              setAllFrames(false);
              changed();
            }}
          />
          Frames from
        </label>
        <input
          type="date"
          aria-label="First day"
          value={from}
          disabled={allFrames}
          onChange={(event) => {
            setFrom(event.target.value);
            changed();
          }}
        />
      </fieldset>
      <div className="scoring-defaults-actions">
        <button
          type="button"
          className="cancel-button"
          aria-disabled={unavailable}
          onClick={() => {
            if (!unavailable) run.mutate({ preview: true });
          }}
        >
          Read file
        </button>
        <button
          type="button"
          className="save-button"
          aria-disabled={unavailable || !read}
          onClick={() => {
            if (!unavailable && read) run.mutate({ preview: false });
          }}
        >
          {run.isPending ? 'Importing…' : 'Import profile'}
        </button>
      </div>
      {read && (
        <div className="star-detection-read" role="status">
          <p>
            <strong>{read.name ?? 'Unnamed profile'}</strong>{' '}
            <small>{read.profile_id}</small>
          </p>
          <p>{recordSummary(read.record)}</p>
          {recordsThatStillApply(
            read.settings.profiles.find((profile) => profile.profile_id === read.profile_id)?.records ?? [],
            effectiveFrom,
          ).map((line) => (
            <p key={line} className="star-detection-meta">{line}</p>
          ))}
          {[...read.warnings, ...read.record.notes].map((warning) => (
            <p key={warning} className="star-detection-meta">{warning}</p>
          ))}
        </div>
      )}
      {stored && (
        <p className="star-detection-read" role="status">
          {stored.stored
            ? `Imported ${stored.name ?? stored.profile_id} · its frames are measured again on the next quality scan`
            : `No change: ${stored.name ?? stored.profile_id} already has these settings`}
        </p>
      )}
      {run.isError && (
        <p className="scoring-default-error" role="alert">
          Could not read the profile: {(run.error as Error).message}
        </p>
      )}
    </section>
  );
}
