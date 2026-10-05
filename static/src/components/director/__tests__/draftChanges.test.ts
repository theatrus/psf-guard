import { describe, expect, it } from 'vitest';
import type { DirectorPlanDraft } from '../../../api/directorTypes';
import { describeFramingChanges, describePlanChanges } from '../draftChanges';
import type { FramingState } from '../framingModel';

const framing: FramingState = {
  targetName: 'M33', center: { ra_degrees: 23.4621, dec_degrees: 30.66 }, positionAngle: 0,
  mosaic: { rows: 1, columns: 1, overlap_percent: 20 }, panelRigId: 'rig-a', panel: { width_degrees: 2, height_degrees: 1.5 },
  shownRigIds: [], surveyId: 'dss2_color', viewCenter: { ra_degrees: 23.4621, dec_degrees: 30.66 }, viewFov: 4, rigFramings: [],
};
const rigName = (id: string) => ({ 'rig-a': 'RedCat', 'rig-b': 'C925' })[id] ?? id;

describe('what the save bar says changed', () => {
  it('names framing changes and ignores the view', () => {
    expect(describeFramingChanges(framing, { ...framing, viewFov: 9, viewCenter: { ra_degrees: 1, dec_degrees: 2 } })).toEqual([]);
    expect(describeFramingChanges(framing, {
      ...framing,
      targetName: 'Triangulum',
      center: { ra_degrees: 23.4621, dec_degrees: 30.76 },
      positionAngle: 92.5,
      mosaic: { rows: 2, columns: 1, overlap_percent: 20 },
      panelRigId: 'rig-b',
    }, rigName)).toEqual([
      'target name “M33” → “Triangulum”',
      'target moved 6′',
      'camera angle 0° → 92.5°',
      'mosaic 1×1, 20% overlap → 2×1, 20% overlap',
      'panel rig RedCat → C925',
    ]);
    expect(describeFramingChanges(framing, { ...framing, rigFramings: [{ rig_id: 'rig-b', center: null, position_angle_degrees: 10, mosaic: { rows: 1, columns: 1, overlap_percent: 20 }, panel: null }] }, rigName))
      .toEqual(['C925 framed separately']);
  });

  it('names plan changes by bandpass and rig', () => {
    const before: DirectorPlanDraft = { project_id: 'p', revision: 3, updated_at_ms: 1,
      objectives: [{ id: 'o1', bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'frames', value: 40 }, priority: 1 }],
      contributions: [{ id: 'c1', objective_id: 'o1', rig_id: 'rig-a', template: { template_guid: null, template_id: 1, name: 'Ha', filter_name: 'Ha', gain: 100, offset: 30, bin: 1, readout_mode: null }, exposure_seconds: 300, panel_ids: [], enabled: true }] };
    expect(describePlanChanges(before, before)).toEqual([]);
    const after: DirectorPlanDraft = {
      ...before,
      objectives: [{ ...before.objectives[0], goal: { kind: 'frames', value: 120 } }, { id: 'o2', bandpass_id: 'oiii', purpose: 'faint_detail', goal: { kind: 'hours', value: 6 }, priority: 1 }],
      contributions: [{ ...before.contributions[0], exposure_seconds: 600 }, { ...before.contributions[0], id: 'c2', rig_id: 'rig-b' }],
    };
    const changes = describePlanChanges(before, after, rigName);
    expect(changes).toContain('H-alpha goal 40 frames per rig → 120 frames per rig');
    expect(changes).toContain('RedCat H-alpha exposure 300 s → 600 s');
    expect(changes).toContain('C925 H-alpha added');
    expect(changes.some(change => change.endsWith('added, 6 h'))).toBe(true);
    expect(describePlanChanges(after, before, rigName)).toContain('C925 H-alpha removed');
  });
});
