import { describe, expect, it } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import WorkerShareSettings from '../WorkerShareSettings';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function mockServer() {
  const saved: unknown[] = [];
  let current = {
    interactive_ratio: 0.5,
    background_ratio: 0.25,
    default_interactive_ratio: 0.5,
    default_background_ratio: 0.25,
    logical_cores: 16,
    memory_budget_fraction: 0.5,
    default_memory_budget_fraction: 0.5,
    available_memory_bytes: 48*1024**3,
    memory_basis: 'installed',
  };
  server.use(
    http.get('/api/settings/workers', () => ok(current)),
    http.put('/api/settings/workers', async ({ request }) => {
      const body = (await request.json()) as { interactive_ratio: number | null; background_ratio: number | null; memory_budget_fraction?: number | null };
      saved.push(body);
      current = {
        ...current,
        interactive_ratio: body.interactive_ratio ?? current.default_interactive_ratio,
        background_ratio: body.background_ratio ?? current.default_background_ratio,
        memory_budget_fraction: body.memory_budget_fraction === undefined ? current.memory_budget_fraction : body.memory_budget_fraction ?? current.default_memory_budget_fraction,
      };
      return ok(current);
    })
  );
  return saved;
}

function renderSettings(
  queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
) {
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  return render(<WorkerShareSettings />, { wrapper: Wrapper });
}

describe('WorkerShareSettings', () => {
  it('shows each share in cores and saves a change, leaving the other at its default', async () => {
    const saved = mockServer();
    renderSettings();
    const background = await screen.findByRole('slider', { name: 'Background work share of cores' });
    expect(screen.getByText('25% · 4 of 16 cores')).toBeInTheDocument();
    expect(screen.getByRole('slider', { name: 'Interactive work share of cores' })).toHaveValue('50');
    fireEvent.change(background, { target: { value: '10' } });
    fireEvent.pointerUp(background);
    await waitFor(() => expect(saved).toEqual([{ interactive_ratio: null, background_ratio: 0.1 }]));
    expect(await screen.findByText('10% · 2 of 16 cores')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Use the default' }));
    await waitFor(() => expect(saved).toHaveLength(2));
    expect(saved[1]).toEqual({ interactive_ratio: null, background_ratio: null });
  });

  it('a refetch updates an unmoved slider and keeps a moved one', async () => {
    mockServer();
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    renderSettings(queryClient);
    const background = await screen.findByRole('slider', { name: 'Background work share of cores' });
    fireEvent.change(background, { target: { value: '10' } });
    server.use(
      http.get('/api/settings/workers', () =>
        ok({
          interactive_ratio: 0.75,
          background_ratio: 0.5,
          default_interactive_ratio: 0.5,
          default_background_ratio: 0.25,
          logical_cores: 16,
        })
      )
    );
    await act(() => queryClient.refetchQueries({ queryKey: ['worker-settings'] }));
    await waitFor(() =>
      expect(screen.getByRole('slider', { name: 'Interactive work share of cores' })).toHaveValue('75')
    );
    expect(background).toHaveValue('10');
  });
});


it('saves a memory budget independently, shows GiB, and restores the server default', async () => {
  const saved=mockServer(); renderSettings();
  const memory=await screen.findByRole('slider',{name:'Processing memory budget'});
  expect(screen.getByText('50% · 24.0 of 48.0 GiB installed memory')).toBeInTheDocument();
  fireEvent.change(memory,{target:{value:'75'}});fireEvent.pointerUp(memory);
  await waitFor(()=>expect(saved).toEqual([{interactive_ratio:null,background_ratio:null,memory_budget_fraction:0.75}]));
  expect(await screen.findByText('75% · 36.0 of 48.0 GiB installed memory')).toBeInTheDocument();
  await userEvent.click(screen.getByRole('button',{name:'Use the default memory budget'}));
  await waitFor(()=>expect(saved).toHaveLength(2));
  expect(saved[1]).toEqual({interactive_ratio:null,background_ratio:null,memory_budget_fraction:null});
  await waitFor(()=>expect(memory).toHaveValue('50'));
});


it('lets keyboard users adjust the draft before Enter saves it', async () => {
  const saved=mockServer();renderSettings();
  const memory=await screen.findByRole('slider',{name:'Processing memory budget'});
  fireEvent.change(memory,{target:{value:'65'}});fireEvent.keyUp(memory,{key:'ArrowRight'});
  expect(saved).toHaveLength(0);
  fireEvent.keyUp(memory,{key:'Enter'});
  await waitFor(()=>expect(saved).toEqual([{interactive_ratio:null,background_ratio:null,memory_budget_fraction:0.65}]));
});
