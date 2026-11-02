import { useState } from 'react';
import type { ForecastHour } from '../../api/directorTypes';
import type { RigNight, ScheduleBlock, ScheduleGapReason, ScheduleRule, ScheduleTargetReason, ScheduledNight } from '../../api/nightPlanTypes';
import { formatHours } from '../director/visibilityFormat';
import { formatClock } from './nightsModel';

const HOUR_MS = 3_600_000;

const GAP_LABEL: Record<ScheduleGapReason, string> = {
  twilight: 'Twilight',
  unavailable: 'Not available',
  moon: 'Moon',
  meridian: 'Meridian',
  below_limits: 'Below limits',
  complete: 'All done',
  nothing_eligible: 'Nothing fits',
};

const GAP_HELP: Record<ScheduleGapReason, string> = {
  twilight: 'Too light for any template owed tonight.',
  unavailable: "Outside the night's clear hours, set by a mark or the site's forecast.",
  moon: 'The targets are up, but their Moon avoidance rules turn them down.',
  meridian: 'Waiting out the meridian flip, or outside a meridian window.',
  below_limits: 'Nothing owed is above its horizon and limits for its minimum time.',
  complete: 'Every plan has its frames.',
  nothing_eligible: 'Plans are owed, but none can start before the night or the clear hours end.',
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
}

/**
 * One night of a rig's schedule, from an hour before dusk to an hour after
 * dawn: a block per stretch on a target, its filter runs inside, and the
 * gaps with the reason nothing is shot. Choose a block to see why the
 * planner chose its target.
 */
export default function NightTimeline({ night, scheduled, colours, now, cloudThreshold }: Props) {
  const [chosen, setChosen] = useState<number | null>(null);
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
  const block = chosen !== null ? scheduled.blocks[chosen] : null;
  const reasons = [...new Set(scheduled.gaps.filter(gap => gap.end_ms > start && gap.start_ms < end).map(gap => gap.reason))];
  return <div className="nights-timeline-box">
    <div className="nights-timeline" role="group" aria-label="Schedule">
      <div className="nights-timeline-track">
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
          return <button key={`block-${item.start_ms}`} type="button" className="nights-block" style={style}
            data-colour={colours.get(item.target.project_name) ?? 0} aria-pressed={chosen === index}
            aria-label={`${item.target.project_name}, ${item.target.target_name}: ${formatClock(item.start_ms)} – ${formatClock(item.end_ms)}, ${frames} frames`}
            title={`${item.target.target_name} · ${formatClock(item.start_ms)} – ${formatClock(item.end_ms)} · ${runSummary(item)}`}
            onClick={() => setChosen(chosen === index ? null : index)}>
            {item.runs.map(run => {
              const inner = place(run.start_ms, run.end_ms);
              const share = (run.end_ms - run.start_ms) / Math.max(1, item.end_ms - item.start_ms);
              return inner && <span key={run.start_ms} className="nights-run" style={{ width: `${share * 100}%` }}>{share * (item.end_ms - item.start_ms) / span > 0.04 ? run.filter : ''}</span>;
            })}
          </button>;
        })}
        {now !== undefined && now > start && now < end && <span className="nights-now" style={{ left: `${((now - start) / span) * 100}%` }} title={`Now, ${formatClock(now)}`} />}
      </div>
      {night.clouds.length > 0 && <div className="nights-clouds" role="group" aria-label="Cloud cover">
        {night.clouds.map(hour => {
          const style = place(hour.start_ms, hour.end_ms);
          if (!style) return null;
          const cloudy = cloudThreshold !== undefined && hour.cloud_total > cloudThreshold;
          return <span key={hour.start_ms} className={`nights-cloud${cloudy ? ' is-cloudy' : ''}`}
            style={{ ...style, opacity: 0.12 + 0.88 * hour.cloud_total / 100 }}
            title={`${formatClock(hour.start_ms)} – ${formatClock(hour.end_ms)}: ${cloudWords(hour)}`} />;
        })}
      </div>}
      <div className="nights-timeline-axis" aria-hidden="true">
        {ticks.map(t => <span key={t} style={{ left: `${((t - start) / span) * 100}%` }}>{formatClock(t)}</span>)}
      </div>
    </div>
    {(reasons.length > 0 || night.clouds.length > 0) && <ul className="nights-gap-legend" aria-label="Legend">
      {reasons.map(reason => <li key={reason}><span className="nights-gap-key" data-reason={reason} aria-hidden="true" />{GAP_LABEL[reason]}: {GAP_HELP[reason]}</li>)}
      {night.clouds.length > 0 && <li><span className="nights-cloud-key" aria-hidden="true" />
        Cloud cover: the lighter an hour, the more cloud.{cloudThreshold !== undefined && ` Hours above the site's ${cloudThreshold}% threshold have a red edge.`}
      </li>}
    </ul>}
    {scheduled.note && <p className="director-muted" role="note">{scheduled.note}</p>}
    {scheduled.blocks.length === 0 && <p className="director-muted">Nothing is scheduled this night.</p>}
    {block && <BlockReasons block={block} />}
  </div>;
}

/** An hour's cloud in words: the total, then each layer the provider gives. */
function cloudWords(hour: ForecastHour): string {
  const layers = ([['low', hour.cloud_low], ['mid', hour.cloud_mid], ['high', hour.cloud_high]] as const)
    .filter(([, value]) => value !== null)
    .map(([name, value]) => `${name} ${Math.round(value!)}%`);
  return `${Math.round(hour.cloud_total)}% cloud${layers.length ? ` (${layers.join(', ')})` : ''}`;
}

function runSummary(block: ScheduleBlock): string {
  const parts = block.runs.reduce<Map<string, number>>((map, run) => map.set(run.filter, (map.get(run.filter) ?? 0) + run.frames), new Map());
  return [...parts].map(([filter, frames]) => `${filter} ×${frames}`).join(', ');
}

/** Why the planner chose a block's target: the candidates it looked at,
 *  and the rule scores of those it scored. */
function BlockReasons({ block }: { block: ScheduleBlock }) {
  const { decision } = block;
  const scored = decision.candidates.filter(candidate => candidate.score);
  const rules = [...new Set(scored.flatMap(candidate => candidate.score!.rules.map(rule => rule.rule)))];
  const exposure = block.runs.reduce((sum, run) => sum + run.frames * run.exposure_seconds, 0) / 3600;
  return <section className="nights-reasons" aria-label="Why this target">
    <h4>{block.target.project_name} · {block.target.target_name}</h4>
    <p>{formatClock(block.start_ms)} – {formatClock(block.end_ms)}: {runSummary(block)}, {formatHours(exposure)} of exposure{block.slew ? ', after a slew and centre' : ''}.</p>
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
