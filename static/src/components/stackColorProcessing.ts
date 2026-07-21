import type { StackColorProcessing, StackColorRole } from '../api/types';
import { defaultStretchRequest } from './stackStretchModels';

export function defaultColorProcessing(roles: StackColorRole[]): StackColorProcessing {
  return {
    input_stretches: Object.fromEntries(
      roles.map((role) => [role, [defaultStretchRequest('auto-mtf')]])
    ),
    output_stretches: [],
  };
}
