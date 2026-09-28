import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import ProjectTargetSelector from '../ProjectTargetSelector';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

const guid = '9A1D4C2E-0000-4000-8000-000000000001';

function project(id: number, name: string, extra: Record<string, unknown> = {}) {
  return {
    id,
    profile_id: 'profile',
    profile_name: 'Profile',
    name,
    display_name: name,
    description: null,
    has_files: true,
    state: 1,
    latest_image_date: 1_705_352_400,
    ...extra,
  };
}

function target(id: number, projectId: number, name: string) {
  return { id, project_id: projectId, name, active: true, has_files: true };
}

/** Both rigs shoot Heart under one Target Scheduler GUID; the shed also has its own project. */
function serveTwoRigs() {
  server.use(
    http.get('/api/databases', () =>
      ok([
        { id: 'attic', name: 'Attic catalog', path: '/attic.sqlite' },
        { id: 'shed', name: 'Shed catalog', path: '/shed.sqlite' },
      ])
    ),
    http.get('/api/db/attic/projects', () => ok([project(1, 'Heart', { guid })])),
    http.get('/api/db/shed/projects', () =>
      ok([project(3, 'Heart', { guid: guid.toLowerCase(), latest_image_date: 1_705_300_000 }), project(4, 'Pelican')])
    ),
    http.get('/api/db/attic/targets', () => ok([target(11, 1, 'Heart r1c1')])),
    http.get('/api/db/shed/targets', () => ok([target(31, 3, 'Heart r1c2'), target(41, 4, 'IC 5070')]))
  );
}

function Location() {
  const location = useLocation();
  return <output data-testid="location">{`${location.pathname}${location.search}`}</output>;
}

function renderAt(route: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[route]}>
          {children}
          <Location />
        </MemoryRouter>
      </QueryClientProvider>
    );
  }
  return render(<ProjectTargetSelector />, { wrapper: Wrapper });
}

async function openPicker() {
  const trigger = document.querySelector<HTMLButtonElement>('#scope-select')!;
  await waitFor(() => expect(trigger).not.toBeDisabled());
  fireEvent.click(trigger);
}

describe('a project shot by several rigs in the picker', () => {
  it('is one row that opens to each rig, and choosing a rig scopes the grid to it', async () => {
    serveTwoRigs();
    renderAt('/grid');
    await openPicker();

    const heart = await screen.findByRole('button', { name: /Heart.*2 rigs/ });
    expect(heart).toHaveTextContent('Attic catalog · Shed catalog');
    // Neither rig's copy shows again as a plain row.
    expect(screen.getAllByRole('button', { name: /^Heart/ })).toHaveLength(1);
    expect(screen.getByRole('button', { name: /Pelican/ })).toBeInTheDocument();

    fireEvent.click(heart);
    const shed = await screen.findByRole('region', { name: 'Heart on Shed catalog' });
    expect(within(screen.getByRole('region', { name: 'Heart on Attic catalog' })).getByText('Heart r1c1')).toBeInTheDocument();
    expect(within(shed).getByText('Heart r1c2')).toBeInTheDocument();

    fireEvent.click(within(shed).getByRole('button', { name: /All images/ }));
    expect(screen.getByTestId('location')).toHaveTextContent('/grid?db=shed&project=3');
  });

  it('is found by typing a target that only the other rig has, already open to it', async () => {
    serveTwoRigs();
    renderAt('/grid');
    await openPicker();
    await screen.findByRole('button', { name: /Heart.*2 rigs/ });

    fireEvent.change(screen.getByLabelText('Search projects or targets'), { target: { value: 'r1c2' } });
    const shed = await screen.findByRole('region', { name: 'Heart on Shed catalog' });
    expect(within(shed).getByText('Heart r1c2')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Pelican/ })).not.toBeInTheDocument();

    fireEvent.click(within(shed).getByRole('button', { name: /Heart r1c2/ }));
    expect(screen.getByTestId('location')).toHaveTextContent('/grid?db=shed&project=3&target=31');
  });

  it('reopens on the rig that is in view', async () => {
    serveTwoRigs();
    renderAt('/grid?db=shed&project=3&target=31');
    await openPicker();

    const shed = await screen.findByRole('region', { name: 'Heart on Shed catalog' });
    expect(within(shed).getByRole('button', { name: /Heart r1c2/ })).toHaveAttribute('aria-current', 'true');
  });
});
