import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { NightAvailabilityState, NightMarkEdit, NightMarkScope, RigNight, RigNights, ScheduleFidelity, ScheduleRuntimeChoice } from '../../api/nightPlanTypes';
import { formatHours } from '../director/visibilityFormat';
import { httpStatus } from '../../utils/httpStatus';
import { retryWhenBusy } from '../director/retry';
import { describeNow } from '../director/dashboardModel';
import { useDirectorClock } from '../../hooks/useDirectorClock';
import NightTimeline from './NightTimeline';
import {
  RUNTIME_LABEL,
  STATE_LABEL,
  clockValue,
  describeDates,
  eveningOf,
  formatClock,
  formatNightDate,
  isTonight,
  plansOnNight,
  windowFor,
} from './nightsModel';

interface Props {
  rig: RigNights;
  /** One night, or a run of nights to mark together. */
  nights: RigNight[];
  colours: Map<string, number>;
  canMark: boolean;
  /** The grid's range, which the night's schedule carries over from. */
  range: { from: string; count: number };
  onClose: () => void;
}

/** The chosen night of one rig: its darkness, Moon, marks and the hours
 *  each plan could use, and the form that marks it. */
export default function NightDetail({ rig, nights, colours, canMark, range, onClose }: Props) {
  const first = nights[0];
  const single = nights.length === 1;
  return <section className="nights-detail director-records" aria-label="Night details">
    <div className="director-toolbar">
      <h3>{rig.catalog_name} · {describeDates(nights.map(night => night.date))}</h3>
      <button type="button" aria-label="Close night details" title="Close" onClick={onClose}><X size={16} /></button>
    </div>
    {single ? <NightFacts night={first} /> : <p className="director-muted">
      {formatHours(nights.reduce((sum, night) => sum + night.available_dark_hours, 0))} clear of {formatHours(nights.reduce((sum, night) => sum + night.dark_hours, 0))} dark over these nights.
    </p>}
    {single && <NightSchedule rig={rig} night={first} colours={colours} range={range} />}
    {single && <PlanHours rig={rig} night={first} colours={colours} />}
    {rig.warnings.map(warning => <p key={warning} className="director-muted" role="note">{warning}</p>)}
    {canMark && <MarkEditor rig={rig} nights={nights} />}
  </section>;
}

/** The night's schedule from the runtime that drives the rig, or the other
 *  one to compare, with the planner's reasons for each block. */
function NightSchedule({ rig, night, colours, range }: { rig: RigNights; night: RigNight; colours: Map<string, number>; range: { from: string; count: number } }) {
  const [runtime, setRuntime] = useState<ScheduleRuntimeChoice>('auto');
  const [fidelity, setFidelity] = useState<ScheduleFidelity>('estimate');
  const index = rig.nights.findIndex(n => n.date === night.date);
  const shown = runtime === 'auto' ? rig.runtime : runtime;
  const other = rig.runtime === 'target_scheduler' ? 'director' : 'target_scheduler';
  const query = useQuery({
    queryKey: ['nightPlanDetail', rig.rig.id, range.from, range.count, night.date, runtime, fidelity],
    queryFn: () => apiClient.getNightPlans({
      rigs: [rig.rig.id],
      start_ms: eveningOf(range.from),
      nights: range.count,
      schedule_nights: index + 1,
      explain: night.date,
      runtime,
      fidelity,
    }),
    enabled: index >= 0,
    retry: retryWhenBusy,
    retryDelay: 1200,
    refetchOnWindowFocus: false,
  });
  const detail = query.data?.rigs.find(entry => entry.rig.id === rig.rig.id);
  const scheduled = detail?.schedule?.nights.find(n => n.date === night.date);
  // Tonight: what the rig says it is doing, beside what it is planned to do.
  const now = useDirectorClock();
  const tonight = isTonight(night, now);
  const statuses = useQuery({ queryKey: ['directorRigStatuses'], queryFn: apiClient.getDirectorRigStatuses, enabled: tonight, retry: retryWhenBusy, retryDelay: 1200, refetchInterval: 15_000 });
  const live = statuses.data?.find(view => view.rig.id === rig.rig.id);
  return <section className="nights-schedule" aria-label="Schedule">
    <div className="nights-mark-row">
      <h4>{RUNTIME_LABEL[shown]} schedule</h4>
      <button type="button" aria-pressed={runtime !== 'auto'} onClick={() => setRuntime(runtime === 'auto' ? other : 'auto')}>
        Compare with {RUNTIME_LABEL[other]}
      </button>
      {shown === 'target_scheduler' && <label title="Exposure time only, and every frame accepted when the project grades, as Target Scheduler's own Scheduler Preview counts them">
        <input type="checkbox" checked={fidelity === 'target_scheduler_preview'} onChange={event => setFidelity(event.target.checked ? 'target_scheduler_preview' : 'estimate')} /> As Target Scheduler's preview
      </label>}
    </div>
    <p className="director-muted">{shown === 'target_scheduler'
      ? `A prediction of Target Scheduler ${detail?.schedule?.assumptions.target_scheduler_version ?? '5'}'s choices from the rig's database. Target Scheduler decides on the rig; change a plan's priority, limits or rule weights in Target Scheduler to change this prediction.`
      : "Director's own selection, stepped through the night's dark hours. The rig still decides on the night."}
      {detail?.schedule?.assumptions.overheads && ` Times include ${detail.schedule.assumptions.overheads.target_change_seconds} s per target change, ${detail.schedule.assumptions.overheads.filter_change_seconds} s per filter change and ${detail.schedule.assumptions.overheads.per_exposure_seconds} s per exposure.`}
    </p>
    {tonight && <p className="nights-live" role="status">
      <strong>Now:</strong> {live ? describeNow(live, now) : 'no report from the rig'}.
      {' '}Tonight is planned from now, with the frames the rig has taken so far.
    </p>}
    {query.isPending && <p role="status">Planning {formatNightDate(night.date)}...</p>}
    {query.isError && <p className="director-error" role="alert">{query.error instanceof Error ? query.error.message : 'The schedule could not be computed'}</p>}
    {detail?.warnings.map(warning => <p key={warning} className="director-muted" role="note">{warning}</p>)}
    {scheduled && <NightTimeline night={night} scheduled={scheduled} colours={colours} now={tonight ? now : undefined} />}
  </section>;
}

function NightFacts({ night }: { night: RigNight }) {
  const { availability } = night;
  return <>
    <dl className="nights-facts">
      <div><dt>Dark</dt><dd>{night.dusk_ms !== null && night.dawn_ms !== null ? `${formatClock(night.dusk_ms)} – ${formatClock(night.dawn_ms)}, ${formatHours(night.dark_hours)}` : 'No darkness'}</dd></div>
      <div><dt>Clear</dt><dd>{availability.state === 'limited'
        ? availability.windows.map(w => `${formatClock(w.start_ms)} – ${formatClock(w.end_ms)}`).join(', ') + `, ${formatHours(night.available_dark_hours)} dark`
        : availability.state === 'unavailable' ? 'Not available' : `All night, ${formatHours(night.available_dark_hours)} dark`}</dd></div>
      <div><dt>Moon</dt><dd>{Math.round(night.moon_illumination * 100)}% lit, up {formatHours(night.moon_hours_up_in_dark)} of the dark</dd></div>
      <div><dt>Load</dt><dd>{describeLoad(night)}</dd></div>
    </dl>
    {availability.marks.length > 0 && <ul className="nights-marks">{availability.marks.map(mark => <li key={mark.scope}>
      {mark.scope === 'site' ? 'Site' : 'Rig'}: {STATE_LABEL[mark.state]}
      {mark.state === 'limited' && ` (${mark.windows.map(w => `${formatClock(w.start_ms)} – ${formatClock(w.end_ms)}`).join(', ')})`}
      {mark.note && <span className="director-muted"> · {mark.note}</span>}
    </li>)}</ul>}
  </>;
}

function describeLoad(night: RigNight): string {
  const { demand_hours, available_hours, idle_hours, overbooked_hours } = night.load;
  if (demand_hours <= 0) return available_hours > 0 ? `Nothing owed fits; ${formatHours(available_hours)} idle` : 'Nothing owed fits';
  const owed = `${formatHours(demand_hours)} of owed work fits, against ${formatHours(available_hours)} clear`;
  if (overbooked_hours >= 0.05) return `${owed}: overbooked by ${formatHours(overbooked_hours)}`;
  if (idle_hours >= 0.05) return `${owed}: ${formatHours(idle_hours)} idle`;
  return owed;
}

function PlanHours({ rig, night, colours }: { rig: RigNights; night: RigNight; colours: Map<string, number> }) {
  const working = new Set(plansOnNight(rig, night).map(plan => plan.project_id));
  const index = rig.nights.findIndex(n => n.date === night.date);
  const progress = rig.schedule?.progress ?? [];
  const planned = (projectId: number, targetId: number) => progress
    .filter(p => p.project_id === projectId && p.target_id === targetId)
    .reduce((sum, p) => sum + (p.planned[index] ?? 0), 0);
  const completes = (projectId: number, targetId: number) => {
    const plans = progress.filter(p => p.project_id === projectId && p.target_id === targetId && !p.complete_before);
    if (plans.length === 0 || plans.some(p => p.completes_on === null)) return null;
    return Math.max(...plans.map(p => p.completes_on!));
  };
  if (rig.plans.length === 0) return <p className="director-muted">This rig owes no frames.</p>;
  const hours = new Map(night.targets.map(target => [`${target.project_id}:${target.target_id}`, target]));
  return <div className="director-table-scroll"><table className="nights-plan-table">
    <thead><tr><th>Plan</th><th>Target</th><th>Owed</th><th>Up</th><th>Clear</th><th>Moon blocks</th><th>Peak</th>{rig.schedule && <><th>Planned</th><th>Done by</th></>}</tr></thead>
    <tbody>{rig.plans.flatMap(plan => plan.targets.map(target => {
      const tonight = hours.get(`${plan.project_id}:${target.target_id}`);
      return <tr key={`${plan.project_id}:${target.target_id}`} className={working.has(plan.project_id) ? undefined : 'is-idle-plan'}>
        <td><span className="nights-plan-dot" data-colour={colours.get(plan.name) ?? 0} aria-hidden="true" />{plan.name}{plan.priority !== 'normal' && <span className="director-muted"> · {plan.priority} priority</span>}</td>
        <td>{target.name}<span className="director-muted"> · {target.filters.join(', ')}</span></td>
        <td>{formatHours(target.hours_owed)}<span className="director-muted"> · {target.frames_owed} frames</span></td>
        <td>{formatHours(tonight?.hours_up ?? 0)}</td>
        <td>{formatHours(tonight?.hours_up_available ?? 0)}</td>
        <td>{tonight && tonight.hours_moon_blocked > 0 ? formatHours(tonight.hours_moon_blocked) : '—'}</td>
        <td>{tonight && tonight.max_altitude_degrees > -90 ? `${Math.round(tonight.max_altitude_degrees)}°${tonight.transit_ms ? ` at ${formatClock(tonight.transit_ms)}` : ''}` : '—'}</td>
        {rig.schedule && <>
          <td>{planned(plan.project_id, target.target_id) || '—'}</td>
          <td>{(() => {
            const done = completes(plan.project_id, target.target_id);
            return done === null ? <span className="director-muted">not in {rig.schedule.nights.length} nights</span> : formatNightDate(rig.schedule.nights[done]?.date ?? night.date);
          })()}</td>
        </>}
      </tr>;
    }))}</tbody>
  </table></div>;
}

function failure(error: unknown): string {
  if (httpStatus(error) === 409) return 'These marks changed since the page loaded. The nights have been read again; mark them once more.';
  return error instanceof Error ? error.message : 'The marks could not be saved';
}

/** Mark the chosen nights available, out, or clear for some hours, for the
 *  rig alone or for every rig at its site. */
function MarkEditor({ rig, nights }: { rig: RigNights; nights: RigNight[] }) {
  const client = useQueryClient();
  const first = nights[0];
  const [scope, setScope] = useState<NightMarkScope>('rig');
  const current = first.availability.marks.find(mark => mark.scope === scope);
  const [state, setState] = useState<NightAvailabilityState>(current?.state ?? 'unavailable');
  const limited = current?.state === 'limited' ? current.windows[0] : undefined;
  const [from, setFrom] = useState(limited ? clockValue(limited.start_ms) : '');
  const [to, setTo] = useState(limited ? clockValue(limited.end_ms) : '');
  const [note, setNote] = useState(current?.note ?? '');
  const [problem, setProblem] = useState<string | null>(null);
  const scopeId = scope === 'site' ? rig.planning_site?.id ?? null : rig.rig.id;
  const revision = scope === 'site' ? rig.mark_revisions.site ?? 0 : rig.mark_revisions.rig;
  const marked = nights.some(night => night.availability.marks.some(mark => mark.scope === scope));
  const done = () => { setProblem(null); void client.invalidateQueries({ queryKey: ['nightPlans'] }); };
  const fail = (error: unknown) => {
    setProblem(failure(error));
    if (httpStatus(error) === 409) void client.invalidateQueries({ queryKey: ['nightPlans'] });
  };
  const save = useMutation({
    mutationFn: (marks: NightMarkEdit[]) => apiClient.saveNightMarks(scope, scopeId!, revision, marks),
    onSuccess: done,
    onError: fail,
  });
  const clear = useMutation({
    mutationFn: () => apiClient.clearNightMarks(scope, scopeId!, revision, nights.map(night => night.date)),
    onSuccess: done,
    onError: fail,
  });
  const submit = () => {
    const marks: NightMarkEdit[] = [];
    for (const night of nights) {
      if (state !== 'limited') {
        marks.push({ date: night.date, state, note: note.trim() });
        continue;
      }
      const hours = windowFor(night, from, to);
      if (!hours) {
        setProblem(`The hours from ${from || 'dusk'} to ${to || 'dawn'} leave nothing on ${night.date}.`);
        return;
      }
      marks.push({ date: night.date, state, windows: [hours], note: note.trim() });
    }
    save.mutate(marks);
  };
  const busy = save.isPending || clear.isPending;
  return <form className="nights-mark" onSubmit={event => { event.preventDefault(); submit(); }}>
    <fieldset>
      <legend>Mark {nights.length === 1 ? 'this night' : `these ${nights.length} nights`}</legend>
      <div className="nights-mark-row" role="radiogroup" aria-label="Availability">
        {(['available', 'unavailable', 'limited'] as const).map(option => <label key={option}>
          <input type="radio" name="night-state" value={option} checked={state === option} onChange={() => setState(option)} /> {STATE_LABEL[option]}
        </label>)}
      </div>
      {state === 'limited' && <div className="nights-mark-row">
        <label>From <input type="time" value={from} onChange={event => setFrom(event.target.value)} aria-describedby="nights-limited-help" /></label>
        <label>To <input type="time" value={to} onChange={event => setTo(event.target.value)} aria-describedby="nights-limited-help" /></label>
        <span id="nights-limited-help" className="director-muted">Leave From empty to start at dusk, or To empty to end at dawn. Times after midnight fall the next morning.</span>
      </div>}
      <div className="nights-mark-row">
        <label>For <select value={scope} onChange={event => setScope(event.target.value as NightMarkScope)}>
          <option value="rig">This rig only</option>
          {rig.planning_site && <option value="site">Every rig at {rig.planning_site.name}</option>}
        </select></label>
        <label className="nights-mark-note">Note <input type="text" value={note} maxLength={200} placeholder="Dome motor, high cirrus..." onChange={event => setNote(event.target.value)} /></label>
      </div>
      <p className="director-muted">Marks change night plans only. They never start, stop or block a rig: its safety monitor and N.I.N.A. still decide on the night.</p>
      {problem && <p className="director-error" role="alert">{problem}</p>}
      <div className="nights-mark-row">
        <button type="submit" disabled={busy || !scopeId}>Save mark</button>
        {marked && <button type="button" disabled={busy || !scopeId} onClick={() => clear.mutate()}>Clear mark</button>}
      </div>
    </fieldset>
  </form>;
}
