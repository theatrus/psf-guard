/**
 * Integrated exposure as people say it: hours and minutes once it reaches an
 * hour, minutes below that, seconds only when there is less than a minute.
 * Minutes round to the nearest whole minute so a card reads "2h 5m", not
 * "2h 4m 48s".
 */
export function formatIntegration(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return '—';
  if (seconds < 60) return `${Math.round(seconds)} s`;
  const totalMinutes = Math.round(seconds / 60);
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours === 0) return `${minutes}m`;
  return minutes === 0 ? `${hours}h` : `${hours}h ${minutes}m`;
}

/** Sum of the integrated exposure of several stacks, for a project or a color preview. */
export function totalIntegration(seconds: Array<number | null | undefined>): number {
  return seconds.reduce<number>((sum, value) => sum + (value ?? 0), 0);
}
