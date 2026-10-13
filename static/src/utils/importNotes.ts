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

/** Lights that matched no project, held or left out, or null when none. */
export function unmatchedNote(outcome: ImportOutcome | null | undefined): UnmatchedNote | null {
  const count = outcome?.skipped_unmatched ?? 0;
  if (!outcome || count === 0) return null;
  const held = outcome.new_targets === 'ask';
  const folders = [...new Set((outcome.unmatched ?? []).flatMap((group) => group.folders))].sort();
  return {
    text: `${count} ${held ? 'held' : 'left out'}: no matching project`,
    held,
    folders,
  };
}
