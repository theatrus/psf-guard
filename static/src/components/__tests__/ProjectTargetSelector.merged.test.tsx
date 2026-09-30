import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import ProjectTargetSelector from '../ProjectTargetSelector';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null, status: 'ready' });
const project = { id: 1, profile_id: 'profile', profile_name: 'Profile', name: 'Sh2 86', display_name: 'Sh2 86', description: null, has_files: true, state: 1, latest_image_date: 1_705_352_400 };

function Location() {
  const location = useLocation();
  return <output data-testid="location">{location.pathname}{location.search}</output>;
}

function renderAt(route: string) {
  server.use(
    http.get('/api/databases', () => ok([{ id: 'attic', name: 'Attic catalog', path: '/attic.sqlite' }])),
    http.get('/api/db/attic/projects', () => ok([project])),
    http.get('/api/db/attic/targets', () => ok([{ id: 4, project_id: 1, name: 'Sh2 86', active: true, has_files: true }])),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>{children}<Location /></MemoryRouter></QueryClientProvider>;
  return render(<ProjectTargetSelector />, { wrapper: Wrapper });
}

describe('ProjectTargetSelector on a merged view', () => {
  it('opens Images for a project chosen from the Library, keeping the rest of the URL', async () => {
    renderAt('/plan?dbfilter=attic&show=active&plan=abc&directorView=projects');
    const trigger = document.querySelector<HTMLButtonElement>('#scope-select')!;
    await waitFor(() => expect(trigger).not.toBeDisabled());
    fireEvent.click(trigger);
    fireEvent.click(await screen.findByRole('button', { name: /^Sh2 86/, expanded: false }));
    fireEvent.click(await screen.findByRole('button', { name: /All images/ }));
    // Planning's own params stay behind; the rest of the URL comes along.
    expect(screen.getByTestId('location')).toHaveTextContent('/grid?dbfilter=attic&show=active&db=attic&project=1');
    expect(screen.getByTestId('location')).not.toHaveTextContent('plan=');
  });

  it('refreshes caches only for the database a scoped view shows', async () => {
    renderAt('/?db=attic&project=1');
    const refresh = await screen.findByRole('button', { name: '↻' });
    expect(refresh).toBeDisabled();
  });

  it('only moves the scope when already on a scoped view', async () => {
    renderAt('/sequence?db=attic');
    const trigger = document.querySelector<HTMLButtonElement>('#scope-select')!;
    await waitFor(() => expect(trigger).not.toBeDisabled());
    fireEvent.click(trigger);
    fireEvent.click(await screen.findByRole('button', { name: /^Sh2 86/, expanded: false }));
    fireEvent.click(await screen.findByRole('button', { name: /All images/ }));
    expect(screen.getByTestId('location')).toHaveTextContent('/sequence?db=attic&project=1');
  });
});
