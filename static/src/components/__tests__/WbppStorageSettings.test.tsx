import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import WbppStorageSettings from '../WbppStorageSettings';
import type { PixInsightSettings } from '../../api/types';

function wrapper() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  };
}
const initial: PixInsightSettings = {
  binary: '/opt/PixInsight',
  runs_dir: '/data/wbpp',
  runs_dir_free_bytes: null,
  ready: false,
  display: { kind: 'missing' },
  detection: { install: null, source: null, checked: [], problem: null },
  storage: { max_volume_percent: 90, intermediate_days: 7, master_days: 90 },
  storage_report: {
    checked_at: 100,
    runs: 2,
    intermediate_bytes: 1024 ** 3,
    master_bytes: 2 * 1024 ** 3,
    deleted_files: 3,
    freed_bytes: 1024 ** 3,
    errors: [],
    volumes: [{ path: '/data/wbpp/db', used_percent: 95, max_percent: 80, over_limit: true }],
  },
};
const ok = (data: PixInsightSettings) => ({ success: true, data, error: null });

describe('WBPP storage settings', () => {
  it('saves a separate path, a limit below 50%, and forever retention for masters', async () => {
    let saved: unknown;
    server.use(
      http.get('/api/settings/pixinsight', () => HttpResponse.json(ok(initial))),
      http.put('/api/settings/pixinsight', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(ok({ ...initial, ...(saved as Partial<PixInsightSettings>) }));
      })
    );
    render(<WbppStorageSettings canManage />, { wrapper: wrapper() });
    fireEvent.change(await screen.findByLabelText('WBPP storage folder'), {
      target: { value: '/scratch/wbpp' },
    });
    const limit = screen.getByRole('slider', { name: /WBPP volume usage limit/ });
    expect(limit).toHaveAttribute('min', '0');
    fireEvent.change(limit, { target: { value: '20' } });
    fireEvent.change(screen.getByLabelText('WBPP intermediate retention, in days'), {
      target: { value: '14' },
    });
    fireEvent.click(screen.getByRole('checkbox', { name: /Keep WBPP masters forever/ }));
    expect(screen.getByLabelText('WBPP master retention, in days')).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Save WBPP storage' }));
    await waitFor(() =>
      expect(saved).toEqual({
        binary: '/opt/PixInsight',
        runs_dir: '/scratch/wbpp',
        storage: { max_volume_percent: 20, intermediate_days: 14, master_days: 0 },
      })
    );
  });
  it('requires a finite master lifetime at least as long as intermediate retention', async () => {
    server.use(http.get('/api/settings/pixinsight', () => HttpResponse.json(ok(initial))));
    render(<WbppStorageSettings canManage />, { wrapper: wrapper() });
    fireEvent.change(await screen.findByLabelText('WBPP master retention, in days'), {
      target: { value: '2' },
    });
    expect(screen.getByRole('button', { name: 'Save WBPP storage' })).toBeDisabled();
    expect(screen.getByRole('alert')).toHaveTextContent('Keep masters at least as long');
  });
  it('shows usage and protects settings from read-only changes', async () => {
    server.use(http.get('/api/settings/pixinsight', () => HttpResponse.json(ok(initial))));
    render(<WbppStorageSettings canManage={false} />, { wrapper: wrapper() });
    expect(await screen.findByLabelText('WBPP storage folder')).toBeDisabled();
    expect(screen.getByRole('slider', { name: /WBPP volume/ })).toBeDisabled();
    expect(screen.getByRole('checkbox', { name: /Keep WBPP masters forever/ })).toBeDisabled();
    expect(screen.getByText(/2 runs · intermediate files 1.0 GiB · masters 2.0 GiB/)).toBeInTheDocument();
    expect(screen.getByText(/95.0% used \(cleanup limit 80%\)/)).toBeInTheDocument();
    expect(screen.getByText(/volume is still above its WBPP limit/)).toBeInTheDocument();
  });
});
