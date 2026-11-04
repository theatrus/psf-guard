import type { CSSProperties } from 'react';
import type { RigNights } from '../../api/nightPlanTypes';
import { formatClock, moonCourse, moonCourseWords, moonGradient } from './nightsModel';

interface Props {
  /** The rigs of one site group; the first whose night has the Moon's
   *  course supplies the Moon, the first with a forecast the cloud. */
  rigs: RigNights[];
  date: string;
  zone?: string;
}

/**
 * One night's sky for a group of sites, under its rigs in the grid: the
 * forecast's cloud and the Moon from its rise to its set, from dusk to
 * dawn like the cells' bars.
 */
export default function NightSky({ rigs, date, zone }: Props) {
  const nights = rigs.flatMap(rig => rig.nights.filter(n => n.date === date));
  const night = nights.find(n => n.moon_altitudes.degrees.length > 0) ?? nights[0];
  if (!night || night.dusk_ms === null || night.dawn_ms === null || night.dawn_ms <= night.dusk_ms) return null;
  const from = night.dusk_ms;
  const to = night.dawn_ms;
  const span = to - from;
  const clouded = rigs.find(rig => rig.nights.some(n => n.date === date && n.clouds.length > 0));
  const clouds = clouded?.nights.find(n => n.date === date)?.clouds.filter(hour => hour.end_ms > from && hour.start_ms < to) ?? [];
  const threshold = clouded?.forecast?.cloud_threshold_percent;
  const course = moonCourse(night.moon_altitudes, night.moon_illumination, from, to);
  const moon = `Moon ${moonCourseWords(night.moon_altitudes, night.moon_illumination, course, ms => formatClock(ms, zone))}`;
  const covers = clouds.map(hour => Math.round(hour.cloud_total));
  const least = Math.min(...covers);
  const most = Math.max(...covers);
  const cloud = covers.length === 0 ? [] : [`cloud ${least === most ? least : `${least}–${most}`}%`];
  const words = [moon, ...cloud].join('; ');
  const gradient = moonGradient(course);
  return <div className="nights-sky" role="img" aria-label={words} title={words}>
    {clouds.length > 0 && <span className="nights-sky-clouds">
      {clouds.map(hour => {
        const left = Math.max(0, (hour.start_ms - from) / span);
        const right = Math.min(1, (hour.end_ms - from) / span);
        return <span key={hour.start_ms} className={`nights-cloud${threshold !== undefined && hour.cloud_total > threshold ? ' is-cloudy' : ''}`}
          style={{ left: `${left * 100}%`, width: `${(right - left) * 100}%`, '--cloud-alpha': (0.12 + 0.88 * hour.cloud_total / 100).toFixed(2) } as CSSProperties} />;
      })}
    </span>}
    <span className="nights-sky-moon" style={gradient ? { backgroundImage: gradient } : undefined} />
  </div>;
}
