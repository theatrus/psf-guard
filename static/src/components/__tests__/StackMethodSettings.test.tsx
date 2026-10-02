import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackMethod } from '../../api/types';
import StackMethodSettings from '../StackMethodSettings';

const recommended: StackMethod = {
  normalization: 'local_background',
  weighting: 'noise',
  registration: 'quadratic',
  interpolation: 'lanczos3',
  bayer_drizzle: false,
  reference: 'auto',
  final_pass: 'reintegrate',
};

const classic: StackMethod = {
  normalization: 'global',
  weighting: 'equal',
  registration: 'similarity',
  interpolation: 'bilinear',
  bayer_drizzle: false,
  reference: 'best_graded',
  final_pass: 'reintegrate',
};

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function mockServer(initial: StackMethod) {
  let stored = initial;
  const saved: StackMethod[] = [];
  server.use(
    http.get('/api/settings/stacking/method', () => ok({ method: stored, recommended, classic })),
    http.put('/api/settings/stacking/method', async ({ request }) => {
      stored = (await request.json()) as StackMethod;
      saved.push(stored);
      return ok({ method: stored, recommended, classic });
    })
  );
  return saved;
}

function renderSettings() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  return render(<StackMethodSettings />, { wrapper: Wrapper });
}

describe('StackMethodSettings', () => {
  it("starts on Seiza's recommendation and saves one change as a whole method", async () => {
    const saved = mockServer(recommended);
    renderSettings();
    expect(await screen.findByText('Now: Recommended')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Recommended' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('radio', { name: /^Local background/ })).toBeChecked();
    expect(screen.getByRole('radio', { name: /^Reintegrate/ })).toBeChecked();

    await userEvent.click(screen.getByRole('radio', { name: /^Bilinear/ }));
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(saved[0]).toEqual({ ...recommended, interpolation: 'bilinear' });
    expect(await screen.findByText('Now: Custom')).toBeInTheDocument();
  });

  it('offers a draft without the final pass, and the classic method', async () => {
    const saved = mockServer(recommended);
    renderSettings();
    await userEvent.click(await screen.findByRole('button', { name: 'Draft' }));
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(saved[0]).toEqual({ ...recommended, final_pass: 'draft' });
    expect(await screen.findByText('Now: Draft')).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: /^Draft/ })).toBeChecked();

    await userEvent.click(screen.getByRole('button', { name: 'Classic' }));
    await waitFor(() => expect(saved).toHaveLength(2));
    expect(saved[1]).toEqual(classic);
  });

  it('turns Bayer drizzle on', async () => {
    const saved = mockServer(recommended);
    renderSettings();
    await userEvent.click(await screen.findByRole('checkbox', { name: /Bayer drizzle/ }));
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(saved[0].bayer_drizzle).toBe(true);
  });
});
