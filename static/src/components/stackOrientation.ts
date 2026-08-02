import type { StackColorJob, StackSkyOrientation } from '../api/types';

/**
 * Whether a stack was reprojected onto the shared north-up, east-left grid.
 * Builds keep the reference frame's own rotation by default, so only an
 * explicitly oriented stack may claim the celestial display contract.
 */
export function isSkyOriented(orientation: StackSkyOrientation | null | undefined): boolean {
  return orientation?.convention === 'north_up_east_left';
}

/**
 * A composite only carries the celestial display contract when every channel
 * stack it registered was built on the shared north-up grid.
 */
export function isColorStackSkyOriented(job: StackColorJob | undefined): boolean {
  const sources = job?.sources ?? [];
  return sources.length > 0 && sources.every((source) => isSkyOriented(source.sky_orientation));
}
