import { useEffect, useRef, useState } from 'react';
import type { DirectorVisibleSky } from '../../api/directorTypes';
import { openSettings } from '../../utils/settingsIntent';
import {
  DEFAULT_RANGE_NIGHTS,
  FIRST_NIGHT,
  LAST_NIGHT,
  MAX_VISIBLE_NIGHTS,
  addDays,
  currentChoice,
  darknessNote,
  firstNightFrom,
  rangeProblem,
  visibleSkyError,
  visibleSkyLegend,
  visibleSkyShares,
  type VisibleSkyChoice,
} from './visibleSky';

export interface VisibleSkyRig {
  id: string;
  name: string;
  /** The rig's database, for the link to its Rig setup. */
  slug: string | null;
}

interface Props {
  rigs: VisibleSkyRig[];
  choice: VisibleSkyChoice;
  onChange: (next: VisibleSkyChoice) => void;
  /** The lines now drawn, if any; kept on screen while the next load. */
  view: DirectorVisibleSky | undefined;
  /** Lines are being fetched. Marked busy, never announced: the legend
   *  speaks only when what it describes changes. */
  loading: boolean;
  error: unknown;
  nowMs: number;
}

const MAX_MINUTES = 24 * 60;
/** How long the minimum field waits after the last keystroke or spinner
 *  step before it asks for new lines. */
export const MINIMUM_SETTLE_MS = 500;

/** The Visible sky controls and legend: which rig, how long a place must
 *  stay in view, and which nights. The lines themselves are on the map. */
export default function SkyVisible({ rigs, choice, onChange, view, loading, error, nowMs }: Props) {
  const rig = rigs.find((entry) => entry.id === choice.rigId) ?? null;
  // What is typed, which may not be a number yet. Each value asked for
  // measures the whole sky, so a value is sent once the person stops
  // typing or stepping, or at once when they leave the field or press Enter.
  const shownMinutes = choice.minimumMinutes ?? view?.rig_minimum_minutes ?? null;
  const [typed, setTyped] = useState<string>(shownMinutes === null ? '' : String(shownMinutes));
  const settle = useRef<number | null>(null);
  const cancelSettle = () => {
    if (settle.current !== null) window.clearTimeout(settle.current);
    settle.current = null;
  };
  useEffect(() => {
    cancelSettle();
    setTyped(shownMinutes === null ? '' : String(shownMinutes));
  }, [shownMinutes]);
  useEffect(() => cancelSettle, []);
  // The latest choice, for a value sent after a wait.
  const latest = useRef({ choice, onChange, shownMinutes });
  latest.current = { choice, onChange, shownMinutes };

  const sendMinutes = (text: string) => {
    cancelSettle();
    const minutes = Number(text);
    const now = latest.current;
    if (text.trim() === '' || !Number.isInteger(minutes) || minutes < 1 || minutes > MAX_MINUTES) return;
    if (minutes !== now.shownMinutes) now.onChange({ ...now.choice, minimumMinutes: minutes });
  };
  const typeMinutes = (text: string) => {
    setTyped(text);
    cancelSettle();
    settle.current = window.setTimeout(() => sendMinutes(text), MINIMUM_SETTLE_MS);
  };
  const chooseWhen = (when: VisibleSkyChoice['when']) => {
    if (when === 'tonight') {
      onChange({ ...choice, when });
      return;
    }
    // The rig's own night when the lines for Tonight are drawn; a range
    // remembered from nights now past starts over from there.
    const today = firstNightFrom(choice.when === 'tonight' ? view : undefined, nowMs);
    const kept = currentChoice({ ...choice, when }, today);
    onChange(kept.from && kept.to ? kept : { ...kept, from: today, to: addDays(today, DEFAULT_RANGE_NIGHTS - 1) });
  };
  const problem = rangeProblem(choice);
  const failure = error ? visibleSkyError(error) : null;
  const changed = choice.minimumMinutes !== null && view !== undefined && choice.minimumMinutes !== view.rig_minimum_minutes;

  return (
    <div className="sky-visible-panel" role="group" aria-label="Visible sky" aria-busy={loading}>
      <div className="sky-visible-fields">
        <select
          aria-label="Rig"
          value={rig?.id ?? ''}
          disabled={rigs.length === 0}
          onChange={(event) => onChange({ ...choice, rigId: event.target.value || null, minimumMinutes: null })}
        >
          {rigs.length === 0 && <option value="">No rigs</option>}
          {rigs.map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.name}
            </option>
          ))}
        </select>
        <label className="sky-visible-minimum">
          In view for
          <input
            type="number"
            aria-label="Minimum minutes"
            min={1}
            max={MAX_MINUTES}
            step={5}
            value={typed}
            onChange={(event) => typeMinutes(event.target.value)}
            onBlur={(event) => sendMinutes(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') sendMinutes(event.currentTarget.value);
            }}
          />
          min or more
        </label>
        {changed && (
          <button type="button" className="link-button" onClick={() => onChange({ ...choice, minimumMinutes: null })}>
            Use rig's {view.rig_minimum_minutes} min
          </button>
        )}
        <div className="sky-control-group" role="radiogroup" aria-label="Nights">
          <button type="button" role="radio" aria-checked={choice.when === 'tonight'} className="sky-chip" onClick={() => chooseWhen('tonight')}>
            Tonight
          </button>
          <button type="button" role="radio" aria-checked={choice.when === 'dates'} className="sky-chip" onClick={() => chooseWhen('dates')}>
            Dates
          </button>
        </div>
        {choice.when === 'dates' && (
          <span className="sky-visible-dates">
            <input
              type="date"
              aria-label="First night"
              min={FIRST_NIGHT}
              max={LAST_NIGHT}
              value={choice.from ?? ''}
              onChange={(event) => onChange({ ...choice, from: event.target.value || null })}
            />
            to
            <input
              type="date"
              aria-label="Last night"
              value={choice.to ?? ''}
              min={choice.from ?? FIRST_NIGHT}
              max={choice.from && addDays(choice.from, MAX_VISIBLE_NIGHTS - 1) < LAST_NIGHT ? addDays(choice.from, MAX_VISIBLE_NIGHTS - 1) : LAST_NIGHT}
              onChange={(event) => onChange({ ...choice, to: event.target.value || null })}
            />
          </span>
        )}
      </div>
      <p className="sky-visible-legend" role="status">
        {rigs.length === 0 ? (
          'No rigs yet.'
        ) : problem ? (
          problem
        ) : failure ? (
          <>
            {failure.text}{' '}
            {failure.needsSite && (
              <button type="button" className="link-button" onClick={() => openSettings('rigs', { databaseId: rig?.slug ?? undefined })}>
                Open Rig setup
              </button>
            )}
          </>
        ) : view && rig ? (
          <>
            <span className="sky-visible-key" aria-hidden="true" />
            {visibleSkyLegend(view, rig.name, choice.when === 'tonight' ? nowMs : null)}. {visibleSkyShares(view)} Marks point into the visible sky.
            {darknessNote(view) ? ` ${darknessNote(view)}` : ''}
          </>
        ) : (
          'Working out the visible sky…'
        )}
      </p>
    </div>
  );
}
