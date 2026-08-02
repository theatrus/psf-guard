import { describe, expect, it } from 'vitest';
import { isColorStackSkyOriented, isSkyOriented } from '../stackOrientation';
import type { StackColorJob, StackColorRole, StackSkyOrientation } from '../../api/types';

function orientation(convention: StackSkyOrientation['convention']): StackSkyOrientation {
  return {
    convention,
    version: 1,
    source: convention === 'source_frame' ? 'source_frame' : 'embedded_wcs',
    output_width: 512,
    output_height: 384,
    source_to_output: { matrix: [[1, 0], [0, 1]], translation_x: 0, translation_y: 0 },
  };
}

function colorJob(conventions: StackSkyOrientation['convention'][]): StackColorJob {
  const roles: StackColorRole[] = ['red', 'green', 'blue'];
  return {
    sources: conventions.map((convention, index) => ({
      role: roles[index],
      filter_name: roles[index],
      job_id: `job-${index}`,
      group_index: index,
      artifact_revision: `rev-${index}`,
      accepted_frames: 3,
      reference_image_id: index + 1,
      sky_orientation: orientation(convention),
      registration_transform: null,
    })),
  } as unknown as StackColorJob;
}

describe('stack orientation', () => {
  it('claims the sky frame only for a reprojected stack', () => {
    expect(isSkyOriented(orientation('north_up_east_left'))).toBe(true);
    expect(isSkyOriented(orientation('source_frame'))).toBe(false);
    expect(isSkyOriented(null)).toBe(false);
    expect(isSkyOriented(undefined)).toBe(false);
  });

  it('claims the sky frame for a composite only when every channel is oriented', () => {
    expect(isColorStackSkyOriented(colorJob([
      'north_up_east_left', 'north_up_east_left', 'north_up_east_left',
    ]))).toBe(true);
    expect(isColorStackSkyOriented(colorJob([
      'north_up_east_left', 'source_frame', 'north_up_east_left',
    ]))).toBe(false);
    expect(isColorStackSkyOriented(colorJob(['source_frame']))).toBe(false);
    expect(isColorStackSkyOriented(colorJob([]))).toBe(false);
    expect(isColorStackSkyOriented(undefined)).toBe(false);
  });
});
