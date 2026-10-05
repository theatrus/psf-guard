import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import CalibrationReportDialog from '../CalibrationReportDialog';

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

const filter = {
  lights: 25, bias_frames: 9, dark_frames: 99, dark_age_days: 0.2, dark_flat_frames: 0,
  flat_session: null, flat_age_days: null, nightly_flats: false, missing: ['flat'],
};

describe('CalibrationReportDialog', () => {
  it('says why a night has no flats, what a dark master uses, and how each external master fares', async () => {
    server.use(http.get('/api/db/ultracat/projects/3/calibration-report', () => HttpResponse.json({ success: true, error: null, data: {
      warnings: [], kinds: [], lights_missing_files: 0,
      nights: [{ night: '2026-10-03', lights: 25, filters: [{
        ...filter, filter: 'HA', flat_frames: 0, dark_master_frames: 6, dark_master_nights: 1,
        flat_near_miss: { filter: 'HA', flat_rotation_deg: 94.7, light_rotation_deg: 92.6, off_by_deg: 2.1, tolerance_deg: 2, session: '2026-10-01', frames: 10 },
        external_masters: [{ kind: 'flat', file: 'masterFlat_HA.xisf', matches: false, used: false, reason: 'rotation light=92.6deg master=60deg (32.60 deg apart, tolerance 2.00)' }],
      }] }],
    } })));
    render(<CalibrationReportDialog open dbId="ultracat" projectId={3} projectName="California Nebula" onClose={() => {}} />, { wrapper });
    expect(await screen.findByText('none · nearest HA 94.7°, 2.1° off (limit 2°) · 2026-10-01')).toBeInTheDocument();
    expect(screen.getByText('6 · same night')).toHaveAttribute('title', '99 matching darks within reach');
    expect(screen.getByText(/flat master masterFlat_HA\.xisf · not used: rotation light=92\.6deg/)).toBeInTheDocument();
  });
});
