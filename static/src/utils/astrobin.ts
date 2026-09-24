/**
 * AstroBin knows equipment by the numeric id in the address of its page in
 * the equipment explorer. People find a filter there and either read the
 * number off the address or paste the address whole, so both are accepted.
 */

const EXPLORER = 'https://app.astrobin.com/equipment/explorer/filter';

/** The AstroBin equipment explorer page for a filter id. */
export function astrobinFilterUrl(id: number): string {
  return `${EXPLORER}/${id}`;
}

/** The AstroBin equipment explorer's filter section, for finding one. */
export const ASTROBIN_FILTERS_URL = EXPLORER;

/**
 * The filter id in what the person typed: a bare number, or an AstroBin
 * address whose path names a filter (`…/equipment/explorer/filter/4049/…`).
 * Anything else is `null`.
 */
export function parseAstrobinFilterId(text: string): number | null {
  const trimmed = text.trim();
  if (/^\d+$/.test(trimmed)) {
    const id = Number(trimmed);
    return id > 0 ? id : null;
  }
  const match = /\/filter\/(\d+)(?:[/?#]|$)/.exec(trimmed);
  if (match && /astrobin\.com/i.test(trimmed)) {
    const id = Number(match[1]);
    return id > 0 ? id : null;
  }
  return null;
}
