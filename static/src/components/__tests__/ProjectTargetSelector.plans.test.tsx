import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import ProjectTargetSelector from '../ProjectTargetSelector';

// Two plans no database takes yet; the workspace of the first is open.
const { rows } = vi.hoisted(() => ({
  rows: [
    { project: { id: 7, name: 'Veil mosaic' }, links: [] },
    { project: { id: 8, name: 'Pelican wide' }, links: [] },
  ],
}));

vi.mock('../header/useCurrentPlan', () => {
  const plans = {
    enabled: true,
    instanceId: 'test',
    rows,
    planFor: () => null,
    hrefFor: () => '/plan?plan=veil',
    loading: false,
    error: false,
  };
  return {
    usePlans: () => plans,
    useCurrentPlan: () => ({ ...plans, current: rows[0] }),
  };
});

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function renderAt(route: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[route]}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  }
  return render(<ProjectTargetSelector />, { wrapper: Wrapper });
}

describe('plans in the project picker', () => {
  it('marks the plan whose workspace is open as current', async () => {
    server.use(http.get('/api/databases', () => ok([])));
    renderAt('/plan?plan=veil');
    await userEvent.click(document.querySelector('#scope-select')!);
    expect(await screen.findByRole('button', { name: /Veil mosaic/ })).toHaveAttribute('aria-current', 'true');
    expect(screen.getByRole('button', { name: /Pelican wide/ })).not.toHaveAttribute('aria-current');
  });
});
