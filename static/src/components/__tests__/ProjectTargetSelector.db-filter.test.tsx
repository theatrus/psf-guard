import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import ProjectTargetSelector from '../ProjectTargetSelector';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function project(id: number, name: string) {
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
  };
}

function serveTwoCatalogs() {
  server.use(
    http.get('/api/databases', () =>
      ok([
        { id: 'attic', name: 'Attic catalog', path: '/attic.sqlite' },
        { id: 'shed', name: 'Shed catalog', path: '/shed.sqlite' },
      ])
    ),
    http.get('/api/db/attic/projects', () => ok([project(1, 'Sh2 86')])),
    http.get('/api/db/shed/projects', () => ok([project(1, 'NGC 6820')])),
    http.get('/api/db/:dbId/targets', () => ok([]))
  );
}

function renderSelector() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={['/grid']}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  }
  return render(<ProjectTargetSelector />, { wrapper: Wrapper });
}

describe('the project picker database filter', () => {
  it('narrows the tree to one catalog without changing the scope', async () => {
    serveTwoCatalogs();
    renderSelector();

    const trigger = document.querySelector<HTMLButtonElement>('#scope-select')!;
    await waitFor(() => expect(trigger).not.toBeDisabled());
    fireEvent.click(trigger);

    await screen.findByText('Sh2 86');
    await screen.findByText('NGC 6820');
    // One "All projects · catalog" row per catalog while unfiltered.
    expect(screen.getAllByText('All projects')).toHaveLength(2);

    fireEvent.change(screen.getByLabelText('Filter the list by database'), {
      target: { value: 'shed' },
    });

    await waitFor(() => expect(screen.queryByText('Sh2 86')).toBeNull());
    expect(screen.getByText('NGC 6820')).toBeInTheDocument();
    expect(screen.getAllByText('All projects')).toHaveLength(1);
    // Filtering alone never navigates: the trigger still awaits a choice.
    expect(screen.getByText('Choose project or target')).toBeInTheDocument();
  });
});
