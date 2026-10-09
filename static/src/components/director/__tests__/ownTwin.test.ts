import { describe, expect, it } from 'vitest';
import type { DirectorPlanDraft, DirectorTemplate, DirectorTemplateChoice } from '../../../api/directorTypes';
import { bindOwnTemplates, ownTwin } from '../planModel';
import { defaultMoonPolicy } from '../moonPolicy';

const moon = defaultMoonPolicy();
const own: DirectorTemplate = { id: 4, guid: '00000000-0000-4000-8000-000000000004', profile_id: 'p', name: 'Ha 300', filter_name: 'Ha', gain: null, offset: 30, bin: null, readout_mode: null,
  default_exposure: 300, moon, bandpass: { id: 'h_alpha', name: 'H-alpha', kind: 'narrowband' } };
const choice: DirectorTemplateChoice = { template_guid: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', template_id: null, name: 'Ha shared', filter_name: ' ha ', gain: -1, offset: 30, bin: 0, readout_mode: -1, moon };

describe("a library choice the rig's database already holds", () => {
  it('matches the row activation would pick: unset values, bin 1 and filter case are the same', () => {
    expect(ownTwin(choice, [own])).toBe(own);
    expect(ownTwin({ ...choice, moon: undefined }, [own])).toBe(own);
  });

  it('does not match other settings or Moon rules', () => {
    expect(ownTwin({ ...choice, gain: 100 }, [own])).toBeUndefined();
    expect(ownTwin({ ...choice, bin: 2 }, [own])).toBeUndefined();
    expect(ownTwin({ ...choice, filter_name: 'OIII' }, [own])).toBeUndefined();
    expect(ownTwin({ ...choice, moon: { ...moon, separation_degrees: moon.separation_degrees + 10 } }, [own])).toBeUndefined();
  });

  it('matches the row written under its GUID even after the row changed', () => {
    expect(ownTwin({ ...choice, template_guid: own.guid!.toUpperCase(), gain: 200 }, [own])).toBe(own);
    // The GUID names a row now holding another filter: no match.
    expect(ownTwin({ ...choice, template_guid: own.guid, gain: 200 }, [{ ...own, filter_name: 'OIII' }])).toBeUndefined();
  });

  it('binds the plan to the row and keeps the exposure; an unchanged plan comes back as is', () => {
    const plan: DirectorPlanDraft = { project_id: 'p', revision: 1, updated_at_ms: 1, objectives: [], contributions: [
      { id: 'c', objective_id: 'o', rig_id: 'rig', template: choice, exposure_seconds: 240, panel_ids: [], enabled: true },
    ] };
    const bound = bindOwnTemplates(plan, { rig: [own] });
    expect(bound.contributions[0]).toMatchObject({ exposure_seconds: 240, template: { template_id: 4, template_guid: own.guid, name: 'Ha 300' } });
    expect(bindOwnTemplates(bound, { rig: [own] })).toBe(bound);
    expect(bindOwnTemplates(plan, { other: [own] })).toBe(plan);
  });
});
