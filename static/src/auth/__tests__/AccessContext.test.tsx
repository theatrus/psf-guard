import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { HttpResponse, http } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import AuthGate from '../AccessContext';
import { useAccess } from '../access';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

function ProtectedContent() {
  const access = useAccess();
  return (
    <div>
      Catalog ready · {access.canWrite ? 'editor' : 'viewer'} ·
      {access.canCompute ? ' compute' : ' cached only'} · {access.status.username}
      <button type="button" onClick={() => void access.logout()}>Sign out</button>
    </div>
  );
}

describe('AuthGate', () => {
  it('passes through when server authentication is disabled', async () => {
    render(
      <AuthGate><ProtectedContent /></AuthGate>,
      { wrapper: wrapper() },
    );

    expect(await screen.findByText(/Catalog ready · editor/)).toBeInTheDocument();
  });

  it('shows a normal login form and exposes the signed-in role', async () => {
    server.use(
      http.get('/api/auth/status', () =>
        HttpResponse.json({
          success: true,
          data: {
            authentication_required: true,
            authenticated: false,
            can_compute: false,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.post('/api/auth/login', async ({ request }) => {
        const body = await request.json() as { username: string; password: string };
        if (body.username !== 'viewer' || body.password !== 'secret') {
          return HttpResponse.json(
            {
              success: false,
              data: null,
              error: 'The username or password is incorrect',
              status: 'ready',
            },
            { status: 401 },
          );
        }
        return HttpResponse.json({
          success: true,
          data: {
            authentication_required: true,
            authenticated: true,
            role: 'read_only',
            username: 'viewer',
            can_compute: false,
          },
          error: null,
          status: 'ready',
        });
      }),
      http.post('/api/auth/logout', () => new HttpResponse(null, { status: 204 })),
    );
    render(
      <AuthGate><ProtectedContent /></AuthGate>,
      { wrapper: wrapper() },
    );

    expect(await screen.findByRole('heading', { name: 'Sign in to PSF Guard' }))
      .toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'viewer' } });
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'wrong' } });
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('incorrect');

    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'secret' } });
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
    await waitFor(() => {
      expect(screen.getByText('Catalog ready · viewer · cached only · viewer')).toBeInTheDocument();
    });
    fireEvent.click(screen.getByRole('button', { name: 'Sign out' }));
    expect(await screen.findByRole('heading', { name: 'Sign in to PSF Guard' }))
      .toBeInTheDocument();
  });
});
