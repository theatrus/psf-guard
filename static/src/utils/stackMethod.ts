import type { StackMethod, StackMethodSettings } from '../api/types';

export const STACK_METHOD_QUERY_KEY = ['stack-method'] as const;

/**
 * Whether two methods build the same stack. The rotation margin only
 * matters when Seiza chooses the reference, as on the server, and a method
 * recorded before partial stacks existed did not use them.
 */
export function sameMethod(left: StackMethod, right: StackMethod): boolean {
  const keys = new Set([...Object.keys(left), ...Object.keys(right)]) as Set<keyof StackMethod>;
  return [...keys].every((key) =>
    left[key] === right[key]
    || (key === 'reference_rotation_margin_degrees' && left.reference !== 'auto' && right.reference !== 'auto')
    || (key === 'partial_stacks' && !left.partial_stacks && !right.partial_stacks));
}

/** A short name for a method: a preset's, or "Custom". */
export function methodName(method: StackMethod, settings: StackMethodSettings | undefined): string {
  if (!settings) return '';
  if (sameMethod(method, settings.recommended)) return 'Recommended';
  if (sameMethod(method, { ...settings.recommended, final_pass: 'draft' })) return 'Draft';
  if (sameMethod(method, settings.classic)) return 'Classic';
  return 'Custom';
}
