import { nightOf } from './observingNight';

/**
 * Where a run of frames in capture order crosses into another observing
 * night or another hour. Images and Sequence draw these behind the cards,
 * in the gaps, so a long wrapped grid still shows where a night ended and
 * the hours passed, and no card moves for them.
 */
export interface TimeBreak {
  kind: 'night' | 'hour';
  /** The night or the hour the frames from here on belong to. */
  label: string;
  /** What the label stands for, in full. */
  title: string;
}

export interface TimeMarks {
  /** By position in the list: the break just before that frame. */
  breaks: ReadonlyMap<number, TimeBreak>;
  /** By position: whether the frame's night is an odd one in the list, so
   *  alternate nights can take a band of their own. */
  altNight: ReadonlySet<number>;
  /** Hours between hour marks, or null when the frames are too sparse for
   *  hour marks to say anything. */
  hourStep: number | null;
}

/** Hour marks come at these spacings, the shortest that keeps this many
 *  frames between marks on a typical stretch of the night. */
const HOUR_STEPS = [1, 2, 3] as const;
const FRAMES_PER_HOUR_MARK = 6;

/**
 * Hours between hour marks for frames taken at `times` (seconds), from the
 * median gap between neighbours in the same night: every hour while frames
 * come every ten minutes or faster, every two or three hours for longer
 * exposures, and none when even three hours would hold too few frames to
 * be worth splitting.
 */
export function hourStepFor(times: readonly (number | null | undefined)[], boundarySeconds: number): number | null {
  const gaps: number[] = [];
  for (let i = 1; i < times.length; i += 1) {
    const a = times[i - 1];
    const b = times[i];
    if (a == null || b == null || a === b) continue;
    if (nightOf(a, boundarySeconds) !== nightOf(b, boundarySeconds)) continue;
    gaps.push(Math.abs(b - a));
  }
  if (gaps.length === 0) return null;
  gaps.sort((a, b) => a - b);
  const median = gaps[Math.floor(gaps.length / 2)];
  const framesPerHour = 3600 / median;
  return HOUR_STEPS.find((step) => framesPerHour * step >= FRAMES_PER_HOUR_MARK) ?? null;
}

function nightLabel(night: string): { label: string; title: string } {
  const [year, month, day] = night.split('-').map(Number);
  const date = new Date(Date.UTC(year, month - 1, day));
  if (Number.isNaN(date.getTime())) return { label: night, title: night };
  const otherYear = year !== new Date().getFullYear();
  return {
    label: date.toLocaleDateString(undefined, {
      month: 'short',
      day: 'numeric',
      ...(otherYear ? { year: 'numeric' } : {}),
      timeZone: 'UTC',
    }),
    title: `Night of ${date.toLocaleDateString(undefined, {
      weekday: 'long',
      year: 'numeric',
      month: 'long',
      day: 'numeric',
      timeZone: 'UTC',
    })}`,
  };
}

/** The hour mark a capture falls under, in local time as the cards show
 *  it: its key and its label, such as `23:00`. */
function hourBucket(seconds: number, step: number): { key: string; label: string } {
  const date = new Date(seconds * 1000);
  const hour = Math.floor(date.getHours() / step) * step;
  const label = `${String(hour).padStart(2, '0')}:00`;
  return { key: `${date.toDateString()} ${label}`, label };
}

/**
 * The night and hour breaks in a list of frames already in the order shown,
 * oldest or newest first. A frame without a capture time breaks nothing and
 * carries its neighbour's night.
 */
export function timeMarks(
  times: readonly (number | null | undefined)[],
  boundarySeconds: number,
): TimeMarks {
  const breaks = new Map<number, TimeBreak>();
  const altNight = new Set<number>();
  const hourStep = hourStepFor(times, boundarySeconds);
  let previousNight: string | null = null;
  let previousHour: string | null = null;
  let nightIndex = 0;
  times.forEach((time, index) => {
    if (time == null) {
      if (nightIndex % 2 === 1) altNight.add(index);
      return;
    }
    const night = nightOf(time, boundarySeconds);
    const hour = hourStep ? hourBucket(time, hourStep) : null;
    if (previousNight !== null && night !== previousNight) {
      nightIndex += 1;
      breaks.set(index, { kind: 'night', ...nightLabel(night) });
    } else if (previousHour !== null && hour && hour.key !== previousHour) {
      breaks.set(index, {
        kind: 'hour',
        label: hour.label,
        title: hourStep === 1 ? `From ${hour.label}` : `From ${hour.label}, marked every ${hourStep} hours`,
      });
    }
    if (nightIndex % 2 === 1) altNight.add(index);
    previousNight = night;
    previousHour = hour?.key ?? null;
  });
  return { breaks, altNight, hourStep };
}

/**
 * The class and data a card's slot carries for the night and hour marks
 * drawn in the grid's gaps (`.time-slot` in App.css; TimeBreakMark draws
 * the label). Nothing here takes space, so no card moves for a mark.
 */
export function timeSlotAttributes(marks: TimeMarks | null, index: number) {
  const mark = marks?.breaks.get(index);
  return {
    className: `time-slot${marks?.altNight.has(index) ? ' time-night-alt' : ''}`,
    'data-time-break': mark?.kind,
  };
}
