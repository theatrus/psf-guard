/** The query that reads Settings › Offline sky maps. */
export const skyMapsKey = ['skyMaps'] as const;

/** A size for people: gigabytes, or megabytes under about one. */
export function gigabytes(bytes: number): string {
  const value = bytes / 1024 ** 3;
  return value >= 0.95 ? `${value.toFixed(1)} GB` : `${Math.round(bytes / 1024 ** 2)} MB`;
}
