import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { http, HttpResponse } from 'msw';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import CalibrationReportDialog from '../CalibrationReportDialog';

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return <QueryClientProvider client={client}><MemoryRouter>{children}</MemoryRouter></QueryClientProvider>;
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
    expect(await screen.findByText("SII: can't be calibrated · No matching flat: the nearest SII flats are 70° off its rotation")).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Check lights' }));
    expect(await screen.findByText("2 lights of 30 can't be calibrated.")).toBeInTheDocument();
    expect(screen.getByText('2026-07-01 · SII · No matching flat: the nearest SII flats are 70° off its rotation: 2')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Reject 2 lights' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Rejected 2 lights; Target Scheduler will shoot them again.');
    expect(sent).toEqual({ digest: 'list-one' });
  });

  it('leaves rejected lights out until asked to include them', async () => {
    const night = (date: string) => ({ night: date, lights: 25, filters: [{ ...filter, filter: 'HA', flat_frames: 0, dark_master_frames: 6, dark_master_nights: 1, external_masters: [] }] });
    const asked: string[] = [];
    server.use(http.get('/api/db/c925/projects/1/calibration-report', ({ request }) => {
      const include = new URL(request.url).searchParams.get('include_rejected');
      asked.push(include ?? 'default');
      return HttpResponse.json({ success: true, error: null, data: {
        warnings: [], kinds: [], lights_missing_files: 0, rejected_lights: 25, rejected_included: include === 'true',
        nights: include === 'true' ? [night('2026-07-02'), night('2026-07-01')] : [night('2026-07-01')],
      } });
    }));
    const user = userEvent.setup();
    render(<CalibrationReportDialog open dbId="c925" projectId={1} projectName="Bubble" onClose={() => {}} />, { wrapper });
    expect(await screen.findByText('2026-07-01')).toBeInTheDocument();
    expect(screen.queryByText('2026-07-02')).not.toBeInTheDocument();

    await user.click(screen.getByRole('checkbox', { name: 'Include 25 rejected lights' }));
    expect(await screen.findByText('2026-07-02')).toBeInTheDocument();
    expect(asked).toEqual(['default', 'true']);
  });

  it('opens the lights of a finding or a night in the Images grid', async () => {
    const health = {
      project_id: 1, level: 'poor', checked: 362, missing_files: 0, poor_lights: 120, check_lights: 17,
      findings: [
        { code: 'cannot_calibrate', level: 'poor', reason: 'No matching flat: the nearest OIII flats are 61° off its rotation', lights: 120,
          nights: [{ night: '2026-07-01', filter: 'OIII', lights: 15 }, { night: '2026-06-30', filter: 'OIII', lights: 105 }], image_ids: [] },
        { code: 'old_flats', level: 'check', reason: 'Its flats are 52 days from it', lights: 17,
          nights: [{ night: '2026-10-09', filter: 'SII', lights: 6 }, { night: '2026-10-09', filter: 'HA', lights: 11 }], image_ids: [] },
      ],
    };
    server.use(
      http.get('/api/db/c925/projects/1/calibration-health', () => HttpResponse.json({ success: true, error: null, data: { health, computed_at: 1, running: false } })),
      http.get('/api/db/c925/projects/1/calibration-report', () => HttpResponse.json({ success: true, error: null, data: {
        warnings: [], kinds: [], lights_missing_files: 0,
        nights: [{ night: '2026-07-02', lights: 42, filters: [{ ...filter, filter: 'SII', flat_frames: 0, dark_master_frames: 6, dark_master_nights: 1, external_masters: [],
          cannot_calibrate: 'No matching flat: the nearest SII flats are 70° off its rotation' }] }],
      } })),
    );
    function Where() {
      const location = useLocation();
      return <output data-testid="where">{location.pathname}{location.search}</output>;
    }
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    let closed = 0;
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={client}>
        <MemoryRouter initialEntries={['/?db=c925']}>
          <Routes>
            <Route path="*" element={<><CalibrationReportDialog open dbId="c925" projectId={1} projectName="Bubble" onClose={() => { closed += 1; }} /><Where /></>} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );

    await user.click(await screen.findByRole('button', { name: 'No matching flat: the nearest OIII flats are 61° off its rotation: 120 lights' }));
    expect(screen.getByTestId('where')).toHaveTextContent('/grid?db=c925&project=1&calib=cannot_calibrate&filter=OIII');
    await user.click(screen.getByRole('button', { name: 'Its flats are 52 days from it: 17 lights' }));
    expect(screen.getByTestId('where')).toHaveTextContent('/grid?db=c925&project=1&calib=old_flats&filter=HA%2CSII');
    await user.click(await screen.findByRole('button', { name: "SII: can't be calibrated · No matching flat: the nearest SII flats are 70° off its rotation" }));
    expect(screen.getByTestId('where')).toHaveTextContent('/grid?db=c925&project=1&calib=cannot_calibrate&filter=SII');
    expect(closed).toBe(3);
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

  it('says what the calibration check found, by night and filter, or that it has not run', async () => {
    const report = { warnings: [], kinds: [], nights: [], lights_missing_files: 0 };
    server.use(
      http.get('/api/db/:dbId/projects/:id/calibration-report', () => HttpResponse.json({ success: true, error: null, data: report })),
      http.get('/api/db/c925/projects/1/calibration-health', () => HttpResponse.json({ success: true, error: null, data: {
        running: false, computed_at: 1, health: {
          project_id: 1, level: 'poor', checked: 30, missing_files: 2, poor_lights: 6, check_lights: 3,
          findings: [
            { code: 'thin_dark', level: 'poor', reason: 'Its dark master would use only 2 frames', lights: 6,
              nights: [{ night: '2026-07-02', filter: 'SII', lights: 4 }, { night: '2026-07-01', filter: 'SII', lights: 2 }] },
            { code: 'old_flats', level: 'check', reason: 'Its flats are 41 days from it', lights: 3,
              nights: [{ night: '2026-07-01', filter: 'OIII', lights: 3 }] },
          ],
        },
      } })),
    );
    const { unmount } = render(<CalibrationReportDialog open dbId="c925" projectId={1} projectName="Bubble" onClose={() => {}} />, { wrapper });
    const quality = await screen.findByRole('region', { name: 'Quality' });
    expect(await within(quality).findByText("6 of 30 lights can't be calibrated properly; 3 more to check.")).toBeInTheDocument();
    expect(within(quality).getByText('Its dark master would use only 2 frames: 6 lights')).toBeInTheDocument();
    expect(within(quality).getByText('2026-07-02 SII: 4 · 2026-07-01 SII: 2')).toBeInTheDocument();
    expect(within(quality).getByText('Check')).toHaveClass('is-check');
    expect(within(quality).getByText('2 lights not found or not readable.')).toBeInTheDocument();
    unmount();

    render(<CalibrationReportDialog open dbId="redcat" projectId={7} projectName="Heart" onClose={() => {}} />, { wrapper });
    expect(await screen.findByText('Not checked yet.')).toBeInTheDocument();
  });
});
