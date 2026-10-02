import { describe, expect, it } from 'vitest';
import { describeWbppRunForProject } from '../useWbppRun';
import type { WbppRunProgress, WbppRunStatus } from '../../api/types';

function status(progress: Partial<WbppRunProgress>, queued: WbppRunStatus['queued'] = []): WbppRunStatus {
  return {
    started: false,
    queued,
    progress: { running: false, stage: 'complete', finished_at: 100, project_id: 3, target_id: 7, ...progress } as WbppRunProgress,
  };
}

describe('describeWbppRunForProject', () => {
  it('names a project’s run for the project and for its own target only', () => {
    const done = status({});
    expect(describeWbppRunForProject(done, 3)?.label).toBe('WBPP masters ready');
    expect(describeWbppRunForProject(done, 3, 7)?.label).toBe('WBPP masters ready');
    // Another target of the same project does not wear this run's state.
    expect(describeWbppRunForProject(done, 3, 8)).toBeNull();
    expect(describeWbppRunForProject(done, 4)).toBeNull();
  });

  it('matches a queued run on its target too', () => {
    const queued = status({ stage: '', project_id: null }, [
      { id: 'q', scope: 'target 7', project_id: 3, target_id: 7, position: 1, queued_at: 1 },
    ]);
    expect(describeWbppRunForProject(queued, 3, 7)?.tone).toBe('queued');
    expect(describeWbppRunForProject(queued, 3, 8)).toBeNull();
    expect(describeWbppRunForProject(queued, 3)?.tone).toBe('queued');
  });
});
