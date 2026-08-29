import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import CalibrationLibraryDialog from '../CalibrationLibraryDialog';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

// Two flats one night, one flat two nights later — around a dust cleaning.
// 2026-06-01 20:00 UTC and 2026-06-03 20:00 UTC.
const earlyNight = Math.floor(Date.UTC(2026, 5, 1, 20) / 1000);
const lateNight = Math.floor(Date.UTC(2026, 5, 3, 20) / 1000);

function flat(uuid: string, capturedAt: number, validDirection: string | null = null) {
  return {
    frame_uuid: uuid,
    rig_uuid: 'rig-1',
    kind: 'flat',
    source_path: `/calibration/${uuid}.fits`,
    source_exists: true,
    captured_at: capturedAt,
    camera: 'Camera',
    exposure_s: 3,
    valid_direction: validDirection,
  };
}

const summary = {
  schema_version: 4,
  frame_count: 3,
  master_count: 0,
  rigs: [
    { rig_uuid: 'rig-1', name: 'Rig', frame_count: 3, bias: 0, dark: 0, dark_flat: 0, flat: 3 },
  ],
};

function serveLibrary(frames: unknown[]) {
  server.use(
    http.get('/api/db/demo/calibrations/details', () =>
      HttpResponse.json({ success: true, data: { summary, frames }, error: null })
    ),
    http.get('/api/db/demo/calibrations', () =>
      HttpResponse.json({ success: true, data: summary, error: null })
    )
  );
}

describe('calibration validity marking', () => {
  it('groups frames by imaging night and marks a selected night forward', async () => {
    serveLibrary([
      flat('old-a', earlyNight),
      flat('old-b', earlyNight + 600),
      flat('new-a', lateNight),
    ]);
    let received: unknown = null;
    server.use(
      http.put('/api/db/demo/calibrations/frames/validity', async ({ request }) => {
        received = await request.json();
        return HttpResponse.json({ success: true, data: { updated: 2 }, error: null });
      })
    );

    render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );

    // One group per night, newest first.
    expect(await screen.findByText('Night of 2026-06-03')).toBeInTheDocument();
    expect(screen.getByText('Night of 2026-06-01')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('checkbox', { name: 'Select Flats Night of 2026-06-01' }));
    expect(screen.getByText(/2 frames across 1 group/)).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('Validity direction'), {
      target: { value: 'backward' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Mark' }));

    await waitFor(() =>
      expect(received).toEqual({
        frame_uuids: ['old-a', 'old-b'],
        direction: 'backward',
      })
    );
  });

  it('shows the validity badge on marked frames', async () => {
    serveLibrary([
      flat('old-a', earlyNight, 'backward'),
      flat('new-a', lateNight, 'forward'),
    ]);
    render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );
    expect(await screen.findByText('◂ before only')).toBeInTheDocument();
    expect(screen.getByText('▸ after only')).toBeInTheDocument();
  });

  it('starts collapsed, separates darks from flats, and forgets a whole night', async () => {
    serveLibrary([
      flat('flat-a', earlyNight),
      { ...flat('dark-a', earlyNight), kind: 'dark' },
      { ...flat('bias-a', earlyNight), kind: 'bias' },
    ]);
    let received: unknown = null;
    server.use(
      http.delete('/api/db/demo/calibrations/frames', async ({ request }) => {
        received = await request.json();
        return HttpResponse.json({
          success: true,
          data: { frames_removed: 1, masters_removed: 0 },
          error: null,
        });
      })
    );

    const { container } = render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );

    // Darks and bias sit in their own sections beside the flat nights: the
    // same night appears once per section, not merged.
    expect(await screen.findByText('Flats')).toBeInTheDocument();
    const sectionRows = container.querySelectorAll('.calibration-section-row');
    expect([...sectionRows].map((row) => row.querySelector('strong')?.textContent)).toEqual([
      'Flats',
      'Darks',
      'Bias',
    ]);
    expect(screen.getAllByText('Night of 2026-06-01')).toHaveLength(3);

    // Collapsed first: no frame rows until a group is expanded.
    expect(screen.queryByText('flat-a.fits')).toBeNull();
    expect(screen.queryByText('dark-a.fits')).toBeNull();
    const toggles = screen.getAllByRole('button', { name: /Night of 2026-06-01/ });
    fireEvent.click(toggles[0]);
    expect(await screen.findByText('flat-a.fits')).toBeInTheDocument();
    expect(screen.queryByText('dark-a.fits')).toBeNull();

    // Forget night is destructive: it only appears once a night's checkbox
    // is selected, and names only that section's frames.
    expect(screen.queryByRole('button', { name: 'Forget night' })).toBeNull();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select Darks Night of 2026-06-01' }));
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    fireEvent.click(screen.getByRole('button', { name: 'Forget night' }));
    await waitFor(() => expect(received).toEqual({ frame_uuids: ['dark-a'] }));
    confirm.mockRestore();
  });

  it('offers no selection to a read-only viewer', async () => {
    serveLibrary([flat('old-a', earlyNight)]);
    render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage={false} onClose={() => {}} />,
      { wrapper: wrapper() }
    );
    expect(await screen.findByText('Night of 2026-06-01')).toBeInTheDocument();
    expect(screen.queryByRole('checkbox', { name: /Select Flats/ })).toBeNull();
  });
});
