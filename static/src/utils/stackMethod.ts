import type { StackMethod, StackMethodSettings } from '../api/types';

export const STACK_METHOD_QUERY_KEY = ['stack-method'] as const;

export function sameMethod(left: StackMethod, right: StackMethod): boolean {
  return (Object.keys(left) as Array<keyof StackMethod>).every((key) => left[key] === right[key]);
}

/** A short name for a method: a preset's, or "Custom". */
export function methodName(method: StackMethod, settings: StackMethodSettings | undefined): string {
  if (!settings) return '';
  if (sameMethod(method, settings.recommended)) return 'Recommended';
  if (sameMethod(method, { ...settings.recommended, final_pass: 'draft' })) return 'Draft';
  if (sameMethod(method, settings.classic)) return 'Classic';
  return 'Custom';
}
