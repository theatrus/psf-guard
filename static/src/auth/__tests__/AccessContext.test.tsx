import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { HttpResponse, http } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import AuthGate from '../AccessContext';
import { useAccess } from '../access';
import { SCORING_DEFAULTS, stackScoringSettings } from '../../utils/scoring';

const CALIBRATED = stackScoringSettings(SCORING_DEFAULTS);

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

  it('waits for the server\'s scoring defaults before showing the app', async () => {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    server.use(
      http.get('/api/settings/scoring', async () => {
        await gate;
        return HttpResponse.json({
          success: true,
          data: { scoring: CALIBRATED, builtin: CALIBRATED, stored: true },
          error: null,
        });
      }),
    );
    render(
      <AuthGate><ProtectedContent /></AuthGate>,
      { wrapper: wrapper() },
    );

    await waitFor(() => expect(screen.getByText('Loading scoring defaults…')).toBeInTheDocument());
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.getByText('Loading scoring defaults…')).toBeInTheDocument();
    expect(screen.queryByText(/Catalog ready/)).toBeNull();
    release();
    expect(await screen.findByText(/Catalog ready · editor/)).toBeInTheDocument();
  });

  it('shows the app when the scoring defaults cannot be read', async () => {
    server.use(
      http.get('/api/settings/scoring', () =>
        HttpResponse.json({ success: false, data: null, error: 'Internal error' }, { status: 500 })),
    );
    render(
      <AuthGate><ProtectedContent /></AuthGate>,
      { wrapper: wrapper() },
    );

    expect(await screen.findByText(/Catalog ready · editor/)).toBeInTheDocument();
  });

  it('shows the app at once when the scoring defaults cannot be read, without retrying', async () => {
    let reads = 0;
    server.use(
      http.get('/api/settings/scoring', () => {
        reads += 1;
        return HttpResponse.json({ success: false, data: null, error: 'Internal error' }, { status: 500 });
      }),
    );
    // The app's own query defaults (router/index.tsx), which retry a failed
    // query three times, seconds apart.
    const queryClient = new QueryClient({
      defaultOptions: { queries: { staleTime: 30000, refetchInterval: 30000 } },
    });
    render(
      <QueryClientProvider client={queryClient}>
        <AuthGate><ProtectedContent /></AuthGate>
      </QueryClientProvider>,
    );

    expect(await screen.findByText(/Catalog ready · editor/)).toBeInTheDocument();
    expect(reads).toBe(1);
    queryClient.clear();
  });

  it('shows a normal login form and exposes the signed-in role', async () => {
    let authenticated = false;
    server.use(
      http.get('/api/auth/status', () =>
        HttpResponse.json({
          success: true,
          data: {
            authentication_required: true,
            authenticated,
            ...(authenticated ? { role: 'read_only', username: 'viewer' } : {}),
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
        authenticated = true;
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
      http.post('/api/auth/logout', () => {
        authenticated = false;
        return new HttpResponse(null, { status: 204 });
      }),
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

  it('explains when the browser does not retain a successful login session', async () => {
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
      http.post('/api/auth/login', () =>
        HttpResponse.json({
          success: true,
          data: {
            authentication_required: true,
            authenticated: true,
            role: 'read_write',
            username: 'editor',
            can_compute: true,
          },
          error: null,
          status: 'ready',
        })
      ),
    );
    render(
      <AuthGate><ProtectedContent /></AuthGate>,
      { wrapper: wrapper() },
    );

    expect(await screen.findByRole('heading', { name: 'Sign in to PSF Guard' }))
      .toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'editor' } });
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'secret' } });
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'this browser could not save your session'
    );
    expect(screen.getByRole('heading', { name: 'Sign in to PSF Guard' })).toBeInTheDocument();
  });
});
