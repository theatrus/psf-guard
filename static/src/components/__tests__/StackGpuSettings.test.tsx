import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackGpu } from '../../api/types';
import StackGpuSettings from '../StackGpuSettings';

const recommended: StackGpu = {
  enabled: true,
  debayer: true,
  detection: true,
  resampling: true,
  accumulator: true,
  integration: true,
  concurrency: 3,
  trace: false,
};

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function mockServer(initial: StackGpu) {
  let stored = initial;
  const saved: StackGpu[] = [];
  server.use(
    http.get('/api/settings/stacking/gpu', () => ok({ gpu: stored, recommended, max_concurrency: 16 })),
    http.put('/api/settings/stacking/gpu', async ({ request }) => {
      stored = (await request.json()) as StackGpu;
      saved.push(stored);
      return ok({ gpu: stored, recommended, max_concurrency: 16 });
    })
  );
  return saved;
}

function renderSettings() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  return render(<StackGpuSettings />, { wrapper: Wrapper });
}

describe('StackGpuSettings', () => {
  it('starts with every pass on the GPU and saves one pass turned off', async () => {
    const saved = mockServer(recommended);
    renderSettings();
    expect(await screen.findByRole('checkbox', { name: /^Use the GPU/ })).toBeChecked();
    for (const name of ['Debayering', 'Star detection', 'Resampling', 'Live stack', 'Final pass']) {
      expect(screen.getByRole('checkbox', { name: new RegExp(`^${name}`) })).toBeChecked();
    }
    expect(screen.getByRole('button', { name: 'Restore defaults' })).toBeDisabled();

    await userEvent.click(screen.getByRole('checkbox', { name: /^Live stack/ }));
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(saved[0]).toEqual({ ...recommended, accumulator: false });
    expect(await screen.findByRole('button', { name: 'Restore defaults' })).toBeEnabled();
  });

  it('turning the GPU off keeps every pass on the CPU and disables the passes', async () => {
    const saved = mockServer(recommended);
    renderSettings();
    await userEvent.click(await screen.findByRole('checkbox', { name: /^Use the GPU/ }));
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(saved[0]).toEqual({ ...recommended, enabled: false });
    await waitFor(() => expect(screen.getByRole('checkbox', { name: /^Resampling/ })).toBeDisabled());
    expect(screen.getByRole('spinbutton', { name: /Frames, 1 to 16/ })).toBeDisabled();
  });

  it('restores the defaults in one save', async () => {
    const saved = mockServer({ ...recommended, enabled: false, debayer: false, concurrency: 1, trace: true });
    renderSettings();
    await userEvent.click(await screen.findByRole('button', { name: 'Restore defaults' }));
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(saved[0]).toEqual(recommended);
  });

  it('saves the frames at once on blur and refuses a count past the limit', async () => {
    const saved = mockServer(recommended);
    renderSettings();
    const field = await screen.findByRole('spinbutton', { name: /Frames, 1 to 16/ });
    fireEvent.change(field, { target: { value: '2' } });
    fireEvent.blur(field);
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(saved[0]).toEqual({ ...recommended, concurrency: 2 });

    fireEvent.change(field, { target: { value: '40' } });
    fireEvent.keyDown(field, { key: 'Enter' });
    await waitFor(() => expect(field).toHaveValue(2));
    expect(saved).toHaveLength(1);
  });

  it('says so when the settings cannot be loaded', async () => {
    server.use(http.get('/api/settings/stacking/gpu', () => HttpResponse.json({ success: false, error: 'boom' }, { status: 500 })));
    renderSettings();
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not load the GPU settings.');
  });
});
