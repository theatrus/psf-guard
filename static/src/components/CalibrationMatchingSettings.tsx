import { useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { CalibrationRuleField, CalibrationRuleValue, CalibrationRuleValues } from '../api/types';
import {
  RULE_GROUPS,
  describeRuleValue,
  draftOf,
  focusRuleRow,
  parseDraft,
  sameRuleValue,
  type RuleDraft,
} from '../utils/calibrationRules';
import CalibrationRuleControl from './CalibrationRuleControl';
import { invalidateCalibrationViews } from '../utils/calibrationQueries';
import { invalidateNightSplitViews } from '../hooks/useNightSplit';

/** What each group's settings change, said once above them. */
const GROUP_NOTES: Record<CalibrationRuleField['group'], string> = {
  matching:
    'Which frames calibrate a light. A change builds new masters for the stacks it touches.',
  darks: 'Which darks a master takes. A change builds new masters for the stacks it touches.',
  flat_groups:
    'Which nights share a flat master. Measured groups split where the flats differ, such as where dust came or went. A change builds new masters for the stacks it touches.',
  checks:
    'What the Library, Images and the Calibration panel flag. Nothing is rebuilt. Check only shows a finding but never calls a light one that can\'t be calibrated, so stacks keep it.',
  grades:
    'Where a calibration frame turns suspect, then poor. Grading runs again on the stored measurements; a poor frame leaves its masters. Leave a limit empty for never.',
};

/** What a global field shows: an unset number is empty, so its placeholder names the default. */
function globalDraft(field: CalibrationRuleField, value: CalibrationRuleValue | undefined): RuleDraft {
  if ((field.kind.type === 'number' || field.kind.type === 'count') && value === undefined) return '';
  return draftOf(field, value ?? field.default);
}

/** The value a save writes: `null` for the default, which is not written down. */
function wanted(field: CalibrationRuleField, value: CalibrationRuleValue | null): CalibrationRuleValue | null {
  return value === null || sameRuleValue(value, field.default) ? null : value;
}

function defaultNote(field: CalibrationRuleField): string {
  const text = describeRuleValue(field, field.default);
  return field.kind.type === 'number' || field.kind.type === 'count'
    ? `Empty: ${text}.`
    : `Default: ${text}.`;
}

/**
 * Server-wide calibration checks, persisted in the registry and applied at
 * once: matching tolerances, dark masters, what the light checks flag, and
 * where calibration frames grade suspect or poor. A rig can set any of them
 * for itself (see `CalibrationRigSettings`).
 */
export default function CalibrationMatchingSettings({ focusField = null }: { focusField?: string | null }) {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: ['calibration-settings'],
    queryFn: apiClient.getCalibrationSettings,
    // Another tab or editor may have changed them; read them afresh on opening.
    staleTime: 0,
  });

  // Arriving for one setting, as Calibration's Edit limits asks: bring it
  // into view and focus it once it is drawn, once per arrival.
  const rootRef = useRef<HTMLDivElement>(null);
  const focused = useRef<string | null>(null);
  const drawn = !!settings.data?.fields;
  useEffect(() => {
    if (!focusField) {
      focused.current = null;
      return;
    }
    if (!drawn || focused.current === focusField) return;
    if (focusRuleRow(rootRef.current, focusField)) focused.current = focusField;
  }, [drawn, focusField]);

  const fields = useMemo(() => settings.data?.fields ?? [], [settings.data]);
  const rules: CalibrationRuleValues = useMemo(() => settings.data?.rules ?? {}, [settings.data]);
  // Only what the person edited. A field not here shows the saved value, so
  // the settings read afresh (another tab's save, a rig's save) reach every
  // field nobody is editing, and a save sends only what was edited: what
  // another tab saved, or a value this build cannot read, is kept.
  const [drafts, setDrafts] = useState<Record<string, RuleDraft>>({});
  const [flatStarMasking, setFlatStarMasking] = useState<boolean | null>(null);
  const [excludeUncalibrated, setExcludeUncalibrated] = useState<boolean | null>(null);

  const save = useMutation({
    mutationFn: apiClient.updateCalibrationSettings,
    onSuccess: (updated) => {
      queryClient.setQueryData(['calibration-settings'], updated);
      // Lights' nights may split elsewhere now.
      invalidateNightSplitViews(queryClient);
      setDrafts({});
      setFlatStarMasking(null);
      setExcludeUncalibrated(null);
      // A light's account names the limits it was checked by, and the
      // Library, Grid and reports show what the checks now find.
      invalidateCalibrationViews(queryClient);
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
  const savedMasking = current.flat_star_masking ?? false;
  const savedExclude = current.exclude_uncalibrated ?? false;
  const masking = flatStarMasking ?? savedMasking;
  const exclude = excludeUncalibrated ?? savedExclude;
  const parsed = new Map(
    fields
      .filter((field) => field.key in drafts)
      .map((field) => [field.key, parseDraft(field, drafts[field.key])]),
  );
  const invalid = [...parsed.values()].some((result) => !result.ok);
  // What the save sends: each edited value that differs from the saved one,
  // `null` to go back to the default.
  const edits: Record<string, CalibrationRuleValue | null> = {};
  for (const field of fields) {
    const result = parsed.get(field.key);
    if (!result?.ok) continue;
    const before = wanted(field, rules[field.key] ?? null);
    const after = wanted(field, result.value);
    const same = before === null || after === null ? before === after : sameRuleValue(before, after);
    if (!same) edits[field.key] = after;
  }
  const update: Parameters<typeof apiClient.updateCalibrationSettings>[0] = {};
  if (Object.keys(edits).length > 0) update.rules = edits;
  if (masking !== savedMasking) update.flat_star_masking = masking;
  if (exclude !== savedExclude) update.exclude_uncalibrated = exclude;
  const dirty = Object.keys(update).length > 0;

  const row = (field: CalibrationRuleField) => {
    const result = parsed.get(field.key);
    const error = result && !result.ok ? result.error : null;
    return (
      <div key={field.key} className="review-preference calibration-rule-row" data-rule={field.key}>
        <span>
          {field.label}
          <small>
            {field.help} {defaultNote(field)}
          </small>
        </span>
        <div className="calibration-rule-setting">
          <CalibrationRuleControl
            field={field}
            draft={drafts[field.key] ?? globalDraft(field, rules[field.key])}
            placeholder={typeof field.default === 'number' ? String(field.default) : undefined}
            invalid={!!error}
            onChange={(draft) => setDrafts((previous) => ({ ...previous, [field.key]: draft }))}
          />
          {error && <p className="error-text">{error}</p>}
        </div>
      </div>
    );
  };
  const group = (name: CalibrationRuleField['group']) => fields.filter((field) => field.group === name);
  const legend = (name: CalibrationRuleField['group']) =>
    RULE_GROUPS.find((entry) => entry.group === name)?.legend ?? name;
  const fieldset = (name: CalibrationRuleField['group']) =>
    group(name).length > 0 && (
      <fieldset key={name} className="calibration-settings-group calibration-matching-group" disabled={save.isPending}>
        <legend>{legend(name)}</legend>
        <p className="calibration-group-note">{GROUP_NOTES[name]}</p>
        {group(name).map(row)}
      </fieldset>
    );

  return (
    <div className="calibration-matching-settings" ref={rootRef}>
      <h3>Calibration</h3>
      <p className="review-preferences-note">
        Every database on this server, and every rig without a value of its own (see Rigs below).
        A change applies to the next match, check and stack.
      </p>
      {fieldset('matching')}
      {fieldset('darks')}
      {fieldset('flat_groups')}
      <fieldset className="calibration-settings-group" disabled={save.isPending}>
        <legend>Flat masters</legend>
        <label className="review-preference">
          <input
            type="checkbox"
            checked={masking}
            onChange={(event) => setFlatStarMasking(event.target.checked)}
          />
          <span>Mask stars in flats</span>
        </label>
      </fieldset>
      {fieldset('checks')}
      {group('grades').length > 0 && (
        <details className="calibration-settings-group calibration-grades">
          <summary>{legend('grades')}</summary>
          <fieldset className="calibration-matching-group" aria-label={legend('grades')} disabled={save.isPending}>
            <p className="calibration-group-note">{GROUP_NOTES.grades}</p>
            {group('grades').map(row)}
          </fieldset>
        </details>
      )}
      <fieldset className="calibration-settings-group" disabled={save.isPending}>
        <legend>Stacks and exports</legend>
        <label className="review-preference">
          <input
            type="checkbox"
            checked={exclude}
            onChange={(event) => setExcludeUncalibrated(event.target.checked)}
          />
          <span>
            Leave out lights that can't be calibrated
            <small>
              No matching flat, bias or dark, or flats past the age limit, where the library holds them
              for that camera and the check flags it. Stacks, WBPP runs and every export leave them out,
              and each export says how many and why.
            </small>
          </span>
        </label>
      </fieldset>
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
      <button
        type="button"
        className="save-button"
        disabled={invalid || !dirty || save.isPending}
        onClick={() => save.mutate(update)}
      >
        {save.isPending ? 'Saving…' : 'Save'}
      </button>
    </div>
  );
}
