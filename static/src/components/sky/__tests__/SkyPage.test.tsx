import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import SkyPage from '../SkyPage';
import type { SkyCoverage } from '../../../api/types';

const coverage = vi.hoisted(() => ({ rows: [] as unknown[] }));
vi.mock('../../../hooks/useDatabases', () => ({ useMergedSkyCoverage: () => ({ data: coverage.rows, isLoading: false, isError: false }) }));
// Director is on, as it was when the page drew the visible sky.
vi.mock('../../header/useCurrentPlan', () => ({ usePlans: () => ({ enabled: true, rows: [] }) }));
vi.mock('../../../api/client', () => ({ apiClient: { getDirectorRigSummary: async () => [] } }));

const show = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return render(<QueryClientProvider client={client}><MemoryRouter initialEntries={['/sky']}><SkyPage /></MemoryRouter></QueryClientProvider>).container;
};

const oneTarget: SkyCoverage = {
  db_id: 'rig', db_name: 'Rig', filters: ['Ha'],
  targets: [{ id: 1, project_id: 1, project_name: 'Project', name: 'M31', ra_deg: 10.68, dec_deg: 41.27, footprint: null, preview: null, filters: [{ filter: 'Ha', frames: 4, seconds: 1200, accepted_frames: 4, accepted_seconds: 1200 }], first_night: '2026-09-24', last_night: '2026-09-24', nights: 1 }],
  nights: [{ night: '2026-09-24', target_id: 1, filter: 'Ha', frames: 4, seconds: 1200, accepted_frames: 4, accepted_seconds: 1200 }],
} as unknown as SkyCoverage;

describe('the Sky view', () => {
  beforeEach(() => { coverage.rows = []; window.sessionStorage.clear(); });

  it('has no Visible sky control: that picture is in Planning', () => {
    // An empty catalog, where the control was once offered on an empty map.
    const empty = show();
    expect(screen.getByText('No targets yet. Add a database or import image folders.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Visible sky' })).toBeNull();
    expect(empty.querySelector('.sky-map')).toBeNull();
  });

  it('draws coverage and offers Live, and nothing of the visible sky, whatever an older build stored', () => {
    window.sessionStorage.setItem('psf-guard:sky-view:v1', JSON.stringify({ visibleSky: { on: true, rigId: 'rig-1', minimumMinutes: 45 } }));
    coverage.rows = [oneTarget];
    const page = show();
    expect(page.querySelector('.sky-map')).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Live' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Visible sky' })).toBeNull();
    expect(screen.queryByRole('group', { name: 'Visible sky' })).toBeNull();
    expect(page.querySelector('.sky-visible')).toBeNull();
    expect(page.querySelector('.sky-visible-shade')).toBeNull();
  });
});
