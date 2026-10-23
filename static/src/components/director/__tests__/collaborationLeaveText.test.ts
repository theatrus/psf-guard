import { describe, expect, it } from 'vitest';
import type { DirectorPlanCollaboration } from '../../../api/directorTypes';
import { coordinatorPlan, coordinatorResult } from '../collaborationLeaveText';

const collaboration = (signed_in: string[], not_signed_in: string[]): DirectorPlanCollaboration => ({
  host: 'collab.example', base_url: 'https://collab.example/', remote_project_id: '000000000002',
  remote_project_name: null, imports: 2, unsent_reports: 0, sent_reports: 0, reported_frames: 0,
  transfers: 0, nightly_intake: [], open_assignments: 0, coordinator_reachable: not_signed_in.length === 0,
  signed_in, not_signed_in,
});

describe('What leaving tells the coordinator', () => {
  it('names the rigs that are signed in and those that are not', () => {
    expect(coordinatorPlan(collaboration(['RedCat', 'Askar'], []))).toBe(
      'PSF Guard declines the tasks of RedCat and Askar at collab.example, so the coordinator stops assigning this work to them.');
    expect(coordinatorPlan(collaboration([], ['RedCat']))).toBe(
      'PSF Guard cannot tell collab.example: RedCat is not signed in to collab.example. Leave or decline the project on collab.example too.');
  });

  it('says whether all, some or none of the tasks were declined', () => {
    expect(coordinatorResult({ host: 'collab.example', tasks: 2, declined: ['a', 'b'], problems: [] }))
      .toBe("collab.example declined all 2 of the plan's tasks.");
    expect(coordinatorResult({ host: 'collab.example', tasks: 2, declined: ['a'], problems: ['Askar is not signed in to this collaboration.'] }))
      .toBe("collab.example declined 1 of the plan's 2 tasks. Askar is not signed in to this collaboration. Leave or decline the project on collab.example for the rest.");
    expect(coordinatorResult({ host: 'collab.example', tasks: 1, declined: [], problems: ['RedCat is not signed in to this collaboration.'] }))
      .toBe('collab.example was not told: RedCat is not signed in to this collaboration. Leave or decline the project on collab.example too.');
  });
});
