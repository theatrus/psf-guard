import type { ImportLeftOutReason, ImportOutcome } from '../api/types';

const LEFT_OUT_WORDS: Record<ImportLeftOutReason, string> = {
  no_image_type: 'no image type',
  no_target_name: 'no target name',
  short_exposure: 'too short',
};

/** What a run's scan rules left out, a few words each: "12 skipped by scan rules". */
export function scanRuleNotes(outcome: ImportOutcome | null | undefined): string[] {
  if (!outcome) return [];
  const notes: string[] = [];
  const rules = outcome.scan_rules;
  if (rules?.files_skipped) notes.push(`${rules.files_skipped} skipped by scan rules`);
  if (rules?.folders_skipped) {
    notes.push(`${rules.folders_skipped} folder(s) skipped by scan rules`);
  }
  if (rules?.ignored_folders) {
    notes.push(`${rules.ignored_folders} folder(s) skipped by .psfguardignore`);
  }
  for (const left of outcome.left_out ?? []) {
    notes.push(`${left.frames} left out: ${LEFT_OUT_WORDS[left.reason] ?? left.reason}`);
  }
  return notes;
}

export interface UnmatchedNote {
  text: string;
  /** Held for review (Ask first) rather than left out. */
  held: boolean;
  /** Every folder holding them, for the import preview. */
  folders: string[];
}

/**
 * Lights a run held or left out, or null when none: those that matched no
 * project, and those whose target had no exposure plan for them.
 */
export function unmatchedNote(outcome: ImportOutcome | null | undefined): UnmatchedNote | null {
  const noProject = outcome?.skipped_unmatched ?? 0;
  const noPlan = outcome?.skipped_unplanned ?? 0;
  if (!outcome || noProject + noPlan === 0) return null;
  const held = outcome.new_targets === 'ask';
  const reasons =
    noProject > 0 && noPlan > 0
      ? `${noProject} no matching project, ${noPlan} no exposure plan`
      : noProject > 0
        ? 'no matching project'
        : 'no exposure plan';
  const folders = [
    ...new Set([
      ...(outcome.unmatched ?? []).flatMap((group) => group.folders),
      ...(outcome.unplanned ?? []).flatMap((group) => group.folders),
    ]),
  ].sort();
  return {
    text: `${noProject + noPlan} ${held ? 'held' : 'left out'}: ${reasons}`,
    held,
    folders,
  };
}

/**
 * What an attach changes in what the scheduler shoots, a few words each:
 * "adds 1 exposure plan", "raises acquired on 2 plans".
 */
export function planChanges(attach: { plans_created?: number; plans_raised?: number }): string[] {
  const changes: string[] = [];
  const created = attach.plans_created ?? 0;
  const raised = attach.plans_raised ?? 0;
  if (created > 0) changes.push(`adds ${created} exposure plan${created === 1 ? '' : 's'}`);
  if (raised > 0) changes.push(`raises acquired on ${raised} plan${raised === 1 ? '' : 's'}`);
  return changes;
}
