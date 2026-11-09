import { describe, expect, it } from 'vitest';
import type { DirectorPlanRow } from '../../../api/directorTypes';
import { activationDueText } from '../planCardModel';

const rig = (name: string) => ({ id: `${name}-id`, name, revision: 1 });
const row = (activation_due: DirectorPlanRow['activation_due']): DirectorPlanRow => ({
  project: { id: 'p', name: 'Heart', revision: 1 }, links: [], progress: null, framing: null, plan: null, activation: null, activation_due,
});

describe('why a plan needs activating', () => {
  it('names rigs from the server, even without a link to their database', () => {
    expect(activationDueText(row(null))).toBeNull();
    expect(activationDueText(row({ never: true, parts: [], left_out: [], differs: [], unsent: [] }))).toBe('Not activated yet');
    expect(activationDueText(row({ never: false, parts: ['plan', 'framing'], left_out: [rig('C925')], differs: [rig('RedCat'), rig('Askar')], unsent: ['Observatory'] })))
      .toBe("Needs activating: saved plan and framing not activated yet; C925 not activated yet; RedCat and Askar differ from the plan in Target Scheduler; Observatory hasn't received this activation");
    expect(activationDueText(row({ never: false, parts: [], left_out: [], differs: [], unsent: ['Observatory', 'Backyard'] })))
      .toBe("Needs activating: Observatory and Backyard haven't received this activation");
  });
});
