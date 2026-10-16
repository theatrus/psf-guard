import type { CalibrationLightMark, ImageQualityResult } from '../api/types';
import { cutoffMetricOf, type CutoffMetric } from './sequenceCutoff';

/**
 * The Sequence view's selection rules. A rule chosen in the Select frames
 * field stays on, in the URL, and selects again whenever its answer
 * changes: the cutoff moves or measures something else, a scan rescores
 * the frames, the calibration marks arrive, or another session tab opens.
 */
export type SelectionRuleId =
  | 'below-cutoff'
  | 'clouded'
  | 'off-target'
  | 'unsolved'
  | 'recommended'
  | 'poor-calibration';

export interface SelectionRule {
  id: SelectionRuleId;
  /** The option's name. */
  label: string;
  /** What it selects, in one line. */
  description: string;
}

export const SELECTION_RULES: readonly SelectionRule[] = [
  {
    id: 'below-cutoff',
    label: 'Below the cutoff',
    description: 'Frames scoring under the score cutoff in the view on screen',
  },
  {
    id: 'clouded',
    label: 'Clouds or obstruction',
    description: 'Frames in which a detector found clouds or an obstruction',
  },
  {
    id: 'off-target',
    label: 'Off target',
    description: 'Frames off target, or with a pointing jump, drift or rotation skew',
  },
  {
    id: 'unsolved',
    label: 'Failed plate solve',
    description: 'Frames that failed to solve and show image-quality evidence',
  },
  {
    id: 'recommended',
    label: 'Recommended rejects',
    description: 'Frames the analysis recommends rejecting',
  },
  {
    id: 'poor-calibration',
    label: 'Poor calibration',
    description: 'Lights the calibration library serves poorly',
  },
];

const POINTING_FLAGS = new Set(['off_target', 'pointing_jump', 'pointing_drift', 'rotation_skew']);

export function parseSelectionRule(raw: string | null | undefined): SelectionRuleId | null {
  return SELECTION_RULES.find((rule) => rule.id === raw)?.id ?? null;
}

/** A rule as the Select field names it. Below the cutoff takes its name
 *  from what the cutoff measures. */
export function selectionRuleOf(id: SelectionRuleId, metric: CutoffMetric = 'score'): SelectionRule {
  const rule = SELECTION_RULES.find((candidate) => candidate.id === id)!;
  if (id !== 'below-cutoff') return rule;
  const info = cutoffMetricOf(metric);
  return { ...rule, label: info.ruleLabel, description: info.ruleDescription };
}

interface RuleInputs {
  /** The frames past the cutoff in the measure chosen (sequenceCutoff.ts). */
  pastCutoff: ReadonlySet<number>;
  calibration: ReadonlyMap<number, CalibrationLightMark>;
  /** The project's calibration marks came before a change and wait for a
   *  pass. Fine to show; not to select from, since a reject follows. */
  calibrationStale?: boolean;
}

function matches(rule: SelectionRuleId, image: ImageQualityResult, inputs: RuleInputs): boolean {
  switch (rule) {
    case 'below-cutoff':
      return inputs.pastCutoff.has(image.image_id);
    case 'clouded':
      // A detector named the evidence; a low relative score alone must not
      // pass for a diagnosis.
      return image.category === 'likely_clouds' || image.category === 'possible_obstruction';
    case 'off-target':
      return (image.flags ?? []).some((flag) => POINTING_FLAGS.has(flag));
    case 'unsolved':
      return Boolean(image.pointing?.solve_failed && image.pointing.image_quality_evidence);
    case 'recommended':
      return Boolean(image.regrade_reason);
    case 'poor-calibration':
      return !inputs.calibrationStale && inputs.calibration.get(image.image_id)?.level === 'poor';
  }
}

/** The frames of one view a rule selects. */
export function ruleSelection(
  rule: SelectionRuleId,
  images: readonly ImageQualityResult[],
  inputs: RuleInputs,
): Set<number> {
  return new Set(images.filter((image) => matches(rule, image, inputs)).map((image) => image.image_id));
}

/**
 * The selection once a rule's answer changes from `before` to `after`.
 *
 * A rule and the person share the selection. Frames the rule picked and
 * nobody touched follow the rule: they go when it stops matching them, and
 * new matches come in. A frame clicked by hand since the rule last ran
 * stays as the person left it: one added stays selected, one removed stays
 * out while the rule still matches it. So a click never turns the rule
 * off, and moving the cutoff never undoes a click.
 */
export function followRule(
  selection: ReadonlySet<number>,
  before: ReadonlySet<number>,
  after: ReadonlySet<number>,
): Set<number> {
  const next = new Set<number>();
  for (const id of after) {
    // Removed by hand: the rule had it, the selection does not.
    if (!before.has(id) || selection.has(id)) next.add(id);
  }
  for (const id of selection) {
    // Added by hand: the rule never had it.
    if (!before.has(id)) next.add(id);
  }
  return next;
}

/** How many frames differ from what the rule picked: the ones added or
 *  removed by hand. */
export function handChanges(selection: ReadonlySet<number>, picked: ReadonlySet<number>): number {
  let count = 0;
  for (const id of selection) if (!picked.has(id)) count += 1;
  for (const id of picked) if (!selection.has(id)) count += 1;
  return count;
}
