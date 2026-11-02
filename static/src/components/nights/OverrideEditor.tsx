import { useState } from 'react';
import { useQueryClient, type UseQueryResult } from '@tanstack/react-query';
import { ArrowDown, ArrowUp, X } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorScheduleGoal, NightOverride, NightOverridesNight, RigNight, RigNights, ScheduledNight } from '../../api/nightPlanTypes';
import { httpStatus } from '../../utils/httpStatus';
import { useDraftSection } from '../director/pageDraftsState';
import { BROWSER_ZONE, clockOnNight, formatClock, usableZone, windowFor } from './nightsModel';
import { describeChanges, describeOverride, idlePins, sameOverrides, subjectFromKey, subjectKey, subjectsOf } from './overridesModel';

/** Everything a saved override changes: the night's own list, its schedule
 *  wherever it is drawn, and the plans' estimates. */
const OVERRIDDEN_QUERIES = [['nightOverrides'], ['nightPlans'], ['nightPlanDetail'], ['nightPlansTonight'], ['directorProjection'], ['directorFeasibility']] as const;

type Kind = 'pin' | 'exclude' | 'hold' | 'order';

const KIND_LABEL: Record<Kind, string> = {
  pin: 'Pin',
  exclude: 'Leave out',
  hold: 'Hold',
  order: 'Plan order',
};

const KIND_HELP: Record<Kind, string> = {
  pin: 'Only this runs between From and To.',
  exclude: 'This gets no time between From and To, or all night when both are empty.',
  hold: 'The rig starts no more work from this time, or all night when From is empty.',
  order: "The night's plans run in this order.",
};

interface Props {
  rig: RigNights;
  night: RigNight;
  /** The night's saved overrides, as read. */
  saved: UseQueryResult<NightOverridesNight>;
  /** The night as last planned: with the draft while one is open. */
  scheduled: ScheduledNight | undefined;
  goals: DirectorScheduleGoal[];
  tonight: boolean;
  now: number;
  /** The unsaved edit the schedule previews; `null` when none is open. */
  draft: NightOverride[] | null;
  /** The revision the open edit started from, which Save sends back. */
  base: number | null;
  onDraft: (entries: NightOverride[] | null) => void;
}

function failure(error: unknown): string {
  const message = error instanceof Error ? error.message : '';
  if (httpStatus(error) === 409) {
    // The rig no longer counts as Director's, or someone saved first.
    return message.includes('Target Scheduler') ? message
      : "the night's overrides changed since the edit started. They have been read again; make the change once more.";
  }
  return message || 'the server refused them.';
}

/**
 * A Director rig's overrides for one night, below its timeline: the saved
 * list, and while the night is edited, quick changes, notes on what the
 * edit does, and entry by typed times. The timeline holds most of the
 * editing, and the page's save bar saves or drops the edit. A saved change
 * reaches a running session at its next check-in.
 */
export default function OverrideEditor({ rig, night, saved, scheduled, goals, tonight, now, draft, base, onDraft }: Props) {
  const client = useQueryClient();
  const zone = usableZone(rig.time_zone);
  const refresh = () => { for (const queryKey of OVERRIDDEN_QUERIES) void client.invalidateQueries({ queryKey: [...queryKey] }); };
  const stored = saved.data?.entries ?? [];
  const entries = draft ?? stored;
  const unsaved = draft !== null && !sameOverrides(draft, stored);
  // Why the last save was refused, once the edit it dropped is gone from
  // the save bar.
  const [refused, setRefused] = useState<string | null>(null);
  useDraftSection('night-overrides', {
    label: "This night's overrides",
    order: 1,
    unsaved,
    changes: draft ? describeChanges(stored, draft, goals, zone) : [],
    save: async () => {
      if (!draft) return false;
      try {
        // The revision the edit started from: a refresh during the edit
        // must not let it overwrite a change saved meanwhile.
        const revision = base ?? 0;
        if (draft.length === 0) await apiClient.clearNightOverrides(rig.rig.id, night.date, revision);
        else await apiClient.saveNightOverrides(rig.rig.id, night.date, revision, draft);
        setRefused(null);
        onDraft(null);
        refresh();
        return true;
      } catch (error) {
        // The edit started from a list that is gone: read the night again
        // rather than save the edit over it.
        if (httpStatus(error) === 409) {
          setRefused(`The overrides were not saved: ${failure(error)}`);
          onDraft(null);
          refresh();
        }
        return failure(error);
      }
    },
    discard: () => onDraft(null),
  });
  const idle = scheduled ? idlePins(entries, scheduled, goals) : [];
  const add = (entry: NightOverride) => {
    // A night takes one order and one hold: a new one stands in their place.
    const kept = entry.kind === 'order' || entry.kind === 'hold' ? entries.filter(e => e.kind !== entry.kind) : entries;
    onDraft([...kept, entry]);
  };
  return <section className="nights-override-editor" aria-label="Overrides">
    <h4>Overrides</h4>
    {refused && draft === null && <p className="director-error" role="alert">{refused}</p>}
    {saved.isError && <p className="director-error" role="alert">{saved.error instanceof Error ? saved.error.message : "The night's overrides could not be loaded"}</p>}
    {entries.length === 0
      ? <p className="director-muted">{draft === null ? 'None.' : 'None. Saving removes this night\'s overrides.'}</p>
      : <ul className="nights-override-list">
        {entries.map((entry, index) => <li key={`${entry.kind}-${index}`}>
          <span>{describeOverride(entry, goals, zone)}</span>
          {draft !== null && <button type="button" aria-label={`Remove: ${describeOverride(entry, goals, zone)}`} title="Remove" onClick={() => onDraft(entries.filter((_, at) => at !== index))}><X size={14} /></button>}
        </li>)}
      </ul>}
    {idle.map(window => <p key={window.start_ms} className="director-muted" role="note">
      Nothing pinned can run from {formatClock(window.start_ms, zone)} to {formatClock(window.end_ms, zone)}, so the rig stays idle then.
    </p>)}
    {draft !== null && <>
      {night.availability.state === 'unavailable' && !entries.some(entry => entry.kind === 'hold') && <p className="nights-mark-row">
        <span className="director-muted">Marked not available, but the rig still runs unless held.</span>
        <button type="button" onClick={() => add({ kind: 'hold' })}>Hold the rig all night</button>
      </p>}
      {tonight && <LiveShifts key={goals.map(goal => goal.goal_id).join()} night={night} goals={goals} now={now} zone={zone} onAdd={add} />}
      <details className="nights-override-times">
        <summary>Add one by typing its times</summary>
        <AddOverride key={goals.map(goal => goal.goal_id).join()} night={night} goals={goals} zone={zone} onAdd={add} />
      </details>
      {tonight && <p className="director-muted">A running session uses saved overrides from its next check-in.</p>}
      {!unsaved && <div className="nights-mark-row"><button type="button" onClick={() => onDraft(null)}>Stop editing</button></div>}
    </>}
  </section>;
}

/** Tonight's quick changes, from now: hold the rig, or give the next hour
 *  to one target. */
function LiveShifts({ night, goals, now, zone, onAdd }: { night: RigNight; goals: DirectorScheduleGoal[]; now: number; zone?: string; onAdd: (entry: NightOverride) => void }) {
  const subjects = subjectsOf(goals);
  const [target, setTarget] = useState(subjects.targets[0] ? subjectKey(subjects.targets[0].subject) : '');
  const end = night.noon_ms + 86_400_000;
  // From the next minute, so the hold does not start in the past.
  const from = Math.ceil((now + 1) / 60_000) * 60_000;
  const subject = subjectFromKey(target);
  return <div className="nights-mark-row" role="group" aria-label="Change tonight from now">
    <button type="button" disabled={from >= end} onClick={() => onAdd({ kind: 'hold', from_ms: from })}>Hold from now</button>
    {subjects.targets.length > 0 && <>
      <label>Next hour to <select value={target} onChange={event => setTarget(event.target.value)}>
        {subjects.targets.map(option => <option key={subjectKey(option.subject)} value={subjectKey(option.subject)}>{option.name}</option>)}
      </select></label>
      <button type="button" disabled={!subject || from >= end}
        onClick={() => subject && onAdd({ kind: 'pin', subject, window: { start_ms: from, end_ms: Math.min(end, from + 3_600_000) } })}>
        Pin from {formatClock(from, zone)}
      </button>
    </>}
  </div>;
}

function AddOverride({ night, goals, zone, onAdd }: { night: RigNight; goals: DirectorScheduleGoal[]; zone?: string; onAdd: (entry: NightOverride) => void }) {
  const subjects = subjectsOf(goals);
  const first = subjects.targets[0] ?? subjects.plans[0];
  const [kind, setKind] = useState<Kind>('pin');
  const [subject, setSubject] = useState(first ? subjectKey(first.subject) : '');
  const [from, setFrom] = useState('');
  const [to, setTo] = useState('');
  const [order, setOrder] = useState(() => subjects.plans.map(plan => plan.subject.kind === 'plan' ? plan.subject.plan_id : ''));
  const [problem, setProblem] = useState<string | null>(null);
  const names = new Map(subjects.plans.map(plan => [plan.subject.kind === 'plan' ? plan.subject.plan_id : '', plan.name]));
  const move = (index: number, by: number) => {
    const next = [...order];
    const [item] = next.splice(index, 1);
    next.splice(index + by, 0, item);
    setOrder(next);
  };
  const submit = () => {
    setProblem(null);
    const chosen = subjectFromKey(subject);
    if (kind === 'order') {
      onAdd({ kind: 'order', plans: order });
      return;
    }
    if (kind === 'hold') {
      if (!from) {
        onAdd({ kind: 'hold' });
        return;
      }
      const at = clockOnNight(night.date, from, zone);
      if (at === null) setProblem('Enter the time to hold from.');
      else if (at < night.noon_ms || at >= night.noon_ms + 86_400_000) setProblem(`${from} falls outside this night, which runs from noon to noon at the site.`);
      else onAdd({ kind: 'hold', from_ms: at });
      return;
    }
    if (!chosen) {
      setProblem('Choose what the override is for.');
      return;
    }
    if (kind === 'exclude' && !from && !to) {
      onAdd({ kind: 'exclude', subject: chosen });
      return;
    }
    const window = windowFor(night, from, to, zone);
    if (!window) {
      setProblem(`The time from ${from || 'dusk'} to ${to || 'dawn'} leaves nothing of this night.`);
      return;
    }
    onAdd({ kind, subject: chosen, window });
  };
  return <form className="nights-mark" onSubmit={event => { event.preventDefault(); submit(); }}>
    <fieldset>
      <legend>Add an override</legend>
      <div className="nights-mark-row" role="radiogroup" aria-label="Kind">
        {(Object.keys(KIND_LABEL) as Kind[]).map(option => <label key={option}>
          <input type="radio" name="override-kind" value={option} checked={kind === option} onChange={() => setKind(option)} /> {KIND_LABEL[option]}
        </label>)}
      </div>
      <p className="director-muted">{KIND_HELP[kind]}</p>
      {(kind === 'pin' || kind === 'exclude') && <div className="nights-mark-row">
        <label>For <select value={subject} onChange={event => setSubject(event.target.value)}>
          <optgroup label="Plans">{subjects.plans.map(option => <option key={subjectKey(option.subject)} value={subjectKey(option.subject)}>{option.name}</option>)}</optgroup>
          <optgroup label="Targets">{subjects.targets.map(option => <option key={subjectKey(option.subject)} value={subjectKey(option.subject)}>{option.name}</option>)}</optgroup>
          <optgroup label="Exposure plans">{subjects.exposurePlans.map(option => <option key={subjectKey(option.subject)} value={subjectKey(option.subject)}>{option.name}</option>)}</optgroup>
        </select></label>
      </div>}
      {kind !== 'order' && <div className="nights-mark-row">
        <label>From <input type="time" value={from} onChange={event => setFrom(event.target.value)} aria-describedby="nights-override-times" /></label>
        {kind !== 'hold' && <label>To <input type="time" value={to} onChange={event => setTo(event.target.value)} aria-describedby="nights-override-times" /></label>}
        <span id="nights-override-times" className="director-muted">
          Times in {zone ?? `this browser's time zone, ${BROWSER_ZONE}`}. {kind === 'hold' ? 'Leave From empty to hold all night.' : 'Leave From or To empty for dusk or dawn.'}
        </span>
      </div>}
      {kind === 'order' && <ol className="nights-order">
        {order.map((plan, index) => <li key={plan}>
          <span>{names.get(plan) ?? plan}</span>
          <button type="button" aria-label={`Move ${names.get(plan) ?? plan} up`} title="Up" disabled={index === 0} onClick={() => move(index, -1)}><ArrowUp size={14} /></button>
          <button type="button" aria-label={`Move ${names.get(plan) ?? plan} down`} title="Down" disabled={index === order.length - 1} onClick={() => move(index, 1)}><ArrowDown size={14} /></button>
        </li>)}
      </ol>}
      {problem && <p className="director-error" role="alert">{problem}</p>}
      <div className="nights-mark-row">
        <button type="submit" disabled={kind === 'order' && order.length < 2}>Add</button>
      </div>
    </fieldset>
  </form>;
}
