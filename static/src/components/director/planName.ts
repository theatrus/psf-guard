/** Why the server would refuse a plan name, or '' when it takes it. The name
 *  is trimmed first; the server allows at most 512 UTF-8 bytes and no
 *  control characters. */
export function planNameProblem(trimmed: string): string {
  const hasControl = [...trimmed].some(char => char.charCodeAt(0) < 32 || (char.charCodeAt(0) >= 127 && char.charCodeAt(0) <= 159));
  return !trimmed || new TextEncoder().encode(trimmed).length > 512 || hasControl
    ? 'Enter a name of at most 512 UTF-8 bytes, without control characters.'
    : '';
}

/** The plan page reads this from the navigation state and focuses Find a
 *  target, the first thing a plan with no target needs. */
export const FIND_TARGET_STATE = { findTarget: true } as const;
export const wantsFindTarget = (state: unknown) =>
  typeof state === 'object' && state !== null && (state as { findTarget?: unknown }).findTarget === true;
