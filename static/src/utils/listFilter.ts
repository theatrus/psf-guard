/**
 * A filter that keeps any of several values, as the Images tab's Filter and
 * Flag do. An empty list keeps them all, and so does `all`, which older
 * links carried. In the URL the values are encoded one by one and joined by
 * commas, so a link with one value reads as before.
 */
export function listFilterOf(
  raw: string | null | undefined,
  normalize: (value: string) => string = (value) => value,
): string[] {
  if (!raw) return [];
  const values: string[] = [];
  for (const part of raw.split(',')) {
    let value = part;
    try {
      value = decodeURIComponent(part);
    } catch {
      // A bare % stays as written.
    }
    value = normalize(value.trim());
    if (value.toLowerCase() === 'all') return [];
    if (value !== '' && !values.includes(value)) values.push(value);
  }
  return values;
}

/** Say a list filter in a few words: its values, or how many. */
export function listFilterLabel(values: readonly string[], label: (value: string) => string = (value) => value): string {
  if (values.length === 0) return 'All';
  if (values.length <= 2) return values.map(label).join(', ');
  return `${values.length} chosen`;
}
