import { useEffect, useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { DirectorFeasibility, DirectorRigFeasibility, DirectorSkyPosition } from '../../api/directorTypes';
import { formatHours } from './visibilityFormat';
import './VisibilityPanel.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Feasibility request failed';
const W = 720;
const H = 240;
const PAD = { left: 34, right: 12, top: 10, bottom: 26 };

function useDebounced<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => { const timer = setTimeout(() => setDebounced(value), delay); return () => clearTimeout(timer); }, [value, delay]);
  return debounced;
}

function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

/** Tonight's altitude chart for one rig: target, its horizon at each
 *  azimuth, the Moon, and darkness bands. Time runs from an hour before
 *  dusk to an hour after dawn; a night without darkness shows noon to noon. */
export function AltitudeChart({ rig }: { rig: DirectorRigFeasibility }) {
  const samples = rig.curve.samples;
  const night = rig.curve.night;
  const start = night.dusk_ms !== null ? night.dusk_ms - 3_600_000 : night.noon_ms;
  const end = night.dawn_ms !== null ? night.dawn_ms + 3_600_000 : night.noon_ms + 86_400_000;
  const shown = samples.filter(s => s.t_ms >= start && s.t_ms <= end);
  if (shown.length < 2) return <p className="director-muted">No curve for tonight.</p>;
  const x = (t: number) => PAD.left + ((t - start) / (end - start)) * (W - PAD.left - PAD.right);
  const y = (altitude: number) => PAD.top + ((90 - Math.max(0, Math.min(90, altitude))) / 90) * (H - PAD.top - PAD.bottom);
  const line = (pick: (s: typeof shown[number]) => number | null) => {
    let d = ''; let pen = false;
    for (const s of shown) {
      const v = pick(s);
      if (v === null || v < 0) { pen = false; continue; }
      d += `${pen ? 'L' : 'M'}${x(s.t_ms).toFixed(1)},${y(v).toFixed(1)}`; pen = true;
    }
    return d;
  };
  const bands: Array<{ from: number; to: number; deep: boolean }> = [];
  let open: { from: number; deep: boolean } | null = null;
  for (const s of shown) {
    const dark = s.sun_altitude_degrees < -12;
    const deep = s.sun_altitude_degrees < -18;
    if (dark && (!open || open.deep !== deep)) { if (open) bands.push({ from: open.from, to: s.t_ms, deep: open.deep }); open = { from: s.t_ms, deep }; }
    if (!dark && open) { bands.push({ from: open.from, to: s.t_ms, deep: open.deep }); open = null; }
  }
  if (open) bands.push({ from: open.from, to: end, deep: open.deep });
  const minimum = rig.limits.minimum_altitude_degrees;
  // The target's track breaks where the rig pauses at the meridian.
  const target = line(s => s.targets[0] && !s.targets[0].meridian_blocked ? s.targets[0].altitude_degrees : null);
  const paused = line(s => s.targets[0]?.meridian_blocked ? s.targets[0].altitude_degrees : null);
  const transit = night.targets[0]?.transit_ms ?? null;
  const horizon = line(s => Math.max(s.targets[0]?.horizon_altitude_degrees ?? 0, minimum));
  const moon = line(s => s.moon_altitude_degrees);
  const ticks: number[] = [];
  for (let t = Math.ceil(start / 3_600_000) * 3_600_000; t <= end; t += 2 * 3_600_000) ticks.push(t);
  return <svg className="visibility-chart" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={`Altitude of the target tonight at ${rig.catalog_name}`}>
    {bands.map(band => <rect key={band.from} className={band.deep ? 'band-deep' : 'band-dark'} x={x(band.from)} y={PAD.top} width={Math.max(0, x(band.to) - x(band.from))} height={H - PAD.top - PAD.bottom} />)}
    {[30, 60].map(alt => <line key={alt} className="grid" x1={PAD.left} x2={W - PAD.right} y1={y(alt)} y2={y(alt)} />)}
    {[0, 30, 60, 90].map(alt => <text key={alt} className="axis" x={PAD.left - 4} y={y(alt) + 4} textAnchor="end">{alt}°</text>)}
    {ticks.map(t => <text key={t} className="axis" x={x(t)} y={H - 8} textAnchor="middle">{clock(t)}</text>)}
    <path className="horizon" d={horizon} />
    <path className="moon" d={moon} />
    {transit !== null && transit >= start && transit <= end && <g className="transit"><line x1={x(transit)} x2={x(transit)} y1={PAD.top} y2={H - PAD.bottom} /><text x={x(transit) + 4} y={PAD.top + 12}>meridian</text></g>}
    <path className="paused" d={paused} />
    <path className="target" d={target} />
    <line className="frame" x1={PAD.left} x2={W - PAD.right} y1={y(0)} y2={y(0)} />
  </svg>;
}

export default function VisibilityPanel({ projectId, center, enabled = true }: { projectId: string; center: DirectorSkyPosition; enabled?: boolean }) {
  const rounded = useMemo(() => ({ ra_degrees: Number(center.ra_degrees.toFixed(3)), dec_degrees: Number(center.dec_degrees.toFixed(3)) }), [center.ra_degrees, center.dec_degrees]);
  const debounced = useDebounced(rounded, 600);
  const query = useQuery({
    queryKey: ['directorFeasibility', projectId, debounced],
    queryFn: () => apiClient.getDirectorFeasibility(projectId, { center: debounced }),
    enabled, retry: false, staleTime: 60_000, placeholderData: previous => previous,
  });
  const [chosen, setChosen] = useState<string | null>(null);
  const data: DirectorFeasibility | undefined = query.data;
  const rig = data?.rigs.find(r => r.rig.id === chosen) ?? data?.rigs[0];
  if (!enabled) return null;
  return <section className="visibility" aria-label="Visibility">
    <div className="director-toolbar"><h3>Visibility</h3>
      {data && data.rigs.length > 1 && <select aria-label="Visibility rig" value={rig?.rig.id ?? ''} onChange={event => setChosen(event.target.value)}>
        {data.rigs.map(r => <option key={r.rig.id} value={r.rig.id}>{r.catalog_name}{r.in_plan ? '' : ' (not in plan)'}</option>)}
      </select>}
    </div>
    {query.isPending && <p role="status">Timing the target...</p>}
    {query.isError && <p className="director-error" role="alert">{message(query.error)}</p>}
    {data?.warnings.map(w => <p key={w} className="director-muted">{w}</p>)}
    {rig && <>
      {(() => {
        const tonight = rig.nights[0];
        const up = tonight?.targets[0]?.hours_up ?? 0;
        const verdict = !tonight || tonight.dark_hours === 0 ? `No darkness tonight at ${rig.catalog_name}.`
          : up <= 0 ? `Not visible tonight from ${rig.catalog_name}: never above ${rig.limits.minimum_altitude_degrees}°${rig.custom_horizon ? ' and its horizon' : ''} while dark.`
          : `Visible ${formatHours(up)} tonight from ${rig.catalog_name} (${formatHours(tonight.dark_hours)} dark, peak ${tonight.targets[0].max_altitude_degrees.toFixed(0)}°${tonight.targets[0].hours_lost_to_meridian > 0 ? `, ${formatHours(tonight.targets[0].hours_lost_to_meridian)} lost to the meridian pause` : ''}). Moon ${Math.round(tonight.moon_illumination * 100)}% lit, ${tonight.targets[0].min_moon_separation_degrees.toFixed(0)}° away, up ${formatHours(tonight.moon_hours_up_in_dark)} of the dark.`;
        return <p className={up > 0 ? 'visibility-verdict' : 'visibility-verdict is-down'} data-testid="visibility-verdict">{verdict}</p>;
      })()}
      <AltitudeChart rig={rig} />
      <p className="director-muted visibility-legend"><span className="key target" />target <span className="key horizon" />{rig.custom_horizon ? 'custom horizon and limit' : `minimum altitude ${rig.limits.minimum_altitude_degrees}°`} <span className="key moon" />Moon <span className="key paused" />meridian pause <span className="key dark" />dark (Sun below −12°) <span className="key deep" />astronomical night</p>
      <div className="director-table-scroll"><table className="visibility-nights"><thead><tr><th>Night</th><th>Dark</th><th>Target up</th><th>Moon down too</th><th>Moon</th></tr></thead><tbody>
        {rig.nights.map(night => <tr key={night.date}>
          <td>{night.date}</td><td>{formatHours(night.dark_hours)}</td><td>{formatHours(night.targets[0]?.hours_up ?? 0)}</td><td>{formatHours(night.targets[0]?.hours_up_moon_down ?? 0)}</td>
          <td>{Math.round(night.moon_illumination * 100)}%{night.targets[0] && night.targets[0].hours_up > 0 ? `, ${night.targets[0].min_moon_separation_degrees.toFixed(0)}° away` : ''}</td>
        </tr>)}
      </tbody></table></div>
      {rig.hours_needed !== null && <p className="director-muted" data-testid="visibility-estimate">{rig.hours_needed > 0 ? `This rig owes the plan ${formatHours(rig.hours_needed)}${rig.nights_to_complete !== null ? `; at this week's rate that is about ${rig.nights_to_complete} night${rig.nights_to_complete === 1 ? '' : 's'}.` : ', but the target is not up this week.'}` : 'This rig has nothing to shoot in the plan yet.'}</p>}
    </>}
  </section>;
}
