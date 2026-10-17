import type { ImageQualityResult } from '../api/types';

/** A suspect pill in Sequence's summary, and the frames behind it. */
export interface SequenceIssue {
  key: 'clouds' | 'focus' | 'tracking' | 'off-target' | 'unsolved';
  label: string;
  /** Says what the frames have in common, over the list. */
  title: string;
  /** The pill's colour: `.issue-badge.clouds`, `.focus` or `.tracking`. */
  tone: 'clouds' | 'focus' | 'tracking';
  /** In the strip's order. */
  frames: ImageQualityResult[];
}

const TRACKING_FLAGS = new Set(['tracking_error', 'pointing_jump', 'pointing_drift']);

const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

/**
 * The suspect pills for a view's frames, each with the frames it names,
 * matched on the same flags the server's summary counts. A pill with no
 * frames is left out.
 */
export function sequenceIssues(images: readonly ImageQualityResult[]): SequenceIssue[] {
  // The flags the server's summary counts; a frame's category is its first
  // issue, and an older server sends only that.
  const having = (test: (flags: string[]) => boolean) => images.filter(image =>
    test(image.category ? [image.category, ...(image.flags ?? [])] : image.flags ?? []));
  const clouds = having(flags => flags.includes('likely_clouds'));
  const focus = having(flags => flags.includes('focus_drift'));
  const tracking = having(flags => flags.some(flag => TRACKING_FLAGS.has(flag)));
  const offTarget = having(flags => flags.includes('off_target'));
  const unsolved = having(flags => flags.includes('plate_solve_failed'));
  const issues: SequenceIssue[] = [
    { key: 'clouds', label: plural(clouds.length, 'cloud event', 'cloud events'), title: 'Likely clouds', tone: 'clouds', frames: clouds },
    { key: 'focus', label: 'Focus drift', title: 'Focus drift', tone: 'focus', frames: focus },
    { key: 'tracking', label: 'Tracking issues', title: 'Tracking or pointing jumps and drift', tone: 'tracking', frames: tracking },
    { key: 'off-target', label: `${offTarget.length} off target`, title: 'Off target', tone: 'tracking', frames: offTarget },
    { key: 'unsolved', label: `${unsolved.length} unsolved`, title: 'Plate solve failed', tone: 'clouds', frames: unsolved },
  ];
  return issues.filter(issue => issue.frames.length > 0);
}
