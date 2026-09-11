import type { StackColorInputSources, StackColorJob, StackColorRole, StackColorSource } from '../api/types';

export interface ColorExposureSet {
  key: string;
  label: string;
  candidates: StackColorSource[];
  known: boolean;
  minSeconds: number | null;
  maxSeconds: number | null;
}

function sourceFamilyIdentity(source: StackColorSource): string {
  return JSON.stringify([source.role, source.filter_name, source.exposure_group?.key ?? null]);
}

function compareSourceFamilies(left: StackColorSource, right: StackColorSource): number {
  const family = sourceFamilyIdentity(left).localeCompare(sourceFamilyIdentity(right));
  return family || colorSourceKey(left).localeCompare(colorSourceKey(right));
}

function makeExposureSet(
  candidates: StackColorSource[],
  kind: 'known' | 'mixed' | 'unknown',
  minSeconds: number | null,
  maxSeconds: number | null,
): ColorExposureSet {
  const ordered = [...candidates].sort(compareSourceFamilies);
  const key = JSON.stringify(['exposure-set', kind, ordered.map(sourceFamilyIdentity).sort()]);
  let label = kind === 'mixed' ? 'Mixed exposure' : 'Unknown exposure';
  if (kind === 'known' && minSeconds !== null && maxSeconds !== null) {
    const minimum = `${Number(minSeconds.toPrecision(6))} s`;
    const maximum = `${Number(maxSeconds.toPrecision(6))} s`;
    label = minimum === maximum ? minimum : `${minimum} - ${maximum}`;
  }
  return { key, label, candidates: ordered, known: kind === 'known', minSeconds, maxSeconds };
}

/** Match physical exposure ranges; filter-local family keys only identify the resulting set. */
export function buildColorExposureSets(
  candidates: StackColorSource[],
  roles: StackColorRole[],
): ColorExposureSet[] {
  const required = new Set(roles);
  const intervals: Array<{ source: StackColorSource; min: number; max: number }> = [];
  const unknown: StackColorSource[] = [];
  const mixed: Array<{ source: StackColorSource; min: number; max: number }> = [];
  for (const source of candidates) {
    if (!required.has(source.role)) continue;
    const min = source.exposure_group?.min_seconds;
    const max = source.exposure_group?.max_seconds;
    if (typeof min !== 'number' || typeof max !== 'number'
      || !Number.isFinite(min) || !Number.isFinite(max) || min <= 0 || max < min) {
      unknown.push(source);
    } else if (max / min >= 2) {
      mixed.push({ source, min, max });
    } else {
      intervals.push({ source, min, max });
    }
  }
  intervals.sort((left, right) => left.min - right.min || left.max - right.max
    || compareSourceFamilies(left.source, right.source));
  const sets: ColorExposureSet[] = [];
  let start = 0;
  while (start < intervals.length) {
    const minimum = intervals[start].min;
    let maximum = intervals[start].max;
    let end = start + 1;
    // Bound the full union so overlapping or adjacent ranges cannot bridge tiers.
    while (end < intervals.length && Math.max(maximum, intervals[end].max) / minimum < 2) {
      maximum = Math.max(maximum, intervals[end].max);
      end += 1;
    }
    sets.push(makeExposureSet(intervals.slice(start, end).map((entry) => entry.source), 'known', minimum, maximum));
    start = end;
  }
  if (mixed.length > 0) {
    sets.push(makeExposureSet(mixed.map((entry) => entry.source), 'mixed',
      mixed.reduce((minimum, entry) => Math.min(minimum, entry.min), Infinity),
      mixed.reduce((maximum, entry) => Math.max(maximum, entry.max), 0)));
  }
  if (unknown.length > 0) sets.push(makeExposureSet(unknown, 'unknown', null, null));
  return sets;
}

export function completedColorArtifact(
  watched: StackColorJob | undefined,
  catalog: StackColorJob[],
  remembered: StackColorJob | undefined,
): StackColorJob | undefined {
  if (watched?.state !== 'completed') return remembered;
  // Only the catalog recomputes whether a completed artifact is still current.
  return catalog.find((job) => job.job_id === watched.job_id) ?? watched;
}

export function colorSourceKey(source: Pick<StackColorSource, 'job_id' | 'group_index' | 'artifact_revision'>): string {
  return JSON.stringify([source.job_id, source.group_index, source.artifact_revision]);
}

export function sameColorSourceFamily(left: StackColorSource, right: StackColorSource): boolean {
  return left.role === right.role && left.filter_name === right.filter_name
    && (left.exposure_group?.key ?? null) === (right.exposure_group?.key ?? null);
}

export function resolveColorSources(
  candidates: StackColorSource[],
  roles: StackColorRole[],
  selected: Partial<Record<StackColorRole, string>>,
): { sources: StackColorSource[]; complete: boolean; inputSources: StackColorInputSources } {
  const sources: StackColorSource[] = [];
  for (const role of roles) {
    const matching = candidates.filter((source) => source.role === role);
    const key = selected[role];
    const source = key === undefined
      ? matching.length === 1 ? matching[0] : undefined
      : matching.find((candidate) => colorSourceKey(candidate) === key);
    if (source) sources.push(source);
  }
  return {
    sources,
    complete: sources.length === roles.length,
    inputSources: Object.fromEntries(sources.map((source) => [source.role, {
      job_id: source.job_id,
      group_index: source.group_index,
      artifact_revision: source.artifact_revision,
    }])),
  };
}

export function colorSourcesLabel(sources: StackColorSource[]): string {
  return sources.map((source) => source.label || [source.filter_name, source.exposure_group?.label]
    .filter(Boolean).join(' · ')).join(', ');
}
