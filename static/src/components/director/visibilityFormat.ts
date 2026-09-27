export function formatHours(hours: number): string {
  if (hours <= 0) return 'none';
  if (hours < 1) return `${Math.round(hours * 60)} min`;
  return `${hours.toFixed(1)} h`;
}
