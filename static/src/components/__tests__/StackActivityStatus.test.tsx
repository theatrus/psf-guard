import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import StackActivityStatus from '../StackActivityStatus';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

function activity(active: unknown[]) {
  return HttpResponse.json({
    success: true,
    data: { schema_version: 1, active },
    error: null,
    status: 'ready',
  });
}

describe('StackActivityStatus', () => {
  it('reports a running stack build from any view', async () => {
    server.use(
      http.get('/api/stack-activity', () => activity([
        {
          kind: 'mono',
          job_id: 'job-a',
          database_id: 'test',
          project_id: 1,
          state: 'running',
          label: 'Sh2 86 · Ha',
          detail: 'Registering frames',
          processed_units: 4,
          total_units: 10,
          created_unix_seconds: 100,
        },
      ])),
    );

    render(<StackActivityStatus />, { wrapper: wrapper() });

    expect(await screen.findByText('Stacking')).toBeInTheDocument();
    expect(screen.getByText('Sh2 86 · Ha · 4/10 frames')).toBeInTheDocument();
  });

  it('counts the other queued builds alongside the running one', async () => {
    server.use(
      http.get('/api/stack-activity', () => activity([
        {
          kind: 'color',
          job_id: 'job-color',
          database_id: 'test',
          project_id: 1,
          state: 'queued',
          label: 'M 31 · SHO',
          detail: 'Waiting for stacker',
          processed_units: 0,
          total_units: 0,
          created_unix_seconds: 90,
        },
        {
          kind: 'mono',
          job_id: 'job-a',
          database_id: 'test',
          project_id: 1,
          state: 'running',
          label: 'Sh2 86 · Ha',
          detail: 'Rendering preview',
          processed_units: 10,
          total_units: 10,
          created_unix_seconds: 100,
        },
      ])),
    );

    render(<StackActivityStatus />, { wrapper: wrapper() });

    expect(await screen.findByText('Stacking +1')).toBeInTheDocument();
    expect(screen.getByText('Sh2 86 · Ha · 10/10 frames')).toBeInTheDocument();
  });

  it('stays out of the header when nothing is stacking', async () => {
    server.use(http.get('/api/stack-activity', () => activity([])));
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false, gcTime: 0 } },
    });

    const { container } = render(
      <QueryClientProvider client={queryClient}>
        <StackActivityStatus />
      </QueryClientProvider>
    );

    await waitFor(() =>
      expect(queryClient.getQueryData(['stack-activity'])).toBeDefined()
    );
    expect(container.querySelector('.stack-activity-status')).toBeNull();
  });
});
