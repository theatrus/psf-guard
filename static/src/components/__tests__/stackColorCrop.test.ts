import { describe, expect, it } from 'vitest';
import { cropLabels, cropOrder, describeCrop, offCenterChannels } from '../stackColorCrop';
import type { StackColorCropReport, StackColorJob } from '../../api/types';

function job(crop_report: StackColorCropReport | null): StackColorJob {
  return { crop: 'inscribed', crop_report } as unknown as StackColorJob;
}

const report: StackColorCropReport = {
  grid_width: 128,
  grid_height: 128,
  x: 0,
  y: 80,
  width: 128,
  height: 48,
  retained_fraction: 48 / 128,
  channels: [
    {
      role: 'red',
      name: 'red',
      covered_pixels: 128 * 128,
      center_offset_pixels: 0,
      off_center: false,
    },
    {
      role: 'blue',
      name: 'blue',
      covered_pixels: 128 * 48,
      center_offset_pixels: 39.5,
      off_center: true,
    },
  ],
};

describe('stack color crop', () => {
  it('offers every mode, keeping blank edges first', () => {
    expect(cropOrder).toEqual(['none', 'bounds', 'inscribed']);
    expect(cropLabels.none).toBe('Keep blank edges');
  });

  it('describes what a crop kept', () => {
    expect(describeCrop(job(report))).toBe('Cropped to 128×48 of 128×128 · 38% kept');
  });

  it('says nothing about a preview that kept the whole grid', () => {
    expect(describeCrop(job(null))).toBeNull();
    expect(offCenterChannels(job(null))).toEqual([]);
  });

  it('names only the channel that bounded the crop', () => {
    expect(offCenterChannels(job(report)).map((channel) => channel.role)).toEqual(['blue']);
  });
});
