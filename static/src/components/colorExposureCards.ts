import type { StackColorJob, StackColorRole, StackColorSource } from '../api/types';
import { buildColorExposureSets, sameColorSourceFamily, type ColorExposureSet } from './stackColorSources';

export interface ColorExposureCardSet extends ColorExposureSet {
  /** A historical set which must remain inspectable, not an automatic build source. */
  retained: boolean;
}

type RecordedColorSources = Pick<StackColorJob, 'job_id' | 'created_unix_seconds' | 'sources' | 'kind' | 'palette'>;

function recipeRoles(job: RecordedColorSources): StackColorRole[] {
  if (job.kind === 'rgb') return ['red', 'green', 'blue'];
  if (job.kind === 'lrgb') return ['luminance', 'red', 'green', 'blue'];
  if (job.palette === 'hoo' || job.palette === 'foraxx-hoo') return ['ha', 'oiii'];
  return job.palette ? ['ha', 'oiii', 'sii'] : [];
}

function represents(set: ColorExposureSet, saved: ColorExposureSet): boolean {
  return set.known && saved.candidates.every((previous) => {
    const current = set.candidates.filter((source) => source.role === previous.role);
    return current.length === 1 && sameColorSourceFamily(current[0], previous);
  });
}

function completeKnownSet(sources: StackColorSource[], roles: StackColorRole[]): ColorExposureSet | undefined {
  if (sources.length !== roles.length || !roles.every((role) => sources.filter((source) => source.role === role).length === 1)) {
    return undefined;
  }
  const sets = buildColorExposureSets(sources, roles);
  return sets.length === 1 && sets[0].known ? sets[0] : undefined;
}

/**
 * Retain family descriptors when a saved or active combination loses an input.
 * Callers must resolve build refs against the current catalog, never these
 * historical candidates. Mixed-duration and unknown combinations stay custom.
 */
export function withRetainedColorExposureSets(
  currentSets: ColorExposureSet[],
  jobs: RecordedColorSources[],
  roles: StackColorRole[],
): ColorExposureCardSet[] {
  const result = currentSets.map((set) => ({ ...set, candidates: [...set.candidates], retained: false }));
  const currentSources = currentSets.flatMap((set) => set.candidates);
  const recorded = [...jobs].sort((left, right) => right.created_unix_seconds - left.created_unix_seconds
    || left.job_id.localeCompare(right.job_id));
  for (const job of recorded) {
    const requiredRoles = recipeRoles(job);
    if (requiredRoles.length === 0 || !requiredRoles.every((role) => roles.includes(role))) continue;
    const saved = completeKnownSet(job.sources, requiredRoles);
    if (!saved || result.some((set) => represents(set, saved))) continue;
    const compatible = result.map((set, index) => ({ set, index }))
      .filter(({ set }) => !set.retained && set.known && set.candidates.length > 0
        && requiredRoles.every((role) => set.candidates.filter((source) => source.role === role).length <= 1)
        && set.candidates.some((current) => saved.candidates.some((previous) => sameColorSourceFamily(current, previous)))
        && set.candidates.filter((current) => requiredRoles.includes(current.role))
          .every((current) => saved.candidates.some((previous) => sameColorSourceFamily(current, previous))))
      .sort((left, right) => right.set.candidates.length - left.set.candidates.length || left.index - right.index);
    let merged = false;
    for (const { set, index } of compatible) {
      const combined = [...set.candidates];
      let ambiguous = false;
      for (const previous of saved.candidates) {
        if (combined.some((source) => source.role === previous.role)) continue;
        const current = currentSources.filter((source) => sameColorSourceFamily(source, previous));
        if (current.length > 1) {
          ambiguous = true;
          break;
        }
        // An anchor can survive an exposure edit. Its current range wins even
        // if the candidate moved to another band, so old metadata cannot bridge tiers.
        combined.push(current[0] ?? previous);
      }
      const extendedSets = ambiguous ? [] : buildColorExposureSets(combined, roles);
      const extended = extendedSets.length === 1 && extendedSets[0].known ? extendedSets[0] : undefined;
      if (!extended) continue;
      result[index] = { ...extended, retained: false };
      merged = true;
      break;
    }
    if (!merged) result.push({ ...saved, retained: true });
  }
  return result;
}
