import type {
  StackColorProcessing,
  StackColorRole,
  StackStretchRequest,
  StackViewProcessingRequest,
} from '../api/types';
import {
  colorCalibrationApplies,
  defaultColorCalibration,
  defaultColorProcessing,
  automaticNarrowbandProcessing,
} from './stackColorProcessing';
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
  supports?: (roles: StackColorRole[]) => boolean;
}

export const builtinColorSetups: BuiltinColorSetup[] = [
  {
    name: 'Built-in color pipeline',
    settings: (roles) => defaultColorProcessing(roles),
  },
  {
    name: 'No background correction',
    settings: (roles) => ({ ...defaultColorProcessing(roles), background_extraction: null }),
  },
  {
    name: 'Automatic narrowband',
    supports: (roles) => roles.includes('ha') && roles.includes('oiii'),
    settings: (roles) => automaticNarrowbandProcessing(defaultColorProcessing(roles)),
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
    ...(settings.emission ? { emission: structuredClone(settings.emission) } : {}),
    ...(settings.narrowband ? { narrowband: structuredClone(settings.narrowband) } : {}),
    // A setup saved before the choice existed sets its inputs' tools itself.
    ...(settings.input_processing === 'channels' ? { input_processing: 'channels' as const } : {}),
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
    input_rc_astro: mapRoleEntries(
      settings.input_rc_astro,
      roles,
      false,
      () => undefined
    ),
    ...(settings.color_rc_astro?.steps.length
      ? { color_rc_astro: structuredClone(settings.color_rc_astro) }
      : {}),
    // A setup saved from a narrowband card says nothing of calibration, so
    // an RGB card keeps the default; a narrowband card never calibrates.
    ...(colorCalibrationApplies(roles)
      ? { color_calibration: settings.color_calibration ?? defaultColorCalibration() }
      : {}),
  };
}

/**
 * A filesystem-safe download name for one exported setup. The collection
 * export keeps its fixed name; a single setup carries its own.
 */
export function setupExportFilename(name: string): string {
  const slug = name
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
  return `psf-guard-setup-${slug || 'unnamed'}.json`;
}
