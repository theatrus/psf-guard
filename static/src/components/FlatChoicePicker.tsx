import { useState } from 'react';
import type { CalibrationKindExplanation, FlatChoiceNote } from '../api/types';
import { windowLabel } from './calibration/flatGroupLabels';
import { useFlatChoiceAccess, useFlatLightChoice } from './calibration/useFlatGroupChoices';
import { useRigClocks } from './calibration/useRigClocks';

/** Bases that leave a light's flats from across a change. */
const UNSETTLED = new Set(['no_same_side', 'undetermined']);

const PLACED_BY: Record<string, string> = {
  time: 'by its time',
  session: 'by its imaging session',
  pixels: 'by its own pixels',
  undetermined: 'undetermined',
  person: 'chosen by you',
};

/** A choice in a few words. */
function choiceText(choice: FlatChoiceNote): string {
  return choice.choice === 'group' ? `Takes ${choice.label}` : `Put ${choice.label}`;
}

interface FlatChoicePickerProps {
  dbId: string;
  imageId: number;
  /** The light's flat account. */
  kind: CalibrationKindExplanation;
  /** The light's rig: its changes read on the rig's clock. */
  rig?: string;
}

/**
 * A person's say over one light's flats, in its Calibration panel: what
 * they chose (with Reset), what they chose that no longer applies, and, for
 * a light shot inside a change's window or flagged, a side of each change
 * or a group to take. Choosing waits for the database-management gate;
 * everyone sees the choices.
 */
export default function FlatChoicePicker(props: FlatChoicePickerProps) {
  // Detail moves to the next light without remounting the panel: what was
  // ticked or picked for one light never carries to the next.
  return <LightFlatChoices key={props.imageId} {...props} />;
}

function LightFlatChoices({ dbId, imageId, kind, rig }: FlatChoicePickerProps) {
  const clock = useRigClocks(dbId)(rig);
  const canChange = useFlatChoiceAccess();
  const choose = useFlatLightChoice(dbId);
  const [wholeSession, setWholeSession] = useState(false);
  const [group, setGroup] = useState('');
  const choices = kind.choices ?? [];
  const options = kind.choice_options;
  const applied = choices.filter((choice) => !choice.orphaned);
  const orphaned = choices.filter((choice) => choice.orphaned);
  const unsettled = !!kind.basis && UNSETTLED.has(kind.basis.basis);
  const changes = (options?.changes ?? []).filter(
    (change) => change.inside || change.chosen !== null || unsettled,
  );
  const picker = canChange && !!options && (changes.length > 0 || unsettled || applied.length > 0);
  if (applied.length === 0 && orphaned.length === 0 && !picker) return null;

  const send = (request: Omit<Parameters<typeof choose.mutate>[0], 'image_ids' | 'whole_session'>) =>
    choose.mutate({ ...request, image_ids: [imageId], whole_session: wholeSession });
  const reset = (choice: FlatChoiceNote) =>
    send({ choice: 'reset', change_key: choice.change_key ?? 'group' });
  const groups = options?.groups ?? [];
  const chosenGroup = groups.find((option) => option.chosen)?.response_key ?? '';

  return (
    <div className="light-calibration-part light-flat-choice" data-testid="calibration-flat-choice">
      {applied.length > 0 && (
        <>
          <span className="light-calibration-part-label">Chosen by you</span>
          <ul className="light-flat-choice-list">
            {applied.map((choice) => (
              <li key={`${choice.choice}:${choice.change_key ?? 'group'}`} data-testid="calibration-flat-chosen">
                <span>{choiceText(choice)}</span>
                {canChange && (
                  <button type="button" className="light-calibration-tune" disabled={choose.isPending}
                    onClick={() => reset(choice)}>
                    Reset
                  </button>
                )}
              </li>
            ))}
          </ul>
        </>
      )}
      {orphaned.length > 0 && (
        <>
          <span className="light-calibration-part-label">Chosen by you, no longer applied</span>
          <ul className="light-flat-choice-list">
            {orphaned.map((choice) => (
              <li key={`${choice.choice}:${choice.change_key ?? 'group'}`} className="is-orphaned"
                data-testid="calibration-flat-orphaned">
                <span>{choiceText(choice)}: {choice.orphaned}</span>
                {canChange && (
                  <button type="button" className="light-calibration-tune" disabled={choose.isPending}
                    onClick={() => reset(choice)}>
                    Reset
                  </button>
                )}
              </li>
            ))}
          </ul>
        </>
      )}
      {picker && (
        <>
          <span className="light-calibration-part-label">Choose its flats</span>
          {changes.map((change) => (
            <div key={change.key} className="light-flat-side" data-testid="calibration-flat-side">
              <span>
                {change.what} · {windowLabel(change.start, change.end, clock)}
                <span className="light-calibration-muted">
                  {' · '}
                  {change.chosen
                    ? `you put it ${change.chosen}`
                    : change.side
                      ? `${change.side} it ${PLACED_BY[change.by] ?? change.by}`
                      : 'side undetermined'}
                </span>
              </span>
              <span className="light-flat-side-buttons" role="group" aria-label={`Its side of the change ${windowLabel(change.start, change.end, clock)}`}>
                {(['before', 'after'] as const).map((side) => (
                  <button key={side} type="button" className="light-calibration-tune"
                    aria-pressed={change.chosen === side} disabled={choose.isPending}
                    onClick={() => send(change.chosen === side
                      ? { choice: 'reset', change_key: change.key }
                      : { choice: side, change_key: change.key })}>
                    {side === 'before' ? 'Before' : 'After'}
                  </button>
                ))}
              </span>
            </div>
          ))}
          {groups.length > 0 && (
            <div className="light-flat-side">
              <label className="light-flat-group">
                <span>Or take one group's flats</span>
                <select value={group || chosenGroup} onChange={(event) => setGroup(event.target.value)}
                  data-testid="calibration-flat-group">
                  <option value="">Choose a group…</option>
                  {groups.map((option) => (
                    <option key={option.response_key} value={option.response_key}>
                      {option.family}, {option.first_night === option.last_night ? option.first_night : `${option.first_night} to ${option.last_night}`}
                      {option.current ? ' (now)' : ''}
                    </option>
                  ))}
                </select>
              </label>
              <button type="button" className="light-calibration-tune"
                disabled={choose.isPending || !(group || chosenGroup) || (group || chosenGroup) === chosenGroup}
                onClick={() => send({ choice: 'group', group_response: group || chosenGroup })}>
                Use group
              </button>
            </div>
          )}
          <label className="light-flat-session">
            <input type="checkbox" checked={wholeSession} onChange={(event) => setWholeSession(event.target.checked)} />
            Every light of its imaging session
          </label>
          {choose.error instanceof Error && (
            <p className="light-calibration-missing" role="alert">Could not set the choice: {choose.error.message}</p>
          )}
        </>
      )}
    </div>
  );
}
