import { Moon } from 'lucide-react';
import { useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent } from 'react';
import type { ForecastHour } from '../../api/directorTypes';
import type { RigNight, RunMoon, ScheduleBlock, ScheduleGapReason, ScheduleRule, ScheduleTargetReason, ScheduledNight } from '../../api/nightPlanTypes';
import { formatHours } from '../director/visibilityFormat';
import { airWords, dewAlpha, dewSpread, formatClock as formatBrowserClock, hourSpans, isDewy, moonCourse, moonCourseWords, moonGradient } from './nightsModel';
import type { OverrideBand } from './overridesModel';
import { BlockMenu, EditableBands, HoldHandle, SNAP_MS, STEP_MS, type TimelineEdit, type TimelineScale } from './TimelineEditing';

const HOUR_MS = 3_600_000;

const GAP_LABEL: Record<ScheduleGapReason, string> = {
  twilight: 'Twilight',
  unavailable: 'Not available',
  moon: 'Moon',
  meridian: 'Meridian',
  below_limits: 'Below limits',
  complete: 'All done',
  nothing_eligible: 'Nothing fits',
  held: 'Held',
  excluded: 'Left out',
};

const GAP_HELP: Record<ScheduleGapReason, string> = {
  twilight: 'Too light to shoot.',
  unavailable: 'Outside the clear hours.',
  moon: 'Moon avoidance rules out every target that is up.',
  meridian: 'Waiting for the meridian flip, or outside a meridian window.',
  below_limits: 'No target is above its limits for long enough.',
  complete: 'Every target has its frames.',
  nothing_eligible: 'No target can start before the clear hours end.',
  held: 'An override stops the rig from starting work.',
  excluded: 'Overrides leave out every target with frames left.',
};

const BAND_LABEL: Record<OverrideBand['kind'], string> = {
  pin: 'Pinned',
  exclude: 'Left out',
  hold: 'Held',
};

const REASON_LABEL: Record<ScheduleTargetReason, string> = {
  complete: 'Complete',
  never_rises: 'Never rises',
  not_visible: 'Not up for its minimum time',
  not_yet_visible: 'Not up yet',
  max_altitude: 'Above its maximum altitude',
  meridian_window_clipped: 'Outside its meridian window',
  before_meridian_window: 'Before its meridian window',
  meridian_flip_clipped: 'Meridian flip zone',
  twilight: 'Twilight',
  moon_avoidance: 'Moon avoidance',
  lower_score: 'Lower score',
  all_exposure_plans: 'Every exposure plan set aside',
  project: 'Its project is set aside',
  not_ready: 'Not ready yet',
};

const RULE_LABEL: Record<ScheduleRule, string> = {
  meridian_flip_penalty: 'Meridian Flip Penalty',
  meridian_window_priority: 'Meridian Window Priority',
  mosaic_completion: 'Mosaic Completion',
  percent_complete: 'Percent Complete',
  project_priority: 'Project Priority',
  setting_soonest: 'Setting Soonest',
  smart_exposure_order: 'Smart Exposure Order',
  target_switch_penalty: 'Target Switch Penalty',
};

const DECISION_LABEL = {
  only: 'The only target ready',
  scored: 'The highest score of the targets ready',
  continued: 'Stayed with the previous target inside its minimum time',
  ranked: "Director's highest-priority goal that fits now",
} as const;

interface Props {
  night: RigNight;
  scheduled: ScheduledNight;
  colours: Map<string, number>;
  /** Draw a line at this instant: the night under way. */
  now?: number;
  /** The site's clear-sky threshold, when it takes a forecast: hours above
   *  it are drawn as cloudy. */
  cloudThreshold?: number;
  /** The site's dew limit, in °C: hours this close to the dew point are
   *  drawn as dewy. */
  dewLimit?: number;
  /** The site's time zone; the browser's without one. */
  zone?: string;
  /** The stretches the night's saved or planned overrides cover. */
  bands?: OverrideBand[];
  /** While the night's overrides are edited: what they are, and what each
   *  change on the timeline does. Blocks then move by drag or the arrow
   *  keys and open a menu, and the overrides become bands to drag. */
  edit?: TimelineEdit;
}

/**
 * One night of a rig's schedule, from an hour before dusk to an hour after
 * dawn: a block per stretch on a target, its filter runs inside, and the
 * gaps with the reason nothing is shot. Choose a block to see why the
 * planner chose its target.
 */
export default function NightTimeline({ night, scheduled, colours, now, cloudThreshold, dewLimit, zone, bands = [], edit }: Props) {
  const onMoveBlock = edit?.onMoveBlock;
  const [menu, setMenu] = useState<string | null>(null);
  // Blocks keep their key while a re-plan moves them, so a chosen or
  // dragged block, and the focus on it, stay with it.
  const keys = blockKeys(scheduled.blocks);
  const [chosen, setChosen] = useState<string | null>(null);
  const [drag, setDrag] = useState<{ key: string; x: number; dx: number } | null>(null);
  const track = useRef<HTMLDivElement>(null);
  // Set when a drag moved a block, so the click that ends it chooses nothing.
  const moved = useRef(false);
  const formatClock = (ms: number) => formatBrowserClock(ms, zone);
  const start = (night.dusk_ms ?? scheduled.start_ms) - HOUR_MS;
  const end = (night.dawn_ms ?? scheduled.end_ms) + HOUR_MS;
  const span = Math.max(1, end - start);
  const place = (from: number, to: number) => {
    const left = Math.max(0, (from - start) / span);
    const right = Math.min(1, (to - start) / span);
    return right > left ? { left: `${left * 100}%`, width: `${(right - left) * 100}%` } : null;
  };
  const ticks: number[] = [];
  for (let t = Math.ceil(start / HOUR_MS) * HOUR_MS; t < end; t += HOUR_MS) ticks.push(t);
  const chosenIndex = chosen === null ? -1 : keys.indexOf(chosen);
  const block = chosenIndex >= 0 ? scheduled.blocks[chosenIndex] : null;
  const width = () => track.current?.getBoundingClientRect().width ?? 0;
  const toMs = (dx: number) => width() > 0 ? Math.round((dx / width()) * span / SNAP_MS) * SNAP_MS : 0;
  const scale: TimelineScale = { start, end, place, toMs };
  const press = (key: string, event: PointerEvent<HTMLButtonElement>) => {
    if (!onMoveBlock) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    setDrag({ key, x: event.clientX, dx: 0 });
  };
  const slide = (event: PointerEvent<HTMLButtonElement>) => {
    if (drag) setDrag({ ...drag, dx: event.clientX - drag.x });
  };
  /** A drag of a snap or more moves the block; anything less is a click. */
  const release = () => {
    if (!drag || !onMoveBlock) return;
    const delta = toMs(drag.dx);
    const index = keys.indexOf(drag.key);
    setDrag(null);
    moved.current = delta !== 0;
    // A block the schedule no longer has is not moved.
    if (delta !== 0 && index >= 0) onMoveBlock(scheduled.blocks[index], delta);
  };
  const nudge = (index: number, event: KeyboardEvent<HTMLButtonElement>) => {
    if (!onMoveBlock || (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft')) return;
    event.preventDefault();
    onMoveBlock(scheduled.blocks[index], event.key === 'ArrowRight' ? STEP_MS : -STEP_MS);
  };
  const summaryId = `nights-clouds-${night.date}`;
  const dewId = `nights-dew-${night.date}`;
  const air = night.clouds.filter(hour => dewSpread(hour) !== null);
  const moon = night.moon_altitudes.degrees.length > 0 ? moonView(night, start, end, formatClock) : null;
  const reasons = [...new Set(scheduled.gaps.filter(gap => gap.end_ms > start && gap.start_ms < end).map(gap => gap.reason))];
  return <div className="nights-timeline-box">
    <div className="nights-timeline" role="group" aria-label="Schedule">
      {edit && <EditableBands edit={edit} scale={scale} />}
      {!edit && bands.length > 0 && <div className="nights-overrides" role="group" aria-label="Overrides">
        {bands.map(band => {
          const style = place(band.start_ms, band.end_ms);
          if (!style) return null;
          return <span key={`${band.kind}-${band.start_ms}-${band.label}`} className="nights-override" data-kind={band.kind} style={style} title={band.label}>
            {BAND_LABEL[band.kind]}
          </span>;
        })}
      </div>}
      <div className="nights-timeline-track" ref={track}>
        {scheduled.gaps.map(gap => {
          const style = place(gap.start_ms, gap.end_ms);
          if (!style) return null;
          const label = `${GAP_LABEL[gap.reason]}, ${formatClock(gap.start_ms)} – ${formatClock(gap.end_ms)}${gap.waiting_for ? `, waiting for ${gap.waiting_for.target_name}` : ''}`;
          return <span key={`gap-${gap.start_ms}`} className="nights-gap" data-reason={gap.reason} style={style} title={`${label}. ${GAP_HELP[gap.reason]}`} />;
        })}
        {scheduled.blocks.map((item, index) => {
          const style = place(item.start_ms, item.end_ms);
          if (!style) return null;
          const frames = item.runs.reduce((sum, run) => sum + run.frames, 0);
          const key = keys[index];
          const dragged = drag?.key === key ? { transform: `translateX(${drag.dx}px)` } : {};
          const moonLine = blockMoon(item);
          const title = `${item.target.target_name} · ${formatClock(item.start_ms)} – ${formatClock(item.end_ms)} · ${runSummary(item)}`;
          return <button key={key} type="button" className="nights-block" style={{ ...style, ...dragged }}
            data-colour={colours.get(item.target.project_name) ?? 0} aria-pressed={chosen === key}
            data-movable={onMoveBlock ? 'true' : undefined}
            aria-label={`${item.target.project_name}, ${item.target.target_name}: ${formatClock(item.start_ms)} – ${formatClock(item.end_ms)}, ${frames} frames${moonLine ? `. ${moonLine}` : ''}${onMoveBlock ? '. Choose it for what to do with it; drag it, or press the left and right arrow keys, to move it 15 minutes at a time.' : ''}`}
            aria-haspopup={edit ? 'menu' : undefined} aria-expanded={edit ? menu === key : undefined}
            title={title}
            onPointerDown={event => press(key, event)}
            onPointerMove={slide}
            onPointerUp={release}
            onPointerCancel={() => setDrag(null)}
            onKeyDown={event => nudge(index, event)}
            onClick={() => {
              if (moved.current) moved.current = false;
              else if (edit) setMenu(menu === key ? null : key);
              else setChosen(chosen === key ? null : key);
            }}>
            {item.runs.map(run => {
              const inner = place(run.start_ms, run.end_ms);
              const share = (run.end_ms - run.start_ms) / Math.max(1, item.end_ms - item.start_ms);
              const labelled = share * (item.end_ms - item.start_ms) / span > 0.04;
              return inner && <span key={run.start_ms} className="nights-run" style={{ width: `${share * 100}%` }}
                title={run.moon ? `${title}\n${run.filter}: ${moonWords(run.moon)}` : undefined}>
                {labelled && <span>{run.filter}</span>}
                {labelled && run.moon && <span className="nights-run-moon" data-down={run.moon.max_altitude_degrees < 0 ? 'true' : undefined}>
                  <Moon size={10} aria-hidden="true" />{Math.round(run.moon.min_separation_degrees)}°
                </span>}
              </span>;
            })}
          </button>;
        })}
        {edit && <HoldHandle edit={edit} scale={scale} width={width} />}
        {now !== undefined && now > start && now < end && <span className="nights-now" style={{ left: `${((now - start) / span) * 100}%` }} title={`Now, ${formatClock(now)}`} />}
      </div>
      {night.clouds.length > 0 && <div className="nights-clouds" role="group" aria-label="Cloud cover" aria-describedby={summaryId}>
        {night.clouds.map(hour => {
          const style = place(hour.start_ms, hour.end_ms);
          if (!style) return null;
          const cloudy = cloudThreshold !== undefined && hour.cloud_total > cloudThreshold;
          return <span key={hour.start_ms} className={`nights-cloud${cloudy ? ' is-cloudy' : ''}`}
            style={{ ...style, '--cloud-alpha': (0.12 + 0.88 * hour.cloud_total / 100).toFixed(2) } as CSSProperties}
            title={`${formatClock(hour.start_ms)} – ${formatClock(hour.end_ms)}: ${cloudWords(hour)}`} />;
        })}
      </div>}
      {air.length > 0 && <div className="nights-dew" role="group" aria-label="Dew" aria-describedby={dewId}>
        {air.map(hour => {
          const style = place(hour.start_ms, hour.end_ms);
          if (!style) return null;
          return <span key={hour.start_ms} className={`nights-dew-hour${isDewy(hour, dewLimit) ? ' is-dewy' : ''}`}
            style={{ ...style, '--dew-alpha': dewAlpha(dewSpread(hour)!) } as CSSProperties}
            title={`${formatClock(hour.start_ms)} – ${formatClock(hour.end_ms)}: ${airWords(hour)}`} />;
        })}
      </div>}
      {moon && <div className="nights-moon-strip" role="img" aria-label={`Moon: ${moon.label}`} title={moon.label}
        style={{ backgroundImage: moon.gradient }} />}
      {edit && menu !== null && (() => {
        const index = keys.indexOf(menu);
        if (index < 0) return null;
        const item = scheduled.blocks[index];
        return <BlockMenu block={item} edit={edit} left={place(item.start_ms, item.end_ms)?.left ?? '0'}
          onReasons={() => setChosen(menu)} onClose={() => setMenu(null)} />;
      })()}
      <div className="nights-timeline-axis" aria-hidden="true">
        {ticks.map(t => <span key={t} style={{ left: `${((t - start) / span) * 100}%` }}>{formatClock(t)}</span>)}
      </div>
    </div>
    {moon && <p className="nights-moon-caption" aria-hidden="true"><Moon size={12} />{moon.label}</p>}
    {(reasons.length > 0 || night.clouds.length > 0) && <ul className="nights-gap-legend" aria-label="Legend">
      {reasons.map(reason => <li key={reason}><span className="nights-gap-key" data-reason={reason} aria-hidden="true" />{GAP_LABEL[reason]}: {GAP_HELP[reason]}</li>)}
      {night.clouds.length > 0 && <li id={summaryId}><span className="nights-cloud-key" aria-hidden="true" />
        {cloudThreshold === undefined ? 'Cloud' : cloudySummary(night.clouds, cloudThreshold, start, end, zone)}
      </li>}
      {air.length > 0 && <li id={dewId}><span className="nights-dew-key" aria-hidden="true" />
        {dewLimit === undefined ? 'Dew' : dewySummary(air, dewLimit, start, end, zone)}
      </li>}
    </ul>}
    {scheduled.note && <p className="director-muted" role="note">{scheduled.note}</p>}
    {scheduled.blocks.length === 0 && <p className="director-muted">Nothing is scheduled this night.</p>}
    {block && <BlockReasons block={block} zone={zone} />}
  </div>;
}

/** A key per block that a re-plan keeps: its target, and how many blocks
 *  on that target come before it. */
function blockKeys(blocks: ScheduleBlock[]): string[] {
  const seen = new Map<string, number>();
  return blocks.map(block => {
    const target = `${block.target.project_id}:${block.target.target_id}`;
    const count = seen.get(target) ?? 0;
    seen.set(target, count + 1);
    return `block-${target}-${count}`;
  });
}

/** The hours above the threshold in the drawn span, the strip's red
 *  edges in words. */
function cloudySummary(hours: ForecastHour[], threshold: number, from: number, to: number, zone?: string): string {
  const spans = hourSpans(hours, hour => hour.cloud_total > threshold, from, to);
  if (spans.length === 0) return `Cloud: none over ${threshold}%`;
  return `Cloud over ${threshold}%: ${listSpans(spans, zone)}`;
}

/** The hours inside the dew limit in the drawn span, the band's red
 *  edges in words. */
function dewySummary(hours: ForecastHour[], limit: number, from: number, to: number, zone?: string): string {
  const spans = hourSpans(hours, hour => isDewy(hour, limit), from, to);
  if (spans.length === 0) return `Dew: none within ${limit} °C`;
  return `Dew within ${limit} °C: ${listSpans(spans, zone)}`;
}

function listSpans(spans: Array<{ start: number; end: number }>, zone?: string): string {
  return spans.map(span => formatBrowserClock(span.start, zone) + ' – ' + formatBrowserClock(span.end, zone)).join(', ');
}

/** An hour's cloud in words: the total, then each layer the provider gives. */
function cloudWords(hour: ForecastHour): string {
  const layers = ([['low', hour.cloud_low], ['mid', hour.cloud_mid], ['high', hour.cloud_high]] as const)
    .filter(([, value]) => value !== null)
    .map(([name, value]) => `${name} ${Math.round(value!)}%`);
  return `${Math.round(hour.cloud_total)}% cloud${layers.length ? ` (${layers.join(', ')})` : ''}`;
}

/** The Moon over a run: the closest it comes to the target, and its
 *  highest altitude. */
function moonWords(moon: RunMoon): string {
  const closest = `Moon ${Math.round(moon.min_separation_degrees)}° away`;
  return moon.max_altitude_degrees < 0 ? `${closest}, down` : `${closest}, ${Math.round(moon.max_altitude_degrees)}° up`;
}

/** A block's Moon, one entry per filter: the closest the Moon comes to the
 *  target and its highest altitude over that filter's runs. Empty without
 *  a Moon. */
function blockMoon(block: ScheduleBlock): string {
  const filters = new Map<string, RunMoon>();
  for (const run of block.runs) {
    if (!run.moon) continue;
    const seen = filters.get(run.filter);
    filters.set(run.filter, seen ? {
      max_altitude_degrees: Math.max(seen.max_altitude_degrees, run.moon.max_altitude_degrees),
      min_separation_degrees: Math.min(seen.min_separation_degrees, run.moon.min_separation_degrees),
    } : run.moon);
  }
  return [...filters].map(([filter, moon]) => `${filter}: ${moonWords(moon)}`).join(' · ');
}

/** The night's Moon across the drawn span: the gradient from its rise to
 *  its set, and its phase and times in words. */
function moonView(night: RigNight, from: number, to: number, formatClock: (ms: number) => string) {
  const course = moonCourse(night.moon_altitudes, night.moon_illumination, from, to);
  return { label: moonCourseWords(night.moon_altitudes, night.moon_illumination, course, formatClock), gradient: moonGradient(course) };
}

function runSummary(block: ScheduleBlock): string {
  const parts = block.runs.reduce<Map<string, number>>((map, run) => map.set(run.filter, (map.get(run.filter) ?? 0) + run.frames), new Map());
  return [...parts].map(([filter, frames]) => `${filter} ×${frames}`).join(', ');
}

/** Why the planner chose a block's target: the candidates it looked at,
 *  and the rule scores of those it scored. */
function BlockReasons({ block, zone }: { block: ScheduleBlock; zone?: string }) {
  const { decision } = block;
  const formatClock = (ms: number) => formatBrowserClock(ms, zone);
  const scored = decision.candidates.filter(candidate => candidate.score);
  const rules = [...new Set(scored.flatMap(candidate => candidate.score!.rules.map(rule => rule.rule)))];
  const moonLine = blockMoon(block);
  const exposure = block.runs.reduce((sum, run) => sum + run.frames * run.exposure_seconds, 0) / 3600;
  return <section className="nights-reasons" aria-label="Why this target">
    <h4>{block.target.project_name} · {block.target.target_name}</h4>
    <p>{formatClock(block.start_ms)} – {formatClock(block.end_ms)}: {runSummary(block)}, {formatHours(exposure)} of exposure{block.slew ? ', after a slew and centre' : ''}.</p>
    {moonLine && <p className="nights-run-moons">{moonLine}</p>}
    <p className="director-muted">{DECISION_LABEL[decision.kind]}{decision.note ? ` (${decision.note})` : ''}.</p>
    {decision.candidates.length > 0 && <div className="director-table-scroll"><table className="nights-plan-table">
      <thead><tr><th>Target</th><th>Where it stood</th>{scored.length > 0 && <th>Score</th>}{rules.map(rule => <th key={rule}>{RULE_LABEL[rule]}</th>)}</tr></thead>
      <tbody>{decision.candidates.map(candidate => <tr key={`${candidate.target.project_id}:${candidate.target.target_id}`}>
        <td>{candidate.target.project_name} · {candidate.target.target_name}</td>
        <td>{candidate.rejected === null ? 'Chosen' : REASON_LABEL[candidate.rejected]}{candidate.could_start_ms ? `, from ${formatClock(candidate.could_start_ms)}` : ''}</td>
        {scored.length > 0 && <td>{candidate.score ? (candidate.score.total * 100).toFixed(1) : '—'}</td>}
        {rules.map(rule => {
          const part = candidate.score?.rules.find(r => r.rule === rule);
          return <td key={rule} title={part ? `${(part.raw * 100).toFixed(0)} × ${part.weight.toFixed(0)}%` : undefined}>{part ? (part.raw * part.weight).toFixed(1) : '—'}</td>;
        })}
      </tr>)}</tbody>
    </table></div>}
  </section>;
}
