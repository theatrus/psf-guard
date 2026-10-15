import { describe, expect, it } from 'vitest';
import type { DirectorActivationReport, DirectorActivationRig } from '../../../api/directorTypes';
import { describeChange } from '../activationState';

const rig = (name: string, detail: string, extra: Partial<DirectorActivationRig> = {}): DirectorActivationRig => ({
  rig: { id: `${name}-id`, name, revision: 1 }, catalog_slug: name, catalog_name: name, profile_id: 'p', applied: false, warnings: [], push: null,
  changes: [{ kind: 'plan', action: 'adopt', name: 'Heart · Ha 300', detail }],
  shoots: [{ plan: 1, target: 'Heart', template: 'Ha', filter: 'Ha', gain: null, offset: null, exposure_seconds: 300, desired: 60, acquired: 12, objectives: [], turned_on: false }],
  ...extra,
});
const report = (rigs: DirectorActivationRig[], plan = 3): DirectorActivationReport => ({
  project: { id: 'p', name: 'Heart', revision: 1 }, framing_revision: 2, plan_revision: plan, panels: 1, rigs,
  warnings: [], preview_digest: 'd'.repeat(64), applied: false, activation_revision: null,
});

describe('describeChange', () => {
  it('names no rig whose only change is a frame taken since', () => {
    const before = report([rig('RedCat', 'takes over plan #1 (12 of 60 frames taken); desired 60 → 80')]);
    const after = report([rig('RedCat', 'takes over plan #1 (13 of 60 frames taken); desired 60 → 80', {
      shoots: [{ ...before.rigs[0].shoots![0], acquired: 13 }],
    })]);
    expect(describeChange(before, after)).toBe('something the preview cannot show');
  });

  it('names the saved plan, a rig whose rows differ, and why a rig refused', () => {
    const before = report([rig('RedCat', 'desired 60 → 80'), rig('Askar', 'desired 60 → 80')]);
    const after = report([rig('RedCat', 'desired 5 → 80'), rig('Askar', 'desired 60 → 80', { warnings: ['Its database stayed locked; try again in a moment.'] })], 4);
    expect(describeChange(before, after)).toBe("the saved plan (revision 3 → 4), RedCat's rows and Askar: Its database stayed locked; try again in a moment.");
  });
});
