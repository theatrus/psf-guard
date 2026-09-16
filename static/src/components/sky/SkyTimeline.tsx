import { useMemo } from 'react';
import { Pause, Play } from 'lucide-react';
import { filterColor } from '../../utils/filterColors';
import { moonIllumination } from '../../utils/skyProjection';
import { formatHours, formatNight, type Lane } from './skyModel';

interface Props {
  lanes: Lane[];
  /** The nights the selected rigs captured on, sorted. */
  nightKeys: string[];
  fromNight: string | null;
  asOfNight: string | null;
  onRange: (fromNight: string | null, asOfNight: string | null) => void;
  playing: boolean;
  onTogglePlay: () => void;
}

const WIDTH = 1000;
const LABEL_WIDTH = 150;
const LANE_HEIGHT = 44;
const MOON_HEIGHT = 10;
const AXIS_HEIGHT = 22;

function dayOf(night: string): number {
  const [y, m, d] = night.split('-').map(Number);
  return Math.floor(Date.UTC(y, m - 1, d) / 86400000);
}

export default function SkyTimeline({ lanes, nightKeys, fromNight, asOfNight, onRange, playing, onTogglePlay }: Props) {
  const span = useMemo(() => {
    if (nightKeys.length === 0) return null;
    const first = dayOf(nightKeys[0]);
    const last = dayOf(nightKeys[nightKeys.length - 1]);
    return { first, last, days: last - first + 1 };
  }, [nightKeys]);

  const plotWidth = WIDTH - LABEL_WIDTH - 8;
  const height = lanes.length * LANE_HEIGHT + MOON_HEIGHT + AXIS_HEIGHT + 8;

  const geometry = useMemo(() => {
    if (!span) return null;
    const columnWidth = plotWidth / span.days;
    const xOf = (night: string) => LABEL_WIDTH + (dayOf(night) - span.first) * columnWidth;
    let maxNightSeconds = 0;
    for (const lane of lanes) {
      for (const slot of lane.perNight.values()) {
        maxNightSeconds = Math.max(maxNightSeconds, slot.seconds);
      }
    }
    const months: Array<{ x: number; label: string }> = [];
    for (let day = span.first; day <= span.last; day += 1) {
      const date = new Date(day * 86400000);
      if (date.getUTCDate() === 1 || day === span.first) {
        const label =
          date.getUTCMonth() === 0 || day === span.first
            ? date.toLocaleDateString(undefined, { month: 'short', year: 'numeric', timeZone: 'UTC' })
            : date.toLocaleDateString(undefined, { month: 'short', timeZone: 'UTC' });
        months.push({ x: LABEL_WIDTH + (day - span.first) * columnWidth, label });
      }
    }
    // Thin the month labels when the span is long enough that they collide.
    const minGap = 46;
    const kept: typeof months = [];
    for (const month of months) {
      if (kept.length === 0 || month.x - kept[kept.length - 1].x >= minGap) kept.push(month);
    }
    const moon: Array<{ x: number; width: number; illumination: number }> = [];
    for (let day = span.first; day <= span.last; day += 1) {
      moon.push({
        x: LABEL_WIDTH + (day - span.first) * columnWidth,
        width: Math.max(columnWidth, 0.8),
        illumination: moonIllumination((day + 0.5) * 86400),
      });
    }
    return { columnWidth, xOf, maxNightSeconds, months: kept, moon };
  }, [span, lanes, plotWidth]);

  if (!span || !geometry) {
    return null;
  }

  const last = nightKeys.length - 1;
  const toIndex = asOfNight ? Math.max(0, nightKeys.indexOf(asOfNight)) : last;
  const fromIndex = fromNight ? Math.max(0, nightKeys.indexOf(fromNight)) : 0;
  const cursorX = geometry.xOf(nightKeys[toIndex]) + geometry.columnWidth;
  const startX = geometry.xOf(nightKeys[fromIndex]);
  const barWidth = Math.max(geometry.columnWidth - 0.6, 1);
  const years = [...new Set(nightKeys.map((night) => night.slice(0, 4)))];
  const setFrom = (index: number) => {
    const bounded = Math.min(index, toIndex);
    onRange(bounded <= 0 ? null : nightKeys[bounded], asOfNight);
  };
  const setTo = (index: number) => {
    const bounded = Math.max(index, fromIndex);
    onRange(fromNight, bounded >= last ? null : nightKeys[bounded]);
  };
  const fromYear = (year: string) => {
    const first = nightKeys.findIndex((night) => night >= `${year}-01-01`);
    onRange(first <= 0 ? null : nightKeys[first], null);
  };

  return (
    <div className="sky-timeline">
      <div className="sky-timeline-head">
        <button
          type="button"
          className="sky-play"
          onClick={onTogglePlay}
          aria-label={playing ? 'Pause the replay' : 'Replay the nights in order'}
        >
          {playing ? <Pause size={16} /> : <Play size={16} />}
        </button>
        <div className="sky-range" style={{ ['--from' as string]: `${(100 * fromIndex) / Math.max(1, last)}%`, ['--to' as string]: `${(100 * toIndex) / Math.max(1, last)}%` }}>
          <input
            className="sky-scrubber sky-scrubber-from"
            type="range"
            min={0}
            max={Math.max(0, last)}
            value={fromIndex}
            aria-label="Start the replay from this night"
            onChange={(event) => setFrom(Number(event.target.value))}
          />
          <input
            className="sky-scrubber sky-scrubber-to"
            type="range"
            min={0}
            max={Math.max(0, last)}
            value={toIndex}
            aria-label="Show the sky as it was covered by this night"
            onChange={(event) => setTo(Number(event.target.value))}
          />
        </div>
        <div className="sky-timeline-asof" aria-live="polite">
          {fromNight || asOfNight ? (
            <>
              {fromNight ? (
                <>
                  from <strong>{formatNight(fromNight)}</strong>
                </>
              ) : (
                'from the start'
              )}
              {' · '}
              {asOfNight ? (
                <>
                  as of <strong>{formatNight(asOfNight)}</strong>
                </>
              ) : (
                'to the latest night'
              )}
            </>
          ) : (
            <>
              every night, <strong>{formatNight(nightKeys[0])}</strong> to <strong>{formatNight(nightKeys[last])}</strong>
            </>
          )}
        </div>
      </div>
      {years.length > 1 && (
        <div className="sky-timeline-years" aria-label="Start year">
          <button type="button" className={`sky-chip${fromNight ? '' : ' is-on'}`} onClick={() => onRange(null, asOfNight)}>
            From the start
          </button>
          {years.map((year) => (
            <button
              key={year}
              type="button"
              className={`sky-chip${fromNight?.startsWith(year) && fromNight === nightKeys.find((night) => night >= `${year}-01-01`) ? ' is-on' : ''}`}
              onClick={() => fromYear(year)}
            >
              From {year}
            </button>
          ))}
        </div>
      )}
      <svg className="sky-timeline-plot" viewBox={`0 0 ${WIDTH} ${height}`} role="img" aria-label="Integration per night and rig">
        {lanes.map((lane, laneIndex) => {
          const top = laneIndex * LANE_HEIGHT;
          return (
            <g key={lane.db_id} className={`sky-lane${lane.aggregate ? ' is-aggregate' : ''}`}>
              <rect className="sky-lane-bg" x={LABEL_WIDTH} y={top} width={plotWidth} height={LANE_HEIGHT - 4} />
              <text className="sky-lane-name" x={0} y={top + 17}>
                {lane.db_name}
              </text>
              <text className="sky-lane-total" x={0} y={top + 33}>
                {formatHours(lane.seconds / 3600)}
              </text>
              {[...lane.perNight.entries()].map(([night, slot]) => {
                const x = geometry.xOf(night);
                const fullHeight = (LANE_HEIGHT - 6) * (slot.seconds / geometry.maxNightSeconds);
                let y = top + LANE_HEIGHT - 4;
                const dim = (asOfNight != null && night > asOfNight) || (fromNight != null && night < fromNight);
                return (
                  <g key={night} className={dim ? 'sky-night is-future' : 'sky-night'}>
                    <title>{`${lane.db_name} · ${formatNight(night)} · ${formatHours(slot.seconds / 3600)}`}</title>
                    {[...slot.byFilter.entries()].map(([filter, seconds]) => {
                      const h = fullHeight * (seconds / slot.seconds);
                      y -= h;
                      return <rect key={filter} x={x} y={y} width={barWidth} height={h} fill={filterColor(filter)} />;
                    })}
                  </g>
                );
              })}
            </g>
          );
        })}
        <g className="sky-moon" transform={`translate(0, ${lanes.length * LANE_HEIGHT})`}>
          <text className="sky-lane-name" x={0} y={MOON_HEIGHT - 1}>
            Moon
          </text>
          {geometry.moon.map((day) => (
            <rect key={day.x} x={day.x} y={0} width={day.width} height={MOON_HEIGHT} fillOpacity={0.08 + 0.7 * day.illumination} />
          ))}
        </g>
        <g className="sky-axis" transform={`translate(0, ${lanes.length * LANE_HEIGHT + MOON_HEIGHT})`}>
          {geometry.months.map((month) => (
            <g key={month.x}>
              <line x1={month.x} x2={month.x} y1={0} y2={5} />
              <text x={month.x + 2} y={16}>
                {month.label}
              </text>
            </g>
          ))}
        </g>
        {fromNight && <line className="sky-cursor sky-cursor-from" x1={startX} x2={startX} y1={0} y2={lanes.length * LANE_HEIGHT + MOON_HEIGHT} />}
        <line className="sky-cursor" x1={cursorX} x2={cursorX} y1={0} y2={lanes.length * LANE_HEIGHT + MOON_HEIGHT} />
      </svg>
    </div>
  );
}

