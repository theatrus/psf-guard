import type { DirectorMoonPolicy } from '../../api/directorTypes';

export const defaultMoonPolicy = (): DirectorMoonPolicy => ({
  enabled: false, separation_degrees: 60, width_days: 7,
  relax_degrees_per_degree: 0, relax_min_altitude_degrees: -15,
  relax_max_altitude_degrees: 5, moon_down: false,
});

export function moonProblem(moon: DirectorMoonPolicy | undefined): string | null {
  if (!moon) return null;
  const bounded = (value: number, low: number, high: number) => Number.isFinite(value) && value >= low && value <= high;
  if (!bounded(moon.separation_degrees, 0, 180)) return 'Moon separation must be between 0 and 180 degrees.';
  if (!bounded(moon.width_days, 1, 14) || !Number.isInteger(moon.width_days)) return 'Moon avoidance width must be a whole number between 1 and 14 days.';
  if (!bounded(moon.relax_degrees_per_degree, 0, 180)) return 'Moon relaxation must be between 0 and 180.';
  if (!bounded(moon.relax_min_altitude_degrees, -90, 90) || !bounded(moon.relax_max_altitude_degrees, -90, 90)
    || moon.relax_min_altitude_degrees >= moon.relax_max_altitude_degrees) return 'The lower Moon altitude must be below the upper altitude, between -90 and 90 degrees.';
  return null;
}
