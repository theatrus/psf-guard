import { describe, expect, it } from 'vitest';
import type { FlatFamily, FlatGroupsRig } from '../../../api/types';
import { browseGroups, familyAngleGap } from '../flatGroupBrowse';

/** One rig whose OIII flats at 72.6° and 82.2° measured alike: one
 *  response family with two angle spans. */
const family: FlatFamily = {
  key: 'OIII:s-oiii72',
  filter: 'OIII',
  rotation: 82.2,
  angles: [{ low: 72.6, high: 72.6 }, { low: 81.2, high: 82.2 }],
  angle_label: '72.6° & 81.2–82.2°',
  groups: [{
    key: 's-oiii0901',
    first_night: '2026-09-01',
    last_night: '2026-09-05',
    frames: 40,
    sessions: [],
  }],
  splits: [],
};

const rig: FlatGroupsRig = {
  rig_uuid: 'rig',
  name: 'C925',
  measured: true,
  dust_limit_pct: 1,
  pattern_limit_pct: 2,
  angle_term: null,
  angle_pairs: 0,
  events: [],
  families: [family],
  unmeasured: [{ night: '2026-09-03', filter: 'OIII', rotation: 72.9, frames: 1, reason: 'needs 2 flats', sessions: [] }],
};

describe('response families', () => {
  it('measure an angle from the nearest of their spans', () => {
    expect(familyAngleGap(family, 72.6)).toBe(0);
    expect(familyAngleGap(family, 81.7)).toBe(0);
    expect(familyAngleGap(family, 77)).toBeCloseTo(4.2, 6);
    expect(familyAngleGap(family, null)).toBe(Number.POSITIVE_INFINITY);
    // A family that predates spans measures from its angle.
    expect(familyAngleGap({ ...family, angles: undefined }, 80)).toBeCloseTo(2.2, 6);
  });

  it('are named by their measured angle range and found at any of its angles', () => {
    const browse = browseGroups([rig]);
    expect(browse.families.map((entry) => entry.label)).toEqual(['OIII · 72.6° & 81.2–82.2°']);
    // An unmeasured night at 72.9° joins the family, though its middle
    // angle is 82.2°.
    expect(browse.entries[0].unmeasured.map((session) => session.night)).toEqual(['2026-09-03']);
    expect(browseGroups([rig], { rotation: 72.6 }).entries).toHaveLength(1);
    expect(browseGroups([rig], { rotation: 77 }).entries).toHaveLength(0);
  });
});
