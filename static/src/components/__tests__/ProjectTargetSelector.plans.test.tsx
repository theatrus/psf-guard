import { describe, expect, it, vi } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import ProjectTargetSelector from '../ProjectTargetSelector';

// Two plans no database takes yet, the first with its workspace open; a plan
// with frames on Askar and none on Ultracat; one with frames on Askar only;
// and one Closed on Ultracat before any frame.
const { rows } = vi.hoisted(() => {
  const link = (catalog_slug: string, catalog_name: string, source_row_id: number | null, source_state = 1) => ({ catalog_slug, catalog_name, source_row_id, source_state });
  return {
    rows: [
      { project: { id: 7, name: 'Veil mosaic' }, links: [] },
      { project: { id: 8, name: 'Pelican wide' }, links: [] },
      { project: { id: 9, name: 'Propeller Nebula' }, links: [link('askar', 'Askar', 6), link('ultracat', 'Ultracat', 20)] },
      { project: { id: 10, name: 'Sh2 86' }, links: [link('askar', 'Askar', 5)] },
      { project: { id: 11, name: 'Old plan' }, links: [link('ultracat', 'Ultracat', 21, 3)] },
    ],
  };
});

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
  it('lists plans activated on a rig with no frames yet, and plans on no rig', async () => {
    const project = (id: number, name: string) => ({ id, profile_id: 'p', profile_name: 'p', name, display_name: name, description: null, has_files: true, state: 1, latest_image_date: 1_705_352_400 });
    server.use(
      http.get('/api/databases', () => ok([{ id: 'askar', name: 'Askar', path: '/askar.sqlite' }, { id: 'ultracat', name: 'Ultracat', path: '/ultracat.sqlite' }])),
      http.get('/api/db/askar/projects', () => ok([project(5, 'Sh2 86'), project(6, 'Propeller Nebula')])),
      http.get('/api/db/ultracat/projects', () => ok([])),
    );
    renderAt('/');
    const trigger = document.querySelector<HTMLButtonElement>('#scope-select')!;
    await vi.waitFor(() => expect(trigger).not.toBeDisabled());
    await userEvent.click(trigger);
    const waiting = await screen.findByRole('region', { name: 'Plans waiting for frames' });
    expect(within(waiting).getByRole('button', { name: /Propeller Nebula/ })).toHaveTextContent('No frames on Ultracat');
    expect(within(waiting).getByRole('button', { name: /Veil mosaic/ })).toHaveTextContent('Not on any rig');
    expect(within(waiting).queryByRole('button', { name: /Sh2 86/ })).not.toBeInTheDocument();
    expect(within(waiting).queryByRole('button', { name: /Old plan/ })).not.toBeInTheDocument();
  });
});
