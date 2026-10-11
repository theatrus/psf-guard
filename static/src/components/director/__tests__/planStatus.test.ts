import { describe, expect, it } from 'vitest';
import type { DirectorContribution, DirectorRigProgress } from '../../../api/directorTypes';
import { partStatus, rigStatus } from '../planModel';

const frames = (desired: number, accepted: number) => ({ desired, acquired: accepted, accepted, rejected: 0 });
const rig = (state: number | null, parts: Array<[number, number, number]>): DirectorRigProgress => ({
  rig_id: 'r', project: state === null ? null : { name: 'P', state }, note: null, other: frames(0, 0), total: frames(0, 0),
  objectives: parts.map(([desired, accepted, plans], index) => ({ objective_id: `o${index}`, frames: frames(desired, accepted), exposure_plans: plans })),
});
const contribution = { enabled: true } as DirectorContribution;

describe('where a rig and its parts stand', () => {
  it('names a part off, not yet in Target Scheduler, done, or its project state', () => {
    const active = rig(1, [[10, 4, 1], [10, 10, 1], [0, 0, 0]]);
    expect(partStatus({ ...contribution, enabled: false }, active, active.objectives[0])).toBe('Off');
    expect(partStatus(contribution, rig(null, []), undefined)).toBe('Not activated');
    expect(partStatus(contribution, active, active.objectives[2])).toBe('Not activated');
    expect(partStatus(contribution, active, active.objectives[1])).toBe('Done');
    expect(partStatus(contribution, active, active.objectives[0])).toBe('Active');
    expect(partStatus(contribution, rig(2, [[10, 4, 1]]), rig(2, [[10, 4, 1]]).objectives[0])).toBe('Inactive');
    expect(partStatus(null, active, undefined)).toBe('');
  });

  it('calls a rig done only when every written part is', () => {
    expect(rigStatus(undefined)).toBe('');
    expect(rigStatus(rig(null, []))).toBe('Not activated');
    expect(rigStatus(rig(1, [[10, 10, 1], [0, 0, 0]]))).toBe('Done');
    expect(rigStatus(rig(1, [[10, 10, 1], [10, 2, 1]]))).toBe('Active');
    expect(rigStatus(rig(3, []))).toBe('Closed');
  });
});
