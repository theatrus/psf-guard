/** Which rig Visibility shows: the one chosen there, or the first of the
 *  rigs the target was timed for. Sky in view follows the same rig. */
export function visibilityRig<T extends { rig: { id: string } }>(rigs: readonly T[] | undefined, chosen: string | null): T | undefined {
  return rigs?.find((entry) => entry.rig.id === chosen) ?? rigs?.[0];
}

/** What Visibility tells whoever follows its rig: still timing the target,
 *  failed, or showing a rig; `rigId` is null when no rig could be timed. */
export type ShownRig = { state: 'loading' } | { state: 'failed' } | { state: 'ready'; rigId: string | null };
