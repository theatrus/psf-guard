import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
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

  it('names the nights stacks leave out, then rejects the lights after a check', async () => {
    const report = { warnings: [], kinds: [], lights_missing_files: 0, nights: [{ night: '2026-07-01', lights: 29, filters: [{
      ...filter, filter: 'SII', flat_frames: 0, dark_master_frames: 2, dark_master_nights: 2, external_masters: [],
      cannot_calibrate: 'No matching flat: the nearest SII flats are 70° off its rotation',
    }] }] };
    const gaps = { checked: 30, missing_files: 0, digest: 'list-one', lights: [
      { image_id: 1, filter: 'SII', night: '2026-07-01', reason: 'No matching flat: the nearest SII flats are 70° off its rotation' },
      { image_id: 2, filter: 'SII', night: '2026-07-01', reason: 'No matching flat: the nearest SII flats are 70° off its rotation' },
    ] };
    let sent: unknown = null;
    server.use(
      http.get('/api/db/c925/projects/1/calibration-report', () => HttpResponse.json({ success: true, error: null, data: report })),
      http.get('/api/db/c925/projects/1/calibration-report/rejects', () => HttpResponse.json({ success: true, error: null, data: gaps })),
      http.post('/api/db/c925/projects/1/calibration-report/rejects', async ({ request }) => {
        sent = await request.json();
        return HttpResponse.json({ success: true, error: null, data: { updated: 2, previous: [] } });
      }),
    );
    const user = userEvent.setup();
    render(<CalibrationReportDialog open dbId="c925" projectId={1} projectName="Bubble" onClose={() => {}} />, { wrapper });
    expect(await screen.findByText('SII: left out of stacks · No matching flat: the nearest SII flats are 70° off its rotation')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Check lights' }));
    expect(await screen.findByText("2 lights of 30 can't be calibrated.")).toBeInTheDocument();
    expect(screen.getByText('2026-07-01 · SII · No matching flat: the nearest SII flats are 70° off its rotation: 2')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Reject 2 lights' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Rejected 2 lights; Target Scheduler will shoot them again.');
    expect(sent).toEqual({ digest: 'list-one' });
  });

  it('asks for a fresh check when the lights changed since', async () => {
    server.use(
      http.get('/api/db/c925/projects/1/calibration-report', () => HttpResponse.json({ success: true, error: null, data: { warnings: [], kinds: [], lights_missing_files: 0, nights: [] } })),
      http.get('/api/db/c925/projects/1/calibration-report/rejects', () => HttpResponse.json({ success: true, error: null, data: {
        checked: 1, missing_files: 0, digest: 'old', lights: [{ image_id: 1, filter: 'SII', night: '2026-07-01', reason: 'No matching bias or dark' }],
      } })),
      http.post('/api/db/c925/projects/1/calibration-report/rejects', () => HttpResponse.json({ success: false, error: 'changed', data: null }, { status: 409 })),
    );
    const user = userEvent.setup();
    render(<CalibrationReportDialog open dbId="c925" projectId={1} projectName="Bubble" onClose={() => {}} />, { wrapper });
    await user.click(await screen.findByRole('button', { name: 'Check lights' }));
    await user.click(await screen.findByRole('button', { name: 'Reject 1 light' }));
    expect(await screen.findByRole('status')).toHaveTextContent('The lights changed since the check; check again.');
    expect(screen.queryByRole('button', { name: 'Reject 1 light' })).toBeNull();
  });
});
