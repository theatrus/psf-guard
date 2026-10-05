import type { DirectorHorizon } from '../../api/directorTypes';

/** The `.hrz` file N.I.N.A. reads: one `azimuth altitude` pair per line. */
export function horizonToHrz(horizon: DirectorHorizon): string | null {
  if (horizon.mode !== 'custom') return null;
  return ['# Azimuth Altitude, degrees', ...horizon.points.map(point => `${point.azimuth_degrees} ${point.altitude_degrees}`)].join('\n') + '\n';
}

/** A few words on a curve: how many points and where it stands highest. */
export function describeHorizon(horizon: DirectorHorizon | null | undefined): string {
  if (!horizon || horizon.mode !== 'custom') return 'flat at the minimum altitude';
  const highest = horizon.points.reduce((top, point) => point.altitude_degrees > top.altitude_degrees ? point : top, horizon.points[0]);
  return `${horizon.points.length} points, highest ${highest.altitude_degrees}° at azimuth ${highest.azimuth_degrees}°`;
}

/** Save the curve as a `.hrz` file named after the site or rig. */
export function downloadHrz(name: string, horizon: DirectorHorizon) {
  const text = horizonToHrz(horizon);
  if (!text) return;
  const url = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = `${name.replace(/[^\w.-]+/g, '_') || 'horizon'}.hrz`;
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}
