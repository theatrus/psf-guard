import type {
  StackColorProcessing,
  StackColorRole,
  StackStretchRequest,
  StackViewProcessingRequest,
} from '../api/types';
import { defaultColorProcessing } from './stackColorProcessing';
import { defaultStretchRequest } from './stackStretchModels';

/**
 * Pre-canned setups. These are derived from the same defaults the editors
 * start from, so they are honest about what the pipeline actually does; they
 * are not stored on the server and cannot be deleted or exported.
 */
export interface BuiltinViewSetup {
  name: string;
  settings: StackViewProcessingRequest;
}

export function builtinViewSetups(displayReferred: boolean): BuiltinViewSetup[] {
  if (displayReferred) {
    return [{
      name: 'Untouched display',
      settings: { ...defaultStretchRequest('identity'), deconvolution: null },
    }];
  }
  return [
    {
      name: 'Default auto stretch',
      settings: { ...defaultStretchRequest('auto-mtf'), deconvolution: null },
    },
    {
      name: 'Brighter auto stretch',
      settings: {
        model: { type: 'auto-mtf', target_median: 0.3, shadows_clip: -2.8 },
        color_strategy: 'linked',
        deconvolution: null,
      },
    },
    {
      name: 'Untouched linear',
      settings: { ...defaultStretchRequest('identity'), deconvolution: null },
    },
  ];
}

export interface BuiltinColorSetup {
  name: string;
  settings: (roles: StackColorRole[]) => StackColorProcessing;
}

export const builtinColorSetups: BuiltinColorSetup[] = [
  {
    name: 'Default color pipeline',
    settings: (roles) => defaultColorProcessing(roles),
  },
  {
    name: 'No background correction',
    settings: (roles) => ({ ...defaultColorProcessing(roles), background_extraction: null }),
  },
];

function deepEqual(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

/**
 * One value per target role, from a setup's role-keyed map.
 *
 * A setup remembers the channels it was saved from, and the card it is applied
 * to may use different ones — SHO saved, RGB applied. When the setup treats
 * all of its own channels the same way, that shared value follows it to every
 * target channel. Otherwise channels are matched by name and the rest fall
 * back, so a per-channel setup never guesses which channel maps to which.
 */
function mapRoleEntries<T>(
  entries: Partial<Record<StackColorRole, T>> | undefined,
  roles: StackColorRole[],
  allowUniform: boolean,
  fallback: (role: StackColorRole) => T | undefined
): Partial<Record<StackColorRole, T>> {
  const values = Object.values(entries ?? {}).filter((value) => value !== undefined);
  // One entry is not a policy — only a setup that visibly treats several of
  // its own channels the same way carries that treatment to foreign channels.
  const uniform =
    allowUniform && values.length > 1 && values.every((value) => deepEqual(value, values[0]))
      ? (values[0] as T)
      : undefined;
  const mapped: Partial<Record<StackColorRole, T>> = {};
  for (const role of roles) {
    const value = entries?.[role] ?? uniform ?? fallback(role);
    if (value !== undefined) mapped[role] = value;
  }
  return mapped;
}

/** A stored color setup, reconciled onto the channels of the target card. */
export function colorSetupForRoles(
  settings: StackColorProcessing,
  roles: StackColorRole[]
): StackColorProcessing {
  return {
    background_extraction: settings.background_extraction ?? null,
    output_stretches: settings.output_stretches ?? [],
    input_stretches: mapRoleEntries<StackStretchRequest[]>(
      settings.input_stretches,
      roles,
      true,
      () => [defaultStretchRequest('auto-mtf')]
    ),
    // Deconvolution is opt-in per channel; it never follows a setup onto a
    // channel the setup did not name.
    input_deconvolutions: mapRoleEntries(
      settings.input_deconvolutions,
      roles,
      false,
      () => undefined
    ),
  };
}
