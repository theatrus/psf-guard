import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import StudioConnectPage from '../StudioConnectPage';

function wrapper(path: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[path]}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  };
}

const ok = <T,>(data: T) => ({ success: true, data, error: null });

describe('StudioConnectPage', () => {
  it('shows the waiting code and allows it', async () => {
    let decided: unknown = null;
    server.use(
      http.get('/api/auth/studio/approve', ({ request }) => {
        expect(new URL(request.url).searchParams.get('code')).toBe('KQ7M-4PXZ');
        return HttpResponse.json(
          ok({
            user_code: 'KQ7M-4PXZ',
            client_name: 'Observatory Mac',
            expires_at: 1_758_000_600,
            saves_documents: true,
          })
        );
      }),
      http.post('/api/auth/studio/approve', async ({ request }) => {
        decided = await request.json();
        return HttpResponse.json(ok({ client_name: 'Observatory Mac', allowed: true }));
      })
    );
    render(<StudioConnectPage />, { wrapper: wrapper('/studio-connect?code=KQ7M-4PXZ') });
    expect(await screen.findByLabelText('Code')).toHaveTextContent('KQ7M-4PXZ');
    expect(screen.getByText('Observatory Mac')).toBeInTheDocument();
    expect(screen.getByText(/save Studio documents/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }));
    await waitFor(() => expect(decided).toEqual({ code: 'KQ7M-4PXZ', allow: true }));
    expect(await screen.findByText('PSF Studio is connected')).toBeInTheDocument();
  });

  it('explains a code no PSF Studio is waiting with', async () => {
    server.use(
      http.get('/api/auth/studio/approve', () =>
        HttpResponse.json(
          { success: false, data: null, error: 'No PSF Studio is waiting with this code.' },
          { status: 404 }
        )
      )
    );
    render(<StudioConnectPage />, { wrapper: wrapper('/studio-connect?code=AAAA-AAAA') });
    expect(
      await screen.findByText('No PSF Studio is waiting with this code.')
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Allow' })).not.toBeInTheDocument();
  });

  it('says that a read-only account saves on the Studio computer only', async () => {
    server.use(
      http.get('/api/auth/studio/approve', () =>
        HttpResponse.json(
          ok({
            user_code: 'KQ7M-4PXZ',
            client_name: 'Laptop',
            expires_at: 1_758_000_600,
            saves_documents: false,
          })
        )
      )
    );
    render(<StudioConnectPage />, { wrapper: wrapper('/studio-connect?code=KQ7M-4PXZ') });
    expect(await screen.findByText(/saves documents on its own computer only/)).toBeInTheDocument();
  });
});
