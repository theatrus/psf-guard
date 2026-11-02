import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../../test/msw-server';
import RigsPage from '../../rigs/RigsPage';
import { plans, rig } from './fixtures';

const ok = (data: unknown) => ({ success: true, data, error: null });
const enabled = { protocol_version: 1, enabled: true, instance_id: '11111111-1111-4111-8111-111111111111', database_management: true };

function Location() {
  const location = useLocation();
  return <output data-testid="location">{location.pathname}{location.search}</output>;
}

function mount(body = plans(), route = '/rigs?rigsTab=nights') {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const planned = vi.fn(() => HttpResponse.json(ok(body)));
  const writes: Array<{ url: string; body: { expected_revision: number; marks: Array<{ state: string; note: string; windows: Array<{ start_ms: number }> }> } }> = [];
  const saved = vi.fn(async ({ request }: { request: Request }) => {
    writes.push({ url: request.url, body: await request.json() });
    return HttpResponse.json(ok({ scope: 'site', scope_id: 'x', revision: 1, marks: [] }));
  });
  server.use(
    http.get('/api/director/v1/status', () => HttpResponse.json(ok(enabled))),
    http.get('/api/director/v1/plans', () => HttpResponse.json(ok({ rows: [] }))),
    http.post('/api/director/v1/night-plans', planned),
    http.put('/api/director/v1/night-availability/:scope/:id', saved),
    http.get('/api/director/v1/rigs/status', () => HttpResponse.json(ok([]))),
  );
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>{children}<Location /></MemoryRouter></QueryClientProvider>;
  return { ...render(<RigsPage />, { wrapper: Wrapper }), planned, saved, writes };
}

describe('Rigs', () => {
  it('opens on Live and switches to Nights, keeping the tab in the address', async () => {
    mount(plans(), '/rigs');
    expect(await screen.findByRole('tab', { name: 'Live' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('link', { name: /Show the rigs on the sky/ })).toHaveAttribute('href', '/sky?live=1');
    fireEvent.click(screen.getByRole('tab', { name: 'Nights' }));
    expect(await screen.findByTestId('nights-grid')).toBeInTheDocument();
    expect(screen.getByTestId('location')).toHaveTextContent('/rigs?rigsTab=nights');
  });
});

describe('Nights', () => {
  it('shows each rig across its nights and the chosen night in detail', async () => {
    const { planned } = mount();
    const grid = await screen.findByTestId('nights-grid');
    expect(planned).toHaveBeenCalled();
    expect(within(grid).getByText('RedCat 61')).toBeInTheDocument();
    expect(within(grid).getByText('Target Scheduler · Backyard')).toBeInTheDocument();
    const cells = within(grid).getAllByRole('button');
    expect(cells).toHaveLength(3);
    expect(cells[1]).toHaveAccessibleName(/none clear of 10\.0 h dark, not available, Moon 42%/);
    expect(within(cells[0]).getByText('5.0 h idle')).toBeInTheDocument();
    fireEvent.click(cells[1]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).getByText(/Rig: Not available/)).toBeInTheDocument();
    expect(within(detail).getByText('· dome motor')).toBeInTheDocument();
    expect(within(detail).getByText('IC 1805')).toBeInTheDocument();
    expect(cells[1]).toHaveAttribute('aria-pressed', 'true');
  });

  it('marks a run of nights limited for every rig at the site', async () => {
    const { saved, writes } = mount();
    const grid = await screen.findByTestId('nights-grid');
    const cells = within(grid).getAllByRole('button');
    fireEvent.click(cells[0]);
    fireEvent.click(cells[2], { shiftKey: true });
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).getByText('Mark these 3 nights')).toBeInTheDocument();
    fireEvent.click(within(detail).getByLabelText('Limited'));
    fireEvent.change(within(detail).getByLabelText('From'), { target: { value: '22:30' } });
    fireEvent.change(within(detail).getByLabelText('For'), { target: { value: 'site' } });
    fireEvent.change(within(detail).getByLabelText('Note'), { target: { value: 'high cirrus' } });
    fireEvent.click(within(detail).getByRole('button', { name: 'Save mark' }));
    await waitFor(() => expect(saved).toHaveBeenCalled());
    const [{ url, body }] = writes;
    expect(url).toContain(`/night-availability/site/${rig().planning_site!.id}`);
    expect(body.expected_revision).toBe(0);
    expect(body.marks).toHaveLength(3);
    for (const mark of body.marks) {
      expect(mark.state).toBe('limited');
      expect(mark.note).toBe('high cirrus');
      expect(mark.windows).toHaveLength(1);
      expect(new Date(mark.windows[0].start_ms).getHours()).toBe(22);
    }
  });

  it('lets a reader see the nights but not mark them', async () => {
    const { AccessContext } = await import('../../../auth/access');
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    server.use(
      http.get('/api/director/v1/status', () => HttpResponse.json(ok(enabled))),
      http.get('/api/director/v1/plans', () => HttpResponse.json(ok({ rows: [] }))),
      http.post('/api/director/v1/night-plans', () => HttpResponse.json(ok(plans()))),
    );
    render(<AccessContext.Provider value={{ status: { authentication_required: true, authenticated: true, role: 'read_only', can_compute: true }, canWrite: false, canCompute: true, logout: async () => undefined }}>
      <QueryClientProvider client={client}><MemoryRouter initialEntries={['/rigs?rigsTab=nights']}><RigsPage /></MemoryRouter></QueryClientProvider>
    </AccessContext.Provider>);
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[0]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).queryByRole('button', { name: 'Save mark' })).toBeNull();
  });
});
