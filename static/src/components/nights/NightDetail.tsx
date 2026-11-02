import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { NightAvailabilityState, NightInterval, NightMarkEdit, NightMarkScope, NightOverride, RigNight, RigNights, ScheduleBlock, TsChanges } from '../../api/nightPlanTypes';
import { formatHours } from '../director/visibilityFormat';
import { httpStatus } from '../../utils/httpStatus';
import { retryWhenBusy } from '../director/retry';
import { describeNow } from '../director/dashboardModel';
import { useDirectorClock } from '../../hooks/useDirectorClock';
import NightTimeline from './NightTimeline';
import OverrideEditor from './OverrideEditor';
import TargetSchedulerControls from './TargetSchedulerControls';
import { isEmpty, tsChanges, type TsEdit } from './tsControlsModel';
import { OverrideChips, type TimelineEdit } from './TimelineEditing';
import { DraftProvider, SaveBar } from '../director/pageDrafts';
import { usePageDrafts } from '../director/pageDraftsState';
import { blockSubject, moveBlock, overrideBands, subjectKey } from './overridesModel';
import {
  BROWSER_ZONE,
  RUNTIME_LABEL,
  STATE_LABEL,
  clockValue,
  describeDates,
  formatClock,
  usableZone,
  formatNightDate,
  isTonight,
  completesOn,
  markOrigin,
  OUT_REASON,
  planRows,
  windowFor,
  type PlanRow,
} from './nightsModel';
import { PROVIDER_LABEL, PROVIDER_URL } from '../director/forecastLabels';

/** A time on the rig's site clock, or the browser's when the site names no
 *  time zone. */
function siteClock(rig: RigNights, ms: number): string {
  return formatClock(ms, usableZone(rig.time_zone));
}

/** An open edit of one rig's night: its overrides as edited, the revision
 *  the edit started from, which Save sends back, and for each target a
 *  block move pinned, where its block stood first and what the move made. */
interface OverrideEdit {
  key: string;
  base: number;
  entries: NightOverride[];
  moves: Record<string, { origin: NightInterval; entries: NightOverride[] }>;
}

interface Props {
  rig: RigNights;
  /** One night, or a run of nights to mark together. */
  nights: RigNight[];
  colours: Map<string, number>;
  canMark: boolean;
  /** The grid's range, which the night's schedule carries over from: its
   *  chosen first night, or none for the night under way. */
  range: { first: string | null; count: number };
  onClose: () => void;
}

/** The chosen night of one rig: its darkness, Moon, marks and the hours
 *  each plan could use, and the form that marks it. */
export default function NightDetail({ rig, nights, colours, canMark, range, onClose }: Props) {
  const first = nights[0];
  const single = nights.length === 1;
  // An open edit of the night's overrides belongs to its rig and night.
  const [stored, setEdit] = useState<OverrideEdit | null>(null);
  const editKey = `${rig.rig.id}:${first.date}`;
  const edit = stored?.key === editKey ? stored : null;
  const draft = edit?.entries ?? null;
  const setDraft = (entries: NightOverride[] | null) => setEdit(entries === null || !edit ? null : { ...edit, entries });
  const startEdit = (entries: NightOverride[], base: number) => setEdit({ key: editKey, base, entries, moves: {} });
  // An open edit of the rig's Target Scheduler settings belongs to the rig.
  const [tsStored, setTsEdit] = useState<{ rig: string; edit: TsEdit } | null>(null);
  const tsEdit = tsStored?.rig === rig.rig.id ? tsStored.edit : null;
  const tsDraft = tsEdit ? tsChanges(rig.ts_projects ?? [], tsEdit) : null;
  const schedule = useNightSchedule(rig, first, range, single, draft, tsDraft && !isEmpty(tsDraft) ? tsDraft : null);
  const director = rig.runtime === 'director' && schedule.shown === 'director';
  const drafts = usePageDrafts();
  const saved = useQuery({
    queryKey: ['nightOverrides', rig.rig.id, first.date],
    queryFn: () => apiClient.getNightOverrides(rig.rig.id, first.date),
    enabled: single && director,
    retry: retryWhenBusy,
    retryDelay: 1200,
  });
  const scheduled = schedule.detail?.schedule?.nights.find(n => n.date === first.date);
  const goals = schedule.detail?.schedule?.goals ?? [];
  const [moveProblem, setMoveProblem] = useState<string | null>(null);
  const move = (block: ScheduleBlock, deltaMs: number) => {
    if (!edit || !scheduled) return;
    // A move made before the last one is planned would start from where the
    // block no longer stands.
    if (schedule.query.isFetching) {
      setMoveProblem('Wait for the schedule to show the last change, then move the block again.');
      return;
    }
    const subject = blockSubject(block, goals);
    if (!subject) {
      setMoveProblem("This block's target cannot be told from the rig's goals, so it cannot be moved.");
      return;
    }
    const key = subjectKey(subject);
    const before = edit.moves[key];
    const moved = moveBlock(block, deltaMs, goals, { start_ms: scheduled.start_ms, end_ms: scheduled.end_ms }, before?.origin);
    if (!moved) {
      setMoveProblem('That move would take the block outside the night.');
      return;
    }
    // A target moved again replaces what its last move made, and only that.
    const replaced = before?.entries ?? [];
    const [pin] = moved.entries;
    const clash = pin.kind === 'pin' && edit.entries.some(entry => entry.kind === 'pin' && !replaced.includes(entry)
      && entry.window.start_ms < pin.window.end_ms && entry.window.end_ms > pin.window.start_ms);
    if (clash) {
      setMoveProblem('The new time overlaps another pin. Remove that pin first, or move the block elsewhere.');
      return;
    }
    setMoveProblem(null);
    setEdit({
      ...edit,
      entries: [...edit.entries.filter(entry => !replaced.includes(entry)), ...moved.entries],
      moves: { ...edit.moves, [key]: { origin: before?.origin ?? { start_ms: block.start_ms, end_ms: block.end_ms }, entries: moved.entries } },
    });
  };
  const zone = usableZone(rig.time_zone);
  const timelineEdit: TimelineEdit | undefined = director && edit && scheduled ? {
    goals,
    entries: edit.entries,
    night: { start_ms: scheduled.start_ms, end_ms: scheduled.end_ms },
    dawn: first.dawn_ms ?? scheduled.end_ms,
    zone,
    onChange: entries => { setMoveProblem(null); setDraft(entries); },
    onMoveBlock: move,
  } : undefined;
  const nightEnded = schedule.now >= first.noon_ms + 86_400_000;
  const canEdit = single && director && canMark && !nightEnded && !!saved.data && goals.length > 0;
  return <DraftProvider drafts={drafts}><section className="nights-detail director-records" aria-label="Night details">
    <SaveBar drafts={drafts} canWrite={canMark} />
    <div className="director-toolbar">
      <h3>{rig.catalog_name} · {describeDates(nights.map(night => night.date))}</h3>
      <span className="nights-mode" data-runtime={rig.runtime} title={rig.runtime === 'director'
        ? "Director picks this rig's work, and takes overrides for a night."
        : "Target Scheduler picks this rig's work from its database."}>{RUNTIME_LABEL[rig.runtime]} decides</span>
      <button type="button" aria-label="Close night details" title="Close" onClick={onClose}><X size={16} /></button>
    </div>
    {single ? <NightFacts rig={rig} night={first} /> : <p className="director-muted">
      {formatHours(nights.reduce((sum, night) => sum + night.available_dark_hours, 0))} clear of {formatHours(nights.reduce((sum, night) => sum + night.dark_hours, 0))} dark over these nights.
    </p>}
    {single && <NightSchedule rig={rig} night={first} colours={colours} view={schedule}
      edit={timelineEdit} moveProblem={edit ? moveProblem : null}
      saved={director ? saved.data?.entries ?? [] : []}
      onEdit={canEdit && !edit && saved.data ? () => { setMoveProblem(null); startEdit([...saved.data.entries], saved.data.revision); } : undefined} />}
    {single && rig.runtime === 'target_scheduler' && schedule.shown === 'target_scheduler' && <TargetSchedulerControls rig={rig}
      saved={rig.ts_projects ?? []} canEdit={canMark} edit={tsEdit} onEdit={edit => setTsEdit(edit ? { rig: rig.rig.id, edit } : null)} />}
    {single && director && <OverrideEditor rig={rig} night={first} saved={saved}
      scheduled={scheduled} goals={goals} tonight={schedule.tonight} now={schedule.now}
      draft={draft} base={edit?.base ?? null} onDraft={setDraft} />}
    {single && <PlanHours rig={rig} night={first} colours={colours} view={schedule} />}
    {rig.warnings.map(warning => <p key={warning} className="director-muted" role="note">{warning}</p>)}
    {canMark && <MarkEditor rig={rig} nights={nights} />}
  </section></DraftProvider>;
}

/** The schedule shown for a night: from the runtime that drives the rig,
 *  or the other one to compare, counted as a live run or as Target
 *  Scheduler's preview. The timeline and the plan table both read it. */
function useNightSchedule(rig: RigNights, night: RigNight, range: Props['range'], enabled: boolean, draft: NightOverride[] | null, tsDraft: TsChanges | null) {
  const index = rig.nights.findIndex(n => n.date === night.date);
  // The rig's own runtime, counted as a live run.
  const shown = rig.runtime;
  const now = useDirectorClock();
  const tonight = isTonight(night, now);
  // Only Director's own schedule of a Director rig takes overrides.
  const previewed = rig.runtime === 'director' && shown === 'director' ? draft : null;
  // Only Target Scheduler's own schedule of its rig takes its changes.
  const tsPreviewed = rig.runtime === 'target_scheduler' && shown === 'target_scheduler' ? tsDraft : null;
  const query = useQuery({
    queryKey: ['nightPlanDetail', rig.rig.id, range.first, range.count, night.date, previewed, tsPreviewed],
    queryFn: () => apiClient.getNightPlans({
      rigs: [rig.rig.id],
      ...(range.first ? { first_night: range.first } : {}),
      nights: range.count,
      schedule_nights: index + 1,
      explain: night.date,
      ...(previewed ? { override_draft: { rig: rig.rig.id, date: night.date, entries: previewed } } : {}),
      ...(tsPreviewed ? { ts_draft: { rig: rig.rig.id, changes: tsPreviewed } } : {}),
    }),
    enabled: enabled && index >= 0,
    retry: retryWhenBusy,
    retryDelay: 1200,
    // Tonight re-plans from now every two minutes; other nights stand
    // until the page asks again or a mark changes them.
    refetchInterval: tonight ? 2 * 60_000 : false,
    refetchOnWindowFocus: false,
    // An edit keeps its last schedule on screen while the next is planned.
    placeholderData: previous => previous,
  });
  const detail = query.data?.rigs.find(entry => entry.rig.id === rig.rig.id);
  // The grid's schedule is the rig's own runtime, counted as a live run,
  // with the night's saved overrides: an unsaved edit is not the grid's.
  const asGrid = previewed === null && tsPreviewed === null;
  return { shown, now, tonight, query, detail, asGrid };
}

type ScheduleView = ReturnType<typeof useNightSchedule>;

/** The night's schedule from the runtime that drives the rig, or the other
 *  one to compare, with the planner's reasons for each block. */
function NightSchedule({ rig, night, colours, view, edit, moveProblem, saved, onEdit }: {
  rig: RigNights; night: RigNight; colours: Map<string, number>; view: ScheduleView;
  /** While the night's overrides are edited: what the timeline edits. */
  edit?: TimelineEdit;
  /** Why the last move was refused. */
  moveProblem: string | null;
  /** The night's saved overrides, for the chips above the timeline. */
  saved: NightOverride[];
  /** Start editing the night's overrides, when the person may. */
  onEdit?: () => void;
}) {
  const { shown, now, tonight, query, detail } = view;
  const scheduled = detail?.schedule?.nights.find(n => n.date === night.date);
  const goals = detail?.schedule?.goals ?? [];
  const zone = usableZone(rig.time_zone);
  const bands = scheduled?.overrides ? overrideBands(scheduled.overrides.entries, goals, { start_ms: scheduled.start_ms, end_ms: scheduled.end_ms }, zone) : [];
  // Tonight: what the rig says it is doing, beside what it is planned to do.
  const statuses = useQuery({ queryKey: ['directorRigStatuses'], queryFn: apiClient.getDirectorRigStatuses, enabled: tonight, retry: retryWhenBusy, retryDelay: 1200, refetchInterval: 15_000 });
  const live = statuses.data?.find(status => status.rig.id === rig.rig.id);
  return <section className="nights-schedule" aria-label="Schedule">
    <div className="nights-mark-row">
      <h4>{RUNTIME_LABEL[shown]} schedule</h4>
      {onEdit && <button type="button" className="is-primary" onClick={onEdit}>Edit night</button>}
    </div>
    <p className="director-muted">{shown === 'target_scheduler'
      ? `A prediction of Target Scheduler ${detail?.schedule?.assumptions.target_scheduler_version ?? '5'}'s choices from the rig's database. Target Scheduler decides on the rig. Change a project's priority or whether it is active under Target Scheduler settings, or its limits and rule weights in Target Scheduler, to change this prediction.`
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
    {edit && <p className="director-muted">Choose a block for what to do with it, or drag it. Drag a pin or its edges, and drag the hold line in from the right. Nothing changes on the rig until the overrides are saved.</p>}
    {edit && query.isFetching && <p className="director-muted" role="status">Planning the edit...</p>}
    {moveProblem && <p className="director-error" role="alert">{moveProblem}</p>}
    {shown === 'director' && rig.runtime === 'director' && <OverrideChips entries={edit ? edit.entries : saved} goals={goals} zone={zone}
      editing={!!edit} onChange={entries => edit?.onChange(entries)} />}
    {scheduled && <NightTimeline night={night} scheduled={scheduled} colours={colours} now={tonight ? now : undefined}
      cloudThreshold={rig.forecast?.cloud_threshold_percent} zone={zone} bands={bands} edit={edit} />}
  </section>;
}

function NightFacts({ rig, night }: { rig: RigNights; night: RigNight }) {
  const { availability } = night;
  return <>
    <dl className="nights-facts">
      <div><dt>Dark</dt><dd>{night.dusk_ms !== null && night.dawn_ms !== null ? `${siteClock(rig, night.dusk_ms)} – ${siteClock(rig, night.dawn_ms)}, ${formatHours(night.dark_hours)}` : 'No darkness'}</dd></div>
      <div><dt>Clear</dt><dd>{availability.state === 'limited'
        ? availability.windows.map(w => `${siteClock(rig, w.start_ms)} – ${siteClock(rig, w.end_ms)}`).join(', ') + `, ${formatHours(night.available_dark_hours)} dark`
        : availability.state === 'unavailable' ? 'Not available' : `All night, ${formatHours(night.available_dark_hours)} dark`}</dd></div>
      <div><dt>Moon</dt><dd>{Math.round(night.moon_illumination * 100)}% lit, up {formatHours(night.moon_hours_up_in_dark)} of the dark</dd></div>
      <div><dt>Load</dt><dd>{describeLoad(night)}</dd></div>
    </dl>
    {availability.marks.length > 0 && <ul className="nights-marks">{availability.marks.map(mark => <li key={mark.scope}>
      {markOrigin(mark)}: {STATE_LABEL[mark.state]}
      {mark.state === 'limited' && ` (${mark.windows.map(w => `${siteClock(rig, w.start_ms)} – ${siteClock(rig, w.end_ms)}`).join(', ')})`}
      {mark.note && <span className="director-muted"> · {mark.note}</span>}
    </li>)}</ul>}
    {rig.forecast && <ForecastNote rig={rig} night={night} />}
  </>;
}

/** What the site's forecast shows for the night, or why it shows nothing,
 *  with the credit its provider's licence requires. */
function ForecastNote({ rig, night }: { rig: RigNights; night: RigNight }) {
  const forecast = rig.forecast!;
  const provider = PROVIDER_LABEL[forecast.provider];
  const said = forecast.fetched_at_ms === null
    ? `${rig.planning_site?.name ?? 'The site'}'s forecast from ${provider} has not arrived or has expired, so nights count as clear.`
    : night.clouds.length === 0
      ? `${provider}'s forecast does not cover this night, so the night counts as clear.`
      : `Hours at or under ${forecast.cloud_threshold_percent}% cloud count as clear, from ${provider}'s forecast fetched ${siteClock(rig, forecast.fetched_at_ms)}. A mark set for the site overrides the forecast.`;
  return <p className="director-muted nights-forecast-note">
    {said} <a href={PROVIDER_URL[forecast.provider]} target="_blank" rel="noreferrer">{forecast.attribution}</a>
  </p>;
}

function describeLoad(night: RigNight): string {
  const { demand_hours, available_hours, idle_hours, overbooked_hours } = night.load;
  if (demand_hours <= 0) return available_hours > 0 ? `Nothing owed fits; ${formatHours(available_hours)} idle` : 'Nothing owed fits';
  const owed = `${formatHours(demand_hours)} of owed work fits, against ${formatHours(available_hours)} clear`;
  if (overbooked_hours >= 0.05) return `${owed}: overbooked by ${formatHours(overbooked_hours)}`;
  if (idle_hours >= 0.05) return `${owed}: ${formatHours(idle_hours)} idle`;
  return owed;
}

/**
 * The plans the night applies to: what the shown schedule shoots, then
 * what is up and clear but not planned. Targets the night does not apply
 * to fold into a list, each with the reason.
 */
function PlanHours({ rig: grid, night: gridNight, colours, view }: { rig: RigNights; night: RigNight; colours: Map<string, number>; view: ScheduleView }) {
  // The night's own plan when it is read, which unsaved Target Scheduler
  // changes are planned into; the grid's until then.
  const rig = view.detail ?? grid;
  const night = rig.nights.find(n => n.date === gridNight.date) ?? gridNight;
  // The shown schedule, or while it is computed, the grid's when it is the same.
  const schedule = view.detail ? view.detail.schedule : view.asGrid ? grid.schedule : null;
  const scheduled = !!schedule?.nights.some(n => n.date === night.date);
  // The grid's schedule runs the whole range, so it can say when a plan is done.
  const doneBy = view.asGrid ? grid.schedule : null;
  if (rig.plans.length === 0) return <p className="director-muted">This rig owes no frames.</p>;
  const { applies, out } = planRows(rig, night, schedule);
  const planned = applies.filter(row => row.status === 'planned').length;
  const notPlanned = applies.length - planned;
  const targets = (count: number) => `${count} target${count === 1 ? '' : 's'}`;
  const heading = applies.length === 0 ? 'No owed target can be imaged this night'
    : scheduled ? `This night: ${[planned && `${targets(planned)} planned`, notPlanned && `${planned ? notPlanned : targets(notPlanned)} not planned`].filter(Boolean).join(', ')}`
    : `This night: ${targets(applies.length)} up and clear`;
  const planName = (row: PlanRow) => <><span className="nights-plan-dot" data-colour={colours.get(row.plan.name) ?? 0} aria-hidden="true" />{row.plan.name}</>;
  const frames = (count: number) => `${count} frame${count === 1 ? '' : 's'}`;
  const headingId = `nights-plans-${rig.rig.id}-${night.date}`;
  return <section className="nights-plans" aria-labelledby={headingId}>
    <h4 id={headingId}>{heading}</h4>
    {view.tonight && applies.length > 0 && <p className="director-muted">Usable hours count the whole night. Tonight's schedule plans only from now.</p>}
    {applies.length > 0 && <div className="director-table-scroll"><table className="nights-plan-table">
      <thead><tr><th>Plan</th><th>Target</th>{scheduled && <th>Planned</th>}<th>Usable</th><th>Peak</th><th>Owed</th>{doneBy && <th>Done by</th>}</tr></thead>
      <tbody>{applies.map(row => {
        const done = doneBy ? completesOn(doneBy.progress.filter(p => p.project_id === row.plan.project_id && p.target_id === row.target.target_id)) : null;
        const blocked = row.hours?.hours_moon_blocked ?? 0;
        return <tr key={`${row.plan.project_id}:${row.target.target_id}`} className={scheduled && row.status !== 'planned' ? 'is-idle-plan' : undefined}>
          <td>{planName(row)}{row.plan.priority !== 'normal' && <span className="director-muted"> · {row.plan.priority} priority</span>}</td>
          <td>{row.target.name}<span className="director-muted"> · {row.target.filters.join(', ')}</span></td>
          {scheduled && <td>{row.planned > 0 ? frames(row.planned) : <span className="director-muted">not planned</span>}</td>}
          <td>{formatHours(row.usable)}{blocked > 0 && <span className="director-muted"> · {formatHours(blocked)} excluded by the Moon</span>}</td>
          <td>{row.hours && row.hours.max_altitude_degrees > -90 ? `${Math.round(row.hours.max_altitude_degrees)}°${row.hours.transit_ms ? ` at ${siteClock(rig, row.hours.transit_ms)}` : ''}` : '—'}</td>
          <td>{formatHours(row.target.hours_owed)}<span className="director-muted"> · {frames(row.target.frames_owed)}</span></td>
          {doneBy && <td>{done === null || done < 0
            ? <span className="director-muted">not within these {doneBy.nights.length} nights</span>
            : formatNightDate(doneBy.nights[done]?.date ?? night.date)}</td>}
        </tr>;
      })}</tbody>
    </table></div>}
    {out.length > 0 && <details className="nights-other-plans" open={applies.length === 0}>
      <summary>Not this night: {targets(out.length)}</summary>
      <ul>{out.map(row => <li key={`${row.plan.project_id}:${row.target.target_id}`}>
        {planName(row)} · {row.target.name}: <span className="director-muted">{row.status === 'done'
          ? `plan done ${row.done_on ? `by ${formatNightDate(row.done_on)}` : 'before these nights'}`
          : OUT_REASON[row.status as keyof typeof OUT_REASON]}</span>
      </li>)}</ul>
    </details>}
  </section>;
}

function failure(error: unknown): string {
  if (httpStatus(error) === 409) return 'These marks changed since the page loaded. The nights have been read again; mark them once more.';
  return error instanceof Error ? error.message : 'The marks could not be saved';
}

/** Mark the chosen nights available, out, or clear for some hours, for the
 *  rig alone or for every rig at its site. */
function MarkEditor({ rig, nights }: { rig: RigNights; nights: RigNight[] }) {
  const [scope, setScope] = useState<NightMarkScope>('rig');
  // A form per scope, so each starts from that scope's own mark.
  return <MarkForm key={scope} rig={rig} nights={nights} scope={scope} onScope={setScope} />;
}

/** All the queries a mark changes: the grid, the open night and Live's
 *  Tonight column. */
const MARKED_QUERIES = [['nightPlans'], ['nightPlanDetail'], ['nightPlansTonight'], ['directorProjection'], ['directorFeasibility']] as const;

function MarkForm({ rig, nights, scope, onScope }: { rig: RigNights; nights: RigNight[]; scope: NightMarkScope; onScope: (scope: NightMarkScope) => void }) {
  const client = useQueryClient();
  const first = nights[0];
  // The form edits marks made by hand; the forecast's are not marks to clear.
  const current = first.availability.marks.find(mark => mark.scope === scope && mark.source.kind === 'manual');
  const [state, setState] = useState<NightAvailabilityState>(current?.state ?? 'unavailable');
  const limited = current?.state === 'limited' ? current.windows[0] : undefined;
  const extra = current?.state === 'limited' ? current.windows.slice(1) : [];
  const zone = usableZone(rig.time_zone);
  const [from, setFrom] = useState(limited ? clockValue(limited.start_ms, zone) : '');
  const [to, setTo] = useState(limited ? clockValue(limited.end_ms, zone) : '');
  const [note, setNote] = useState(current?.note ?? '');
  const [problem, setProblem] = useState<string | null>(null);
  const scopeId = scope === 'site' ? rig.planning_site?.id ?? null : rig.rig.id;
  const revision = scope === 'site' ? rig.mark_revisions.site ?? 0 : rig.mark_revisions.rig;
  const marked = nights.some(night => night.availability.marks.some(mark => mark.scope === scope && mark.source.kind === 'manual'));
  const refresh = () => { for (const queryKey of MARKED_QUERIES) void client.invalidateQueries({ queryKey: [...queryKey] }); };
  const done = () => { setProblem(null); refresh(); };
  const fail = (error: unknown) => {
    setProblem(failure(error));
    if (httpStatus(error) === 409) refresh();
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
      const hours = windowFor(night, from, to, zone);
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
        <span id="nights-limited-help" className="director-muted">Times are in {zone ? `the site's time zone (${zone})` : `this browser's time zone (${BROWSER_ZONE})`}. Leave From empty to start at dusk, or To empty to end at dawn. Times after midnight fall the next morning.</span>
      </div>}
      {state === 'limited' && extra.length > 0 && <p className="director-muted" role="note">
        This mark also has {extra.map(w => `${siteClock(rig, w.start_ms)} – ${siteClock(rig, w.end_ms)}`).join(', ')}. Saving keeps only the hours entered here.
      </p>}
      <div className="nights-mark-row">
        <label>For <select value={scope} onChange={event => onScope(event.target.value as NightMarkScope)}>
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
