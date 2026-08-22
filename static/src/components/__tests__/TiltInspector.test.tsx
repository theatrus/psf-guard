import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import TiltInspector from '../TiltInspector';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

/** A server response whose numbers come from the API, not the browser. */
function detectionResponse() {
  const cells = [];
  for (let row = 0; row < 3; row++) {
    for (let col = 0; col < 3; col++) {
      cells.push({
        row,
        col,
        star_count: 12,
        median_hfr: row === 0 && col === 0 ? 3.0 : 2.0,
        median_eccentricity: 0.2,
        mean_theta: null,
        theta_coherence: 0,
      });
    }
  }
  return {
    success: true,
    data: {
      detected_stars: 108,
      average_hfr: 2.1,
      average_fwhm: 4.2,
      width: 3000,
      height: 2000,
      stars: [],
      cells,
      tilt: {
        center_hfr: 2.0,
        corners: [
          { corner: 'top-left', hfr: 3.0 },
          { corner: 'top-right', hfr: 2.0 },
          { corner: 'bottom-left', hfr: 2.0 },
          { corner: 'bottom-right', hfr: 2.0 },
        ],
        mean_hfr: 2.0,
        tilt_percent: 50,
        curvature_percent: 12.5,
        worst_corner: 'top-left',
        best_corner: 'top-right',
      },
    },
    error: null,
  };
}

describe('TiltInspector', () => {
  it('renders the server-computed verdict and the ASTAP tilt figure', async () => {
    server.use(
      http.get('/api/db/demo/images/7/stars', () =>
        HttpResponse.json(detectionResponse())
      )
    );
    render(<TiltInspector open dbId="demo" imageId={7} onClose={() => {}} />, {
      wrapper: wrapper(),
    });
    // The verdict is the server's, verbatim — no browser-side math left to
    // disagree with it.
    expect(
      await screen.findByText(/50% — softest top-left, sharpest top-right/)
    ).toBeInTheDocument();
    expect(screen.getByText(/corners \+13% vs center/)).toBeInTheDocument();
    // The ASTAP figure: corner HFDs as vertex distances.
    expect(
      screen.getByRole('img', { name: 'Corner HFD tilt figure' })
    ).toBeInTheDocument();
    expect(screen.getByText('3.00')).toBeInTheDocument();
  });

  it('says so when the star cache predates region support', async () => {
    server.use(
      http.get('/api/db/demo/images/8/stars', () =>
        HttpResponse.json({
          success: true,
          data: {
            detected_stars: 10,
            average_hfr: 2.0,
            average_fwhm: 4.0,
            width: 3000,
            height: 2000,
            stars: [],
          },
          error: null,
        })
      )
    );
    render(<TiltInspector open dbId="demo" imageId={8} onClose={() => {}} />, {
      wrapper: wrapper(),
    });
    expect(
      await screen.findByText(/predates region support/)
    ).toBeInTheDocument();
  });
});
