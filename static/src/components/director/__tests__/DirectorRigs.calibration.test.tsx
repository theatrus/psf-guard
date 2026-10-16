import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it } from 'vitest';
import { server } from '../../../test/msw-server';
import { OPEN_SETTINGS_EVENT, type OpenSettingsDetail } from '../../../utils/settingsIntent';
import DirectorRigs from '../DirectorRigs';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });

const asked: OpenSettingsDetail[] = [];
const listen = (event: Event) => asked.push((event as CustomEvent<OpenSettingsDetail>).detail);

afterEach(() => {
  window.removeEventListener(OPEN_SETTINGS_EVENT, listen);
  asked.length = 0;
});

describe('DirectorRigs', () => {
  it('links each catalog to the calibration settings of the rigs it records', async () => {
    server.use(
      http.get('/api/databases', () => ok([
        { id: 'c925', name: 'C925', database_path: '/c925.sqlite', image_directories: [] },
        { id: 'ultracat', name: 'Ultracat', database_path: '/ultracat.sqlite', image_directories: [] },
      ])),
      http.get('/api/director/v1/rigs/profiles', () => ok([])),
      http.get('/api/director/v1/rigs/status', () => ok([])),
      http.get('/api/settings/calibration', () => ok({
        rigs: [{
          rig_uuid: 'rig-1', name: 'ZWO ASI2600MM Pro', detail: '6248×4176',
          databases: ['C925'], database_ids: ['c925'], frames: 120,
          rules: { dark_reach_days: 90 },
        }],
      })),
    );
    window.addEventListener(OPEN_SETTINGS_EVENT, listen);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <MemoryRouter><DirectorRigs /></MemoryRouter>
      </QueryClientProvider>,
    );

    const link = await screen.findByRole('button', { name: 'ZWO ASI2600MM Pro · 6248×4176 · 1 own value' });
    // Only the catalog that records the rig links to it.
    expect(screen.getAllByText('Calibration:')).toHaveLength(1);
    fireEvent.click(link);
    expect(asked.at(-1)).toEqual({ intent: 'calibration', rig: { uuid: 'rig-1', name: 'ZWO ASI2600MM Pro' } });
  });
});
