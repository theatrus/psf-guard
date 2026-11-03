import { describe, expect, it } from 'vitest';
import type { Image, ImageQualityResult } from '../../api/types';
import { formatReading, starReadings, usedReading } from '../starReadings';

const image = (metadata: Record<string, unknown>) => ({ metadata }) as unknown as Image;
const quality = (stars: ImageQualityResult['stars']) => ({ stars }) as ImageQualityResult;

describe('a frame\'s star readings', () => {
  it('keeps both sources and the one scoring used', () => {
    const readings = starReadings('hfr', quality({
      capture: { star_count: 4516, hfr: 1.43 },
      scan: { star_count: 934, hfr: 3.457553288787262 },
      star_count_source: 'scan',
      hfr_source: 'scan',
      compared_on: 'scan',
    }), image({ HFR: 1.43 }));
    expect(readings).toEqual({ capture: 1.43, scan: 3.457553288787262, used: 'scan' });
    expect(formatReading('hfr', usedReading(readings)!)).toBe('3.46');
  });

  it('reads N.I.N.A.\'s value from the metadata before scoring has run', () => {
    expect(starReadings('stars', undefined, image({ DetectedStars: 4516 })))
      .toEqual({ capture: 4516, scan: null, used: 'capture' });
  });

  it('marks a value the scan wrote into an import\'s metadata as PSF Guard\'s', () => {
    const imported = image({ DetectedStars: 600, HFR: 2.1, PsfGuardQualityFields: ['HFR'] });
    expect(starReadings('hfr', undefined, imported).used).toBe('scan');
    expect(starReadings('stars', undefined, imported).used).toBe('capture');
  });

  it('reads an HFR of zero as no measurement', () => {
    expect(starReadings('hfr', undefined, image({ HFR: 0 })).used).toBeNull();
  });
});
