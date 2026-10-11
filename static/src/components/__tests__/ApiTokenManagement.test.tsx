import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import ApiTokenManagement from '../ApiTokenManagement';
import { describeTokenExpiry } from '../../utils/apiTokens';
import type { AuthTokenSummary } from '../../api/types';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const ok = <T,>(data: T) => ({ success: true, data, error: null });

const existing: AuthTokenSummary = {
  id: 'abc123',
  username: 'editor',
  label: 'nightly script',
  role: 'read_write',
  read_only: false,
  created_at: 1_758_000_000,
};

describe('ApiTokenManagement', () => {
  it('mints a token, shows the secret once, and revokes it', async () => {
    let created: unknown = null;
    let tokens = [existing];
    server.use(
      http.get('/api/auth/tokens', () => HttpResponse.json(ok(tokens))),
      http.post('/api/auth/tokens', async ({ request }) => {
        created = await request.json();
        const summary: AuthTokenSummary = {
          id: 'new1',
          username: 'editor',
          label: 'claude',
          role: 'read_only',
          read_only: true,
          created_at: 1_758_100_000,
          expires_at: 1_758_100_000 + 30 * 86_400,
        };
        tokens = [summary, ...tokens];
        return HttpResponse.json(
          ok({ token: 'psfg_' + 'a'.repeat(64), summary, tokens })
        );
      }),
      http.delete('/api/auth/tokens/new1', () => {
        tokens = tokens.filter((token) => token.id !== 'new1');
        return HttpResponse.json(ok(tokens));
      })
    );
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    render(<ApiTokenManagement currentUsername="editor" isEditor />, { wrapper: wrapper() });
    expect(await screen.findByText('nightly script')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '+ New token' }));
    fireEvent.change(screen.getByLabelText('Label'), { target: { value: 'claude' } });
    fireEvent.change(screen.getByLabelText('Expires after (days, optional)'), {
      target: { value: '30' },
    });
    fireEvent.click(screen.getByLabelText("Read only, whatever the user's role"));
    fireEvent.click(screen.getByRole('button', { name: 'Create token' }));

    await waitFor(() => expect(created).not.toBeNull());
    expect(created).toEqual({ label: 'claude', read_only: true, expires_in_days: 30 });
    expect(await screen.findByText('psfg_' + 'a'.repeat(64))).toBeInTheDocument();
    expect(screen.getByText('Copy this token now.')).toBeInTheDocument();
    expect(screen.getByText('claude')).toBeInTheDocument();
    expect(screen.getAllByText('Read only').length).toBeGreaterThan(0);

    const row = screen.getByText('claude').closest('.token-row');
    expect(row).not.toBeNull();
    fireEvent.click(row!.querySelector('button.remove-button')!);
    await waitFor(() => expect(screen.queryByText('claude')).not.toBeInTheDocument());
    expect(screen.queryByText('Copy this token now.')).not.toBeInTheDocument();
  });

  it('connects an agent in one step: a read-only token and the setup for Claude Code and Codex', async () => {
    let created: Record<string, unknown> | null = null;
    const token = 'psfg_' + 'b'.repeat(64);
    server.use(
      http.get('/api/auth/tokens', () => HttpResponse.json(ok([]))),
      http.post('/api/auth/tokens', async ({ request }) => {
        created = await request.json() as Record<string, unknown>;
        const summary: AuthTokenSummary = {
          id: 'agent1', username: 'editor', label: String(created.label), role: 'read_only', read_only: true, created_at: 1_758_100_000,
        };
        return HttpResponse.json(ok({ token, summary, tokens: [summary] }));
      }),
    );
    render(<ApiTokenManagement currentUsername="editor" isEditor />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('button', { name: 'Connect an agent' }));
    expect(screen.getByLabelText('Let it grade and start jobs')).not.toBeChecked();
    fireEvent.click(screen.getByRole('button', { name: 'Create token' }));

    await waitFor(() => expect(created).not.toBeNull());
    expect(created).toMatchObject({ read_only: true });
    expect(String(created!.label)).toMatch(/^agent \d{4}-\d{2}-\d{2}$/);
    const url = `${window.location.origin}/api/mcp`;
    expect(await screen.findByText(`claude mcp add --transport http psf-guard ${url} --header "Authorization: Bearer ${token}"`)).toBeInTheDocument();
    const codex = screen.getByText((_, element) => element?.tagName === 'PRE' && (element.textContent ?? '').includes('[mcp_servers.psf-guard]'));
    expect(codex.textContent).toContain(`url = "${url}"`);
    expect(codex.textContent).toContain('bearer_token_env_var = "PSF_GUARD_TOKEN"');
    expect(codex.textContent).not.toContain(token);
  });

  it('hides the user field from a viewer', async () => {
    server.use(http.get('/api/auth/tokens', () => HttpResponse.json(ok([]))));
    render(<ApiTokenManagement currentUsername="viewer" isEditor={false} />, {
      wrapper: wrapper(),
    });
    expect(await screen.findByText('No tokens yet.')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '+ New token' }));
    expect(screen.queryByLabelText('User (optional)')).not.toBeInTheDocument();
  });

  it('describes expiry', () => {
    const now = 1_758_100_000 * 1000;
    expect(describeTokenExpiry(existing, now)).toBe('Never expires');
    expect(describeTokenExpiry({ ...existing, expires_at: 1 }, now)).toBe('Expired');
    expect(
      describeTokenExpiry({ ...existing, expires_at: 1_758_100_000 + 86_400 }, now)
    ).toMatch(/^Expires /);
  });
});
