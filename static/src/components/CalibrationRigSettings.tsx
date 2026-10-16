import { useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  CalibrationRigSettings as RigSettings,
  CalibrationRuleField,
  CalibrationRuleValue,
  CalibrationRuleValues,
  CalibrationSettings,
} from '../api/types';
import {
  RIG_PRESETS,
  RULE_GROUPS,
  describeRuleValue,
  draftOf,
  focusRuleRow,
  parseDraft,
  rulesKey,
  sameRules,
  type RuleDraft,
} from '../utils/calibrationRules';
import { invalidateCalibrationViews } from '../utils/calibrationQueries';
import CalibrationRuleControl from './CalibrationRuleControl';
import { invalidateNightSplitViews } from '../hooks/useNightSplit';
import UnsavedMark from './UnsavedMark';
import { openSettings } from '../utils/settingsIntent';

/** A rig to open on arrival, as a light's Calibration panel names it. */
export interface CalibrationRigFocus {
  uuid: string;
  name: string;
}

interface CalibrationRigSettingsProps {
  /** Open this rig's values, adding it to the list if no catalog records it. */
  focusRig?: CalibrationRigFocus | null;
  /** With `focusRig`: the setting of the rig's to bring into view and focus. */
  focusField?: string | null;
}

/** The values of a rig no catalog lists yet: none, the same object every render. */
const NO_RULES: CalibrationRuleValues = Object.freeze({}) as CalibrationRuleValues;

function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

/**
 * Each rig's own calibration values. A rig is the calibration library's: a
 * telescope, camera, frame size and binning. A value it sets beats the
 * global one, which beats the default; a value it leaves alone inherits.
 */
export default function CalibrationRigSettings({ focusRig = null, focusField = null }: CalibrationRigSettingsProps) {
  const settings = useQuery({
    queryKey: ['calibration-settings'],
    queryFn: apiClient.getCalibrationSettings,
    // Another tab or editor may have changed them; read them afresh on opening.
    staleTime: 0,
  });
  const [open, setOpen] = useState<string | null>(focusRig?.uuid ?? null);
  useEffect(() => {
    if (focusRig) setOpen(focusRig.uuid);
  }, [focusRig]);
  // Arriving for one rig: bring its values into view once they are drawn.
  const ready = !!settings.data?.fields;
  // With a setting named, that setting comes into view and takes focus
  // once the rig's values are open: once per arrival. A rig's own value
  // opens its group.
  const arrived = useRef<string | null>(null);
  useEffect(() => {
    if (!focusRig || !ready) return;
    const arrival = `${focusRig.uuid}:${focusField ?? ''}`;
    if (arrived.current === arrival) return;
    const editor = document.getElementById(`calibration-rig-${focusRig.uuid}`);
    if (!editor) return;
    if (!focusField || !focusRuleRow(editor, focusField)) {
      editor.closest('.calibration-rig-item')?.scrollIntoView?.({ block: 'start' });
    }
    arrived.current = arrival;
  }, [focusField, focusRig, open, ready]);

  if (settings.isLoading || settings.isError || !settings.data?.fields) return null;
  const data = settings.data;
  const listed = data.rigs ?? [];
  const rigs: RigSettings[] =
    focusRig && !listed.some((rig) => rig.rig_uuid === focusRig.uuid)
      ? [{ rig_uuid: focusRig.uuid, name: focusRig.name, databases: [], frames: 0, rules: NO_RULES }, ...listed]
      : listed;

  return (
    <div className="calibration-rig-settings">
      <h3>Rigs</h3>
      <p className="review-preferences-note">
        A rig's own value beats the global one; the rest inherit. A rig here is a telescope, camera,
        frame size and binning; each catalog's{' '}
        <button type="button" className="link-button" onClick={() => openSettings('rigs')}>
          Rigs
        </button>{' '}
        entry links to the ones it records.
      </p>
      {rigs.length === 0 ? (
        <p className="calibration-group-note">
          No rigs yet. Import calibration frames or lights, and their rigs appear here.
        </p>
      ) : (
        <ul className="calibration-rig-list">
          {rigs.map((rig) => {
            const own = Object.keys(rig.rules).length;
            const expanded = open === rig.rig_uuid;
            const meta = [
              rig.detail,
              rig.databases.length > 0 ? rig.databases.join(', ') : null,
              rig.frames > 0 ? plural(rig.frames, 'calibration frame') : null,
            ].filter(Boolean).join(' · ');
            return (
              <li key={rig.rig_uuid} className="calibration-rig-item">
                <button
                  type="button"
                  className="calibration-rig-toggle"
                  aria-expanded={expanded}
                  aria-controls={`calibration-rig-${rig.rig_uuid}`}
                  onClick={() => setOpen(expanded ? null : rig.rig_uuid)}
                >
                  <span className="calibration-rig-name">{rig.name}</span>
                  {meta && <span className="calibration-rig-meta">{meta}</span>}
                  <span className={`calibration-rig-count${own > 0 ? ' has-own-values' : ''}`}>
                    {own > 0 ? plural(own, 'own value') : 'Inherits all'}
                  </span>
                </button>
                {expanded && (
                  <RigEditor
                    rig={rig}
                    fields={data.fields!}
                    globalRules={data.rules ?? {}}
                  />
                )}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

interface RigEditorProps {
  rig: RigSettings;
  fields: CalibrationRuleField[];
  globalRules: CalibrationRuleValues;
}

function ownDrafts(fields: CalibrationRuleField[], rules: CalibrationRuleValues): Record<string, RuleDraft> {
  return Object.fromEntries(
    fields.filter((field) => field.key in rules).map((field) => [field.key, draftOf(field, rules[field.key])]),
  );
}

function RigEditor({ rig, fields, globalRules }: RigEditorProps) {
  const queryClient = useQueryClient();
  // The values the rig sets for itself, as edited; a key not here inherits.
  const [own, setOwn] = useState<Record<string, RuleDraft>>(() => ownDrafts(fields, rig.rules));
  const [preset, setPreset] = useState<string | null>(null);
  // Start again from the saved values only when they change, not each time
  // the settings are read afresh: a global save hands back new objects with
  // the same values, and must not throw away what is being edited here.
  const latest = useRef({ fields, rules: rig.rules });
  latest.current = { fields, rules: rig.rules };
  const savedKey = rulesKey(rig.rules);
  const fieldKeys = fields.map((field) => field.key).join(',');
  useEffect(() => {
    setOwn(ownDrafts(latest.current.fields, latest.current.rules));
  }, [savedKey, fieldKeys]);

  const settled = (updated: CalibrationSettings) => {
    queryClient.setQueryData(['calibration-settings'], updated);
    // A rig's lights' nights may split elsewhere now.
    invalidateNightSplitViews(queryClient);
    invalidateCalibrationViews(queryClient);
    setPreset(null);
  };
  const save = useMutation({
    mutationFn: (rules: CalibrationRuleValues) =>
      apiClient.updateRigCalibrationSettings(rig.rig_uuid, { name: rig.name, rules }),
    onSuccess: settled,
  });
  const reset = useMutation({
    mutationFn: () => apiClient.deleteRigCalibrationSettings(rig.rig_uuid),
    onSuccess: settled,
  });
  const busy = save.isPending || reset.isPending;

  const inherited = (field: CalibrationRuleField): { value: CalibrationRuleValue; source: string } =>
    field.key in globalRules
      ? { value: globalRules[field.key], source: 'global' }
      : { value: field.default, source: 'default' };

  const parsed = useMemo(
    () =>
      new Map(
        fields
          .filter((field) => field.key in own)
          .map((field) => {
            const result = parseDraft(field, own[field.key]);
            // An own number left empty has no value to stand on.
            if (result.ok && result.value === null && field.kind.type !== 'limits') {
              return [field.key, { ok: false as const, error: 'Enter a value, or let it inherit.' }];
            }
            return [field.key, result];
          }),
      ),
    [fields, own],
  );
  const invalid = [...parsed.values()].some((result) => !result.ok);
  const values: CalibrationRuleValues = Object.fromEntries(
    [...parsed.entries()].flatMap(([key, result]) =>
      result.ok && result.value !== null ? [[key, result.value]] : [],
    ),
  );
  const dirty = !sameRules(values, rig.rules);

  const toggle = (field: CalibrationRuleField) =>
    setOwn((previous) => {
      const next = { ...previous };
      if (field.key in next) delete next[field.key];
      else next[field.key] = draftOf(field, inherited(field).value);
      return next;
    });
  const applyPreset = (id: string) => {
    const chosen = RIG_PRESETS.find((entry) => entry.id === id);
    if (!chosen) return;
    setOwn((previous) => {
      const next = { ...previous };
      for (const field of fields) {
        if (field.key in chosen.rules) next[field.key] = draftOf(field, chosen.rules[field.key]);
      }
      return next;
    });
    setPreset(id);
  };
  const chosenPreset = RIG_PRESETS.find((entry) => entry.id === preset);

  return (
    <div
      className="calibration-rig-editor"
      id={`calibration-rig-${rig.rig_uuid}`}
      role="region"
      aria-label={`${rig.name} calibration settings`}
    >
      <UnsavedMark label={`${rig.name} calibration settings`} dirty={dirty} />
      <div className="calibration-rig-presets" role="group" aria-label="Start from">
        <span className="calibration-rig-presets-label">Start from</span>
        {RIG_PRESETS.map((entry) => (
          <button
            key={entry.id}
            type="button"
            className="calibration-rig-preset"
            aria-pressed={preset === entry.id}
            title={entry.hint}
            disabled={busy}
            onClick={() => applyPreset(entry.id)}
          >
            {entry.label}
          </button>
        ))}
      </div>
      {chosenPreset && (
        <p className="calibration-group-note" role="status">
          {chosenPreset.hint} Review the values below, then save.
        </p>
      )}
      {RULE_GROUPS.map(({ group, legend }) => {
        const inGroup = fields.filter((field) => field.group === group);
        if (inGroup.length === 0) return null;
        const ownInGroup = inGroup.filter((field) => field.key in own).length;
        return (
          <details
            key={group}
            className="calibration-settings-group calibration-rig-group"
            open={ownInGroup > 0 || group === 'checks' || undefined}
          >
            <summary>
              {legend}
              {ownInGroup > 0 && <span className="calibration-rig-group-count"> · {plural(ownInGroup, 'own value')}</span>}
            </summary>
            <fieldset className="calibration-matching-group" aria-label={legend} disabled={busy}>
              {inGroup.map((field) => {
                const isOwn = field.key in own;
                const result = parsed.get(field.key);
                const error = result && !result.ok ? result.error : null;
                const from = inherited(field);
                return (
                  <div key={field.key} className="review-preference calibration-rule-row" data-rule={field.key}>
                    <span>
                      {field.label}
                      <small>{field.help}</small>
                    </span>
                    <div className="calibration-rule-setting">
                      {isOwn ? (
                        <CalibrationRuleControl
                          field={field}
                          draft={own[field.key]}
                          invalid={!!error}
                          onChange={(draft) => setOwn((previous) => ({ ...previous, [field.key]: draft }))}
                        />
                      ) : (
                        <p className="calibration-rule-inherited">
                          {describeRuleValue(field, from.value)}
                          <small> · {from.source}</small>
                        </p>
                      )}
                      <button
                        type="button"
                        className="calibration-rule-own"
                        aria-pressed={isOwn}
                        aria-label={`${field.label}: own value`}
                        title={isOwn ? 'Inherit again' : 'Set a value for this rig'}
                        onClick={() => toggle(field)}
                      >
                        Own value
                      </button>
                      {error && <p className="error-text">{error}</p>}
                    </div>
                  </div>
                );
              })}
            </fieldset>
          </details>
        );
      })}
      {(save.isError || reset.isError) && (
        <p className="error-text" role="alert">
          {((save.error ?? reset.error) as Error).message}
        </p>
      )}
      <div className="settings-actions">
        <button
          type="button"
          className="save-button"
          disabled={invalid || !dirty || busy}
          onClick={() => save.mutate(values)}
        >
          {save.isPending ? 'Saving…' : 'Save rig'}
        </button>
        <button
          type="button"
          className="calibration-rig-reset"
          disabled={busy || Object.keys(rig.rules).length === 0}
          onClick={() => reset.mutate()}
        >
          Inherit everything
        </button>
      </div>
    </div>
  );
}
