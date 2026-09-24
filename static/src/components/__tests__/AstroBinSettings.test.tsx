import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import AstroBinSettings from '../AstroBinSettings';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const ok = (filter_ids: Record<string, number>) => ({
  success: true,
  data: { filter_ids },
  error: null,
});

describe('AstroBinSettings', () => {
  it('lists the map and adds and forgets entries through the whole-map PUT', async () => {
    const puts: unknown[] = [];
    server.use(
      http.get('/api/settings/astrobin', () => HttpResponse.json(ok({ L: 4049 }))),
      http.put('/api/settings/astrobin', async ({ request }) => {
        const body = (await request.json()) as { filter_ids: Record<string, number> };
        puts.push(body);
        return HttpResponse.json(ok(body.filter_ids));
      })
    );
    render(<AstroBinSettings />, { wrapper: wrapper() });
    expect(await screen.findByText('4049')).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('Filter name'), { target: { value: 'Ha' } });
    fireEvent.change(screen.getByLabelText('AstroBin filter id'), { target: { value: '4051' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add' }));
    await waitFor(() => expect(puts[0]).toEqual({ filter_ids: { L: 4049, Ha: 4051 } }));
    expect(await screen.findByText('4051')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Forget AstroBin id for L' }));
    await waitFor(() => expect(puts[1]).toEqual({ filter_ids: { Ha: 4051 } }));
    await waitFor(() => expect(screen.queryByText('4049')).not.toBeInTheDocument());
  });
});
