import { beforeEach, describe, expect, it, vi } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';

const { getServerUrl } = vi.hoisted(() => ({ getServerUrl: vi.fn() }));
vi.mock('../../utils/tauri', () => ({ getServerUrl }));

beforeEach(() => vi.resetModules());

describe('Sky coverage preview URLs', () => {
  it.each([
    { environment: 'desktop', origin: 'http://localhost:49537' },
    { environment: 'browser', origin: '' },
  ])('loads mono and color previews from the backend in $environment', async ({ origin }) => {
    getServerUrl.mockResolvedValue(origin);
    const preview = {
      url: '/api/db/rig%20one/stack-previews/job/0/preview?v=revision-1',
      width: 9576,
      height: 6388,
      kind: 'mono',
      filter: 'B',
      key: 'mono:B',
      label: 'B',
      wcs: { crpix1: 4788, crpix2: 3194, crval1: 130, crval2: 20,
        cd11: -0.0004, cd12: 0, cd21: 0, cd22: 0.0004 },
    };
    const color = { ...preview, kind: 'color', filter: null, key: 'color:RGB', label: 'RGB',
      url: '/api/db/rig%20one/stack-previews/color/job/preview?v=revision-2' };
    const coverage = {
      targets: [
        { id: 1, name: 'M44', preview },
        { id: 2, name: 'M42', preview: color },
        { id: 3, name: 'Not stacked', preview: null },
        { id: 4, name: 'Older response without a preview' },
      ],
      nights: [{ night: '2026-04-16', target_id: 1, filter: 'B', frames: 3, seconds: 180 }],
      filters: ['B'],
      totals: { frames: 3, seconds: 180 },
    };
    let requestedOrigin: string | undefined;
    server.use(http.get('*/api/db/:dbId/sky/coverage', ({ request }) => {
      requestedOrigin = new URL(request.url).origin;
      return HttpResponse.json({ success: true, data: coverage }, {
        headers: { 'Access-Control-Allow-Origin': '*' },
      });
    }));

    const { apiClient } = await import('../client');
    const result = await apiClient.getSkyCoverage('rig one');

    expect(requestedOrigin).toBe(origin || window.location.origin);
    expect(result).toEqual({
      ...coverage,
      targets: [
        { ...coverage.targets[0], preview: { ...preview, url: `${origin}${preview.url}` } },
        { ...coverage.targets[1], preview: { ...color, url: `${origin}${color.url}` } },
        coverage.targets[2],
        coverage.targets[3],
      ],
    });
  });
});
