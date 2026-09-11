import type { StackColorInputSources, StackColorJob, StackColorRole, StackColorSource } from '../api/types';

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
