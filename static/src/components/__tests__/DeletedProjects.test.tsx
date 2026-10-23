import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import DeletedProjects from '../DeletedProjects';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  render(<DeletedProjects catalogs={[{ id: 'c925', name: 'C925' }, { id: 'redcat', name: 'RedCat' }]} />, { wrapper });
}

describe('Deleted projects in the Library', () => {
  it('lists every catalog\'s deletions and allows one back', async () => {
    const allowed: string[] = [];
    server.use(
      http.get('/api/db/c925/deleted-projects', () => ok(allowed.length ? [] : [
        { id: 4, project_id: 9, name: 'M31', removed_at: 1_790_000_000, frames: 1234 },
      ])),
      http.get('/api/db/redcat/deleted-projects', () => ok([
        { id: 1, project_id: 2, name: 'Pelican', removed_at: 1_790_000_000, frames: 1 },
      ])),
      http.post('/api/db/:dbId/deleted-projects/:id/allow', ({ params }) => {
        allowed.push(`${params.dbId}:${params.id}`);
        return ok({ allowed: true });
      }),
    );
    mount();
    const toggle = await screen.findByRole('button', { name: /Deleted projects/ });
    await waitFor(() => expect(toggle).toHaveTextContent('2'));
    fireEvent.click(toggle);
    expect(screen.getByText(/1,234 frames/)).toHaveTextContent('C925');
    expect(screen.getByText(/RedCat · deleted .* · 1 frame$/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Allow M31 back on C925' }));
    const notice = await screen.findByRole('status');
    expect(notice).toHaveTextContent('M31 can come back to C925: the next sync, or an import of its files, adds it as a new project.');
    await waitFor(() => expect(notice).toHaveFocus());
    expect(allowed).toEqual(['c925:4']);
    await waitFor(() => expect(screen.queryByText('M31')).not.toBeInTheDocument());
    expect(screen.getByText('Pelican')).toBeInTheDocument();
  });

  it('shows nothing while no project is deleted', async () => {
    server.use(http.get('/api/db/:dbId/deleted-projects', () => ok([])));
    mount();
    await new Promise(resolve => setTimeout(resolve, 50));
    expect(screen.queryByRole('region', { name: 'Deleted projects' })).not.toBeInTheDocument();
  });
});
