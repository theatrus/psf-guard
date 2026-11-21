/** Why the server would refuse a plan name, or '' when it takes it. The name
 *  is trimmed first; the server allows at most 512 UTF-8 bytes and no
 *  control characters. */
export function planNameProblem(trimmed: string): string {
  const hasControl = [...trimmed].some(char => char.charCodeAt(0) < 32 || (char.charCodeAt(0) >= 127 && char.charCodeAt(0) <= 159));
  return !trimmed || new TextEncoder().encode(trimmed).length > 512 || hasControl
    ? 'Enter a name of at most 512 UTF-8 bytes, without control characters.'
    : '';
}

/** New plan opens the plan with this navigation state. The plan page then
 *  focuses Object name, the search for the plan's first target. */
export const NEW_PLAN_STATE = { newPlan: true } as const;
export const isNewPlan = (state: unknown) =>
  typeof state === 'object' && state !== null && (state as { newPlan?: unknown }).newPlan === true;
