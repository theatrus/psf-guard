import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import type { StackColorJob, StackContinuumResult } from '../../api/types';
import StackContinuumResults from '../StackContinuumResults';
function mount(line: StackContinuumResult) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={client}><StackContinuumResults dbId="test" job={{ job_id: 'fit', continuum: [line] } as StackColorJob} /></QueryClientProvider>);
}
it('loads stellar diagnostics on demand and preserves signed artifact links', async () => {
  const fetched = vi.fn();
  server.use(http.get('/api/db/test/stack-previews/color/fit/continuum/ha/report', () => {
    fetched(); return HttpResponse.json({ status: 'fitted', plan: { diagnostics: { stars: [
      { flux: 100, used: true, validation: false, fractionalResidual: 0.02 },
      { flux: 500, used: false, validation: true, fractionalResidual: -0.03 },
    ] } } });
  }));
  mount({ role: 'ha', references: ['red'], status: 'manual', detail: 'Explicit weights', clipping_maps: false,
    plan: { coefficients: [0.2], referenceFluxRatios: [1], origin: 'manual' }, report_url: '/report', residual_fits_url: '/signed.fits' });
  expect(fetched).not.toHaveBeenCalled();
  await userEvent.click(screen.getByText(/Hα · Manual/));
  expect(screen.getByRole('link', { name: 'Signed residual FITS' })).toHaveAttribute('href', '/signed.fits');
  await userEvent.click(screen.getByRole('button', { name: 'Inspect stellar residuals' }));
  const chart = await screen.findByRole('img', { name: /fractional stellar residual versus log flux/ });
  expect(chart.querySelectorAll('circle')).toHaveLength(2);
  expect(fetched).toHaveBeenCalledOnce();
  expect(within(chart).getByText(/Held out · flux/)).toBeInTheDocument();
});
it('shows Off without suggesting unavailable fit evidence', async () => {
  mount({ role: 'ha', references: ['red'], status: 'disabled', detail: 'Continuum subtraction off', clipping_maps: false });
  await userEvent.click(screen.getByText(/Hα · Off/));
  expect(screen.queryByText('Review saturation evidence in the report')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Inspect stellar residuals' })).not.toBeInTheDocument();
});
