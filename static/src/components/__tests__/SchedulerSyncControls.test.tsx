import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { HttpResponse, http } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import SchedulerSyncControls from '../SchedulerSyncControls';

const local = {
  id: 'local',
  name: 'Review copy',
  db_path: '/tmp/local.sqlite',
  image_dirs: ['/images/local'],
};
const telescope = {
  id: 'scope',
  name: 'Telescope scheduler',
  db_path: '/tmp/scope.sqlite',
  image_dirs: ['/images/scope'],
};

function wrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}

const counts = { inserted: 0, updated: 0, unchanged: 0, skipped: 0 };

describe('SchedulerSyncControls', () => {
  it('previews before applying a planning-only push', async () => {
    const requests: Array<Record<string, unknown>> = [];
    server.use(
      http.post('/api/databases/local/sync', async ({ request }) => {
        const body = (await request.json()) as Record<string, unknown>;
        requests.push(body);
        return HttpResponse.json({
          success: true,
          data: {
            kind: body.kind,
            dry_run: body.dry_run,
            source_db_id: 'local',
            destination_db_id: 'scope',
            exposuretemplate: counts,
            project: { ...counts, updated: 1 },
            ruleweight: counts,
            target: { ...counts, updated: 2 },
            exposureplan: { ...counts, inserted: 1 },
            acquiredimage: null,
            imagedata: null,
            grade_filled: 0,
            grade_preserved: 0,
            imagedata_bytes: 0,
            total_inserted: 1,
            total_updated: 3,
          },
          error: null,
          status: 'ready',
        });
      })
    );

    const user = userEvent.setup();
    render(
      <SchedulerSyncControls database={local} databases={[local, telescope]} />,
      { wrapper: wrapper() }
    );

    await user.click(screen.getByText('Scheduler database sync'));
    await user.click(screen.getByRole('button', { name: 'Preview planning push' }));
    expect(await screen.findByText(/Preview: 1 rows added and 3 updated/)).toBeInTheDocument();
    expect(requests).toHaveLength(1);
    expect(requests[0]).toMatchObject({
      peer_db_id: 'scope',
      kind: 'push_planning',
      dry_run: true,
    });

    await user.click(screen.getByRole('button', { name: 'Apply planning push' }));
    expect(await screen.findByText(/Pushed planning settings to Telescope scheduler/)).toBeInTheDocument();
    expect(requests).toHaveLength(2);
    expect(requests[1]).toMatchObject({ kind: 'push_planning', dry_run: false });
  });

  it('offers a full pull with image data by default', async () => {
    let body: Record<string, unknown> | null = null;
    server.use(
      http.post('/api/databases/local/sync', async ({ request }) => {
        body = (await request.json()) as Record<string, unknown>;
        return HttpResponse.json({
          success: true,
          data: {
            kind: 'pull',
            dry_run: true,
            source_db_id: 'scope',
            destination_db_id: 'local',
            exposuretemplate: counts,
            project: counts,
            ruleweight: counts,
            target: counts,
            exposureplan: counts,
            acquiredimage: { ...counts, inserted: 5 },
            imagedata: { ...counts, inserted: 5 },
            grade_filled: 0,
            grade_preserved: 0,
            imagedata_bytes: 500,
            total_inserted: 10,
            total_updated: 0,
          },
          error: null,
          status: 'ready',
        });
      })
    );

    const user = userEvent.setup();
    render(
      <SchedulerSyncControls database={local} databases={[local, telescope]} />,
      { wrapper: wrapper() }
    );
    await user.click(screen.getByText('Scheduler database sync'));
    await user.click(screen.getByRole('button', { name: 'Preview full pull' }));
    expect(await screen.findByText(/Preview: 10 rows added/)).toBeInTheDocument();
    expect(body).toMatchObject({
      peer_db_id: 'scope',
      kind: 'pull',
      dry_run: true,
      with_image_data: true,
    });
  });
});
