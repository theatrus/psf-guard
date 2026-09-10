import { describe, expect, it } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { apiClient } from '../client';
import type { CalibrationSettings } from '../types';

const settings = (enabled: boolean): CalibrationSettings => ({
  rotation_tolerance_deg: 3.5,
  default_rotation_tolerance_deg: 2,
  external_masters: 'fallback',
  flat_star_masking: enabled,
});

describe('calibration settings API', () => {
  it.each([false, true])('loads flat masking as %s', async (enabled) => {
    server.use(http.get('/api/settings/calibration', () => HttpResponse.json({
      success: true, data: settings(enabled), error: null,
    })));
    expect(await apiClient.getCalibrationSettings()).toEqual(settings(enabled));
  });

  it.each([false, true])('sends explicit flat masking %s alongside matching settings', async (enabled) => {
    let received: unknown;
    server.use(http.put('/api/settings/calibration', async ({ request }) => {
      received = await request.json();
      return HttpResponse.json({ success: true, data: settings(enabled), error: null });
    }));
    const update = {
      rotation_tolerance_deg: 3.5,
      external_masters: 'fallback' as const,
      flat_star_masking: enabled,
    };
    expect(await apiClient.updateCalibrationSettings(update)).toEqual(settings(enabled));
    expect(received).toEqual(update);
  });

  it('propagates a failed settings load', async () => {
    server.use(http.get('/api/settings/calibration', () => HttpResponse.json({
      success: false, data: null, error: 'Registry unavailable',
    })));
    await expect(apiClient.getCalibrationSettings()).rejects.toThrow('Registry unavailable');
  });

  it('propagates a failed settings save', async () => {
    server.use(http.put('/api/settings/calibration', () => HttpResponse.json({
      success: false, data: null, error: 'Could not save the registry',
    })));
    await expect(apiClient.updateCalibrationSettings({
      rotation_tolerance_deg: null,
      external_masters: 'prefer',
      flat_star_masking: true,
    })).rejects.toThrow('Could not save the registry');
  });
});
