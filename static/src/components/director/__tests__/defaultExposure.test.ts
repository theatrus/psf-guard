import { describe, expect, it } from 'vitest';
import type { DirectorRigProfileSummary, DirectorTemplate } from '../../../api/directorTypes';
import { defaultExposure } from '../planModel';

const rig = { default_exposure_seconds: { broadband: 120, narrowband: 300 } } as DirectorRigProfileSummary;
const template = (seconds: number) => ({ default_exposure: seconds } as DirectorTemplate);

describe("a rig's exposure for a template", () => {
  it("takes the template's own default when it is a usable sub length", () => {
    expect(defaultExposure(rig, 'broadband', template(180))).toBe(180);
    expect(defaultExposure(rig, 'broadband', template(1))).toBe(1);
  });

  it("falls back to the rig's for the band when the template's is under a second", () => {
    // A light template an import seeded from flats or test frames.
    expect(defaultExposure(rig, 'broadband', template(0.4))).toBe(120);
    expect(defaultExposure(rig, 'narrowband', template(0))).toBe(300);
    expect(defaultExposure(rig, 'narrowband', null)).toBe(300);
  });
});
