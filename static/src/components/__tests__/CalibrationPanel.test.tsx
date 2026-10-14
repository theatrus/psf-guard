import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import CalibrationPanel from '../CalibrationPanel';
import type { CalibrationKindExplanation, ImageCalibrationExplanation } from '../../api/types';

function kind(
  name: CalibrationKindExplanation['kind'],
  extra: Partial<CalibrationKindExplanation> = {},
): CalibrationKindExplanation {
  return {
    kind: name,
    master: { frames: 20, nights: 1, temp_min_c: -10, temp_max_c: -10, first_night: '2026-09-28', last_night: '2026-09-28' },
    used: [{ night: '2026-09-28', frames: 20, temp_min_c: -10, temp_max_c: -10 }],
    matching_unused: [],
    refused: [],
    kept_out: [],
    suspect: [],
    other_filters: 0,
    other_cameras: 0,
    ...extra,
  };
}

const explanation: ImageCalibrationExplanation = {
  light: {
    file: 'light.fits', night: '2026-10-06', filter: 'Ha', exposure_s: 300, gain: 100, offset: 50,
    binning: '1x1', camera_temp: -10, rotator_deg: null, camera: 'Cam', telescope: null, focal_length_mm: null,
  },
  findings: [
    { code: 'thin_dark', level: 'poor', reason: 'Its dark master would use only 3 frames', kinds: ['dark'] },
    { code: 'mixed_flats', level: 'check', reason: 'Its flat master mixes flats from 2 nights', kinds: ['flat'] },
  ],
  kinds: [
    kind('bias'),
    kind('dark', { master: { frames: 3, nights: 1 }, used: [{ night: '2026-09-28', frames: 3 }] }),
    kind('dark_flat', { master: { frames: 0, nights: 0 }, used: [] }),
    kind('flat', {
      master: { frames: 46, nights: 2 },
      used: [
        { night: '2026-10-06', frames: 6 },
        { night: '2026-10-01', frames: 40 },
      ],
    }),
  ],
};

function Where() {
  const location = useLocation();
  return <output data-testid="where">{location.search}</output>;
}

function show(route: string, data: ImageCalibrationExplanation = explanation) {
  server.use(
    http.get('/api/db/:dbId/images/:imageId/calibration', () =>
      HttpResponse.json({ success: true, error: null, status: 'ready', data })),
  );
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[route]}>
        {children}
        <Where />
      </MemoryRouter>
    </QueryClientProvider>
  );
  render(<CalibrationPanel dbId="c925" imageId={8} projectId={1} />, { wrapper: Wrapper });
}

const focus = () => new URLSearchParams(screen.getByTestId('where').textContent!).get('calibFocus');

describe('CalibrationPanel', () => {
  it('opens on the finding Images was filtered by and picks out its nights', async () => {
    show('/detail/8?db=c925&project=1&calib=mixed_flats');
    const mixed = await screen.findByRole('button', { name: /mixes flats from 2 nights/ });
    expect(mixed).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: /only 3 frames/ })).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByText(/One master averages flats from different nights/)).toBeInTheDocument();

    const flats = screen.getByTestId('calibration-kind-flat');
    expect(flats).toHaveClass('is-about', 'is-check');
    const nights = within(flats).getAllByRole('listitem');
    expect(nights.map((night) => night.textContent)).toEqual([
      "2026-10-066 framesthe light's night",
      '2026-10-0140 frames5 days from the light',
    ]);
    expect(nights.every((night) => night.classList.contains('is-focus'))).toBe(true);
    // The kinds it is not about fold to one line; one with nothing is left out.
    expect(screen.getByTestId('calibration-kind-dark')).toHaveClass('is-folded');
    expect(within(screen.getByTestId('calibration-kind-dark')).queryByRole('listitem')).toBeNull();
    expect(screen.queryByTestId('calibration-kind-dark_flat')).toBeNull();

    // A night opens its frames on the Calibration page, without the detail's focus.
    const link = within(nights[1]).getByRole('link', { name: '2026-10-01' });
    const href = new URL(link.getAttribute('href')!, 'http://x');
    expect(href.pathname).toBe('/calibration');
    expect(Object.fromEntries(href.searchParams)).toMatchObject({
      db: 'c925', project: '1', calKind: 'flat', calFilter: 'Ha', calFrom: '2026-10-01', calTo: '2026-10-01',
    });
    expect(href.searchParams.get('calibFocus')).toBeNull();
  });

  it('keeps the chosen finding in the URL and can show every kind', async () => {
    show('/detail/8?db=c925&project=1');
    // With nothing chosen, the worst finding shows.
    const thin = await screen.findByRole('button', { name: /only 3 frames/ });
    expect(thin).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByTestId('calibration-kind-dark')).toHaveClass('is-about', 'is-poor');
    expect(screen.getByTestId('calibration-kind-flat')).toHaveClass('is-folded');

    fireEvent.click(screen.getByRole('button', { name: /mixes flats from 2 nights/ }));
    await waitFor(() => expect(focus()).toBe('mixed_flats'));
    expect(screen.getByTestId('calibration-kind-dark')).toHaveClass('is-folded');

    fireEvent.click(screen.getByRole('button', { name: 'Show every kind' }));
    await waitFor(() => expect(focus()).toBe('none'));
    expect(document.querySelectorAll('.light-calibration-kind.is-folded')).toHaveLength(0);
    expect(screen.getByText(/Choose a finding/)).toBeInTheDocument();

    // Choosing the same finding again lets it go.
    fireEvent.click(screen.getByRole('button', { name: /only 3 frames/ }));
    await waitFor(() => expect(focus()).toBe('thin_dark'));
    fireEvent.click(screen.getByRole('button', { name: /only 3 frames/ }));
    await waitFor(() => expect(focus()).toBe('none'));
  });

  it('says so when the library has no finding for the light', async () => {
    show('/detail/8?db=c925&project=1', { ...explanation, findings: [] });
    expect(await screen.findByText(/calibrates this light with no findings/)).toBeInTheDocument();
    expect(screen.getByText('No findings')).toBeInTheDocument();
    expect(document.querySelectorAll('.light-calibration-kind.is-folded')).toHaveLength(0);
  });
});
