import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import WorkerShareSettings from '../WorkerShareSettings';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function mockServer() {
  const saved: unknown[] = [];
  let current = {
    interactive_ratio: 0.5,
    background_ratio: 0.25,
    default_interactive_ratio: 0.5,
    default_background_ratio: 0.25,
    logical_cores: 16,
  };
  server.use(
    http.get('/api/settings/workers', () => ok(current)),
    http.put('/api/settings/workers', async ({ request }) => {
      const body = (await request.json()) as { interactive_ratio: number | null; background_ratio: number | null };
      saved.push(body);
      current = {
        ...current,
        interactive_ratio: body.interactive_ratio ?? current.default_interactive_ratio,
        background_ratio: body.background_ratio ?? current.default_background_ratio,
      };
      return ok(current);
    })
  );
  return saved;
}

function renderSettings() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  return render(<WorkerShareSettings />, { wrapper: Wrapper });
}

describe('WorkerShareSettings', () => {
  it('shows each share in cores and saves a change, leaving the other at its default', async () => {
    const saved = mockServer();
    renderSettings();
    const background = await screen.findByRole('slider', { name: 'Background work share of cores' });
    expect(screen.getByText('25% · 4 of 16 cores')).toBeInTheDocument();
    fireEvent.change(background, { target: { value: '10' } });
    fireEvent.pointerUp(background);
    await waitFor(() => expect(saved).toEqual([{ interactive_ratio: null, background_ratio: 0.1 }]));
    expect(await screen.findByText('10% · 2 of 16 cores')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Use the default' }));
    await waitFor(() => expect(saved).toHaveLength(2));
    expect(saved[1]).toEqual({ interactive_ratio: null, background_ratio: null });
  });
});
