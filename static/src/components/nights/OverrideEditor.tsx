import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowDown, ArrowUp, X } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorScheduleGoal, NightOverride, RigNight, RigNights, ScheduledNight } from '../../api/nightPlanTypes';
import { httpStatus } from '../../utils/httpStatus';
import { retryWhenBusy } from '../director/retry';
import { BROWSER_ZONE, clockOnNight, formatClock, windowFor } from './nightsModel';
import { describeOverride, idlePins, subjectFromKey, subjectKey, subjectsOf } from './overridesModel';

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
  pin: 'Between From and To, only this plan, target or exposure plan may run.',
  exclude: 'This plan, target or exposure plan gets no time between From and To, or all night when both are empty.',
  hold: 'The rig takes no more work from this time, or all night when From is empty.',
  order: "The night's plans in this order, in place of the rig's own.",
};

interface Props {
  rig: RigNights;
  night: RigNight;
  /** The night as last planned: with the draft while one is open. */
  scheduled: ScheduledNight | undefined;
  goals: DirectorScheduleGoal[];
  tonight: boolean;
  now: number;
  canEdit: boolean;
  /** The unsaved edit the schedule previews; `null` when none is open. */
  draft: NightOverride[] | null;
  onDraft: (entries: NightOverride[] | null) => void;
}

function failure(error: unknown): string {
  if (httpStatus(error) === 409) return "The night's overrides changed since they were opened. They have been read again; make the change once more.";
  return error instanceof Error ? error.message : 'The overrides could not be saved';
}

/**
 * A Director rig's overrides for one night: pins, plans left out, a hold
 * and the night's plan order. An edit is previewed in the schedule above
 * before it is saved, and a saved change reaches a running session at its
 * next check-in.
 */
export default function OverrideEditor({ rig, night, scheduled, goals, tonight, now, canEdit, draft, onDraft }: Props) {
  const client = useQueryClient();
  const zone = rig.time_zone ?? undefined;
  const stored = useQuery({
    queryKey: ['nightOverrides', rig.rig.id, night.date],
    queryFn: () => apiClient.getNightOverrides(rig.rig.id, night.date),
    retry: retryWhenBusy,
    retryDelay: 1200,
  });
  const [problem, setProblem] = useState<string | null>(null);
  const refresh = () => { for (const queryKey of OVERRIDDEN_QUERIES) void client.invalidateQueries({ queryKey: [...queryKey] }); };
  const save = useMutation({
    mutationFn: (entries: NightOverride[]) => {
      const revision = stored.data?.revision ?? 0;
      return entries.length === 0
        ? apiClient.clearNightOverrides(rig.rig.id, night.date, revision)
        : apiClient.saveNightOverrides(rig.rig.id, night.date, revision, entries);
    },
    onSuccess: () => { setProblem(null); onDraft(null); refresh(); },
    onError: error => {
      setProblem(failure(error));
      if (httpStatus(error) === 409) refresh();
    },
  });
  const entries = draft ?? stored.data?.entries ?? [];
  const idle = scheduled ? idlePins(entries, scheduled, goals) : [];
  const nightEnded = now >= night.noon_ms + 86_400_000;
  const remove = (index: number) => onDraft(entries.filter((_, at) => at !== index));
  const add = (entry: NightOverride) => {
    // A night takes one order and one hold: a new one stands in their place.
    const kept = entry.kind === 'order' || entry.kind === 'hold' ? entries.filter(e => e.kind !== entry.kind) : entries;
    onDraft([...kept, entry]);
  };
  return <section className="nights-override-editor" aria-label="Overrides">
    <div className="nights-mark-row">
      <h4>Overrides</h4>
      {canEdit && draft === null && !nightEnded && <button type="button" onClick={() => { setProblem(null); onDraft([...(stored.data?.entries ?? [])]); }}>Edit night</button>}
    </div>
    {stored.isError && <p className="director-error" role="alert">{stored.error instanceof Error ? stored.error.message : "The night's overrides could not be loaded"}</p>}
    {entries.length === 0
      ? <p className="director-muted">{draft === null ? 'None: the rig follows its own program this night.' : 'No overrides in this edit: saving returns the night to the rig\'s own program.'}</p>
      : <ul className="nights-override-list">
        {entries.map((entry, index) => <li key={`${entry.kind}-${index}`}>
          <span>{describeOverride(entry, goals, zone)}</span>
          {draft !== null && <button type="button" aria-label={`Remove: ${describeOverride(entry, goals, zone)}`} title="Remove" onClick={() => remove(index)}><X size={14} /></button>}
        </li>)}
      </ul>}
    {idle.map(window => <p key={window.start_ms} className="director-muted" role="note">
      Nothing pinned runs from {formatClock(window.start_ms, zone)} to {formatClock(window.end_ms, zone)}: the sky or the Moon rules it out there, so the rig stays idle in that window.
    </p>)}
    {draft !== null && <>
      <p className="director-muted">The schedule above shows this edit. Drag a block, or select it and press the left or right arrow key, to move it; the move pins its target to the new time. Nothing changes on the rig until the overrides are saved.</p>
      {night.availability.state === 'unavailable' && !entries.some(entry => entry.kind === 'hold') && <p className="nights-mark-row">
        <span className="director-muted">This night is marked not available, which changes the plans only. A hold keeps the rig from taking work.</span>
        <button type="button" onClick={() => add({ kind: 'hold' })}>Hold the rig all night</button>
      </p>}
      {tonight && <LiveShifts night={night} goals={goals} now={now} zone={zone} onAdd={add} />}
      <AddOverride night={night} goals={goals} zone={zone} onAdd={add} />
      <p className="director-muted">{tonight
        ? 'A session already running takes saved overrides at its next check-in: it finishes the exposure under way, then follows the new program.'
        : 'Saved overrides apply to the program the rig pulls for this night.'}</p>
      {problem && <p className="director-error" role="alert">{problem}</p>}
      <div className="nights-mark-row">
        <button type="button" disabled={save.isPending} onClick={() => save.mutate(entries)}>Save overrides</button>
        <button type="button" disabled={save.isPending} onClick={() => { setProblem(null); onDraft(null); }}>Discard</button>
      </div>
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
          Times are in {zone ? `the site's time zone (${zone})` : `this browser's time zone (${BROWSER_ZONE})`}. Leave From empty to start at {kind === 'hold' ? 'the start of the night' : 'dusk'}{kind !== 'hold' && ', or To empty to end at dawn'}. Times after midnight fall the next morning.
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
