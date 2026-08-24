import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import ExportDefaultsSettings from '../ExportDefaultsSettings';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const current = (layout: 'standard' | 'wbpp') => ({
  success: true,
  data: { default_layout: layout },
  error: null,
});

describe('ExportDefaultsSettings', () => {
  it('shows the configured default', async () => {
    server.use(http.get('/api/settings/export', () => HttpResponse.json(current('wbpp'))));
    render(<ExportDefaultsSettings />, { wrapper: wrapper() });
    const select = await screen.findByLabelText('Default export layout');
    expect(select).toHaveValue('wbpp');
  });

  it('saves a change and reflects the server response', async () => {
    let saved: unknown = null;
    server.use(
      http.get('/api/settings/export', () => HttpResponse.json(current('standard'))),
      http.put('/api/settings/export', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(current('wbpp'));
      })
    );
    render(<ExportDefaultsSettings />, { wrapper: wrapper() });
    const select = await screen.findByLabelText('Default export layout');
    fireEvent.change(select, { target: { value: 'wbpp' } });
    await waitFor(() => expect(saved).toEqual({ default_layout: 'wbpp' }));
    await waitFor(() => expect(select).toHaveValue('wbpp'));
  });
});
