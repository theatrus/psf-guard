import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { HttpResponse, http } from 'msw';
import { beforeEach, describe, expect, it } from 'vitest';
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
  beforeEach(() => {
    sessionStorage.clear();
  });

  it('restores a durable preview after the Settings UI reloads', async () => {
    const result = {
      kind: 'push_planning' as const,
      dry_run: true,
      source_db_id: 'local',
      destination_db_id: 'scope',
      exposuretemplate: counts,
      project: { ...counts, updated: 1 },
      ruleweight: counts,
      target: counts,
      exposureplan: counts,
      acquiredimage: null,
      imagedata: null,
      grades: null,
      grade_filled: 0,
      grade_preserved: 0,
      imagedata_bytes: 0,
      total_inserted: 0,
      total_updated: 1,
    };
    sessionStorage.setItem(
      'psf-guard.scheduler-sync-preview',
      JSON.stringify({ localDbId: 'local', previewId: 'restored-preview' })
    );
    server.use(
      http.get(
        '/api/databases/local/sync/previews/restored-preview',
        () => HttpResponse.json({
          success: true,
          data: {
            preview_id: 'restored-preview',
            created_at: 1,
            expires_at: 4_102_444_800,
            result,
          },
          error: null,
          status: 'ready',
        })
      )
    );

    render(<SchedulerSyncControls databases={[local, telescope]} />, {
      wrapper: wrapper(),
    });

    expect(await screen.findByText('Restored the pending transfer preview.'))
      .toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Apply this preview' }))
      .toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Send planning' }))
      .toHaveAttribute('aria-pressed', 'true');
  });

  it('previews before applying a planning-only push', async () => {
    const requests: Array<Record<string, unknown>> = [];
    const result = {
      kind: 'push_planning',
      dry_run: true,
      source_db_id: 'local',
      destination_db_id: 'scope',
      exposuretemplate: counts,
      project: { ...counts, updated: 1 },
      ruleweight: counts,
      target: { ...counts, updated: 2 },
      exposureplan: { ...counts, inserted: 1 },
      acquiredimage: null,
      imagedata: null,
      grades: null,
      grade_filled: 0,
      grade_preserved: 0,
      imagedata_bytes: 0,
      total_inserted: 1,
      total_updated: 3,
    };
    server.use(
      http.post('/api/databases/local/sync/preview', async ({ request }) => {
        const body = (await request.json()) as Record<string, unknown>;
        requests.push(body);
        return HttpResponse.json({
          success: true,
          data: {
            preview_id: 'planning-preview',
            created_at: 1,
            expires_at: 4_102_444_800,
            result,
          },
          error: null,
          status: 'ready',
        });
      }),
      http.post(
        '/api/databases/local/sync/previews/planning-preview/apply',
        () => {
          requests.push({ preview_id: 'planning-preview' });
          return HttpResponse.json({
            success: true,
            data: { ...result, dry_run: false },
            error: null,
            status: 'ready',
          });
        }
      )
    );

    const user = userEvent.setup();
    render(<SchedulerSyncControls databases={[local, telescope]} />, {
      wrapper: wrapper(),
    });

    await user.click(screen.getByRole('button', { name: 'Send planning' }));
    await user.click(screen.getByRole('button', { name: 'Preview changes' }));
    expect(await screen.findByText(/1 rows will be added and 3 updated/)).toBeInTheDocument();
    expect(requests).toHaveLength(1);
    expect(requests[0]).toMatchObject({
      peer_db_id: 'scope',
      kind: 'push_planning',
      dry_run: true,
    });

    await user.click(screen.getByRole('button', { name: 'Apply this preview' }));
    expect(await screen.findByText(/Send planning complete/)).toBeInTheDocument();
    expect(requests).toHaveLength(2);
    expect(requests[1]).toEqual({ preview_id: 'planning-preview' });
  });

  it('offers a full pull with image data by default', async () => {
    let body: Record<string, unknown> | null = null;
    server.use(
      http.post('/api/databases/scope/sync/preview', async ({ request }) => {
        body = (await request.json()) as Record<string, unknown>;
        return HttpResponse.json({
          success: true,
          data: {
            preview_id: 'merge-preview',
            created_at: 1,
            expires_at: 4_102_444_800,
            result: {
              kind: 'pull',
              dry_run: true,
              source_db_id: 'local',
              destination_db_id: 'scope',
              exposuretemplate: counts,
              project: counts,
              ruleweight: counts,
              target: counts,
              exposureplan: counts,
              acquiredimage: { ...counts, inserted: 5 },
              imagedata: { ...counts, inserted: 5 },
              grades: null,
              grade_filled: 0,
              grade_preserved: 0,
              imagedata_bytes: 500,
              total_inserted: 10,
              total_updated: 0,
            },
          },
          error: null,
          status: 'ready',
        });
      })
    );

    const user = userEvent.setup();
    render(<SchedulerSyncControls databases={[local, telescope]} />, {
      wrapper: wrapper(),
    });
    await user.click(screen.getByRole('button', { name: 'Preview changes' }));
    expect(await screen.findByText(/10 rows will be added/)).toBeInTheDocument();
    expect(body).toMatchObject({
      peer_db_id: 'local',
      kind: 'pull',
      dry_run: true,
      with_image_data: true,
    });

    await user.click(screen.getByRole('button', { name: 'Send planning' }));
    expect(
      screen.queryByRole('button', { name: 'Apply this preview' })
    ).not.toBeInTheDocument();
  });

  it('previews reviewed grades before offering a grade push', async () => {
    const requests: Array<Record<string, unknown>> = [];
    const result = {
      kind: 'push_grades',
      dry_run: true,
      source_db_id: 'local',
      destination_db_id: 'scope',
      exposuretemplate: counts,
      project: counts,
      ruleweight: counts,
      target: counts,
      exposureplan: counts,
      acquiredimage: null,
      imagedata: null,
      grades: {
        source_considered: 8,
        source_no_guid: 0,
        matched: 7,
        changed: 3,
        unchanged: 4,
        unmatched_source: 1,
        destination_only: 2,
        duplicate_guids: 0,
        transitions: { 'Pending→Accepted': 2, 'Pending→Rejected': 1 },
      },
      grade_filled: 0,
      grade_preserved: 0,
      imagedata_bytes: 0,
      total_inserted: 0,
      total_updated: 3,
    };
    server.use(
      http.post('/api/databases/local/sync/preview', async ({ request }) => {
        const body = (await request.json()) as Record<string, unknown>;
        requests.push(body);
        return HttpResponse.json({
          success: true,
          data: {
            preview_id: 'grade-preview',
            created_at: 1,
            expires_at: 4_102_444_800,
            result,
          },
          error: null,
          status: 'ready',
        });
      }),
      http.post('/api/databases/local/sync/previews/grade-preview/apply', () => {
        requests.push({ preview_id: 'grade-preview' });
        return HttpResponse.json({
          success: true,
          data: { ...result, dry_run: false },
          error: null,
          status: 'ready',
        });
      })
    );

    const user = userEvent.setup();
    render(<SchedulerSyncControls databases={[local, telescope]} />, {
      wrapper: wrapper(),
    });

    await user.click(screen.getByRole('button', { name: 'Send reviewed grades' }));
    await user.click(screen.getByRole('button', { name: 'Preview changes' }));

    expect(
      await screen.findByText('3 reviewed grade(s) will change')
    ).toBeInTheDocument();
    expect(screen.getByText('Pending→Accepted: 2')).toBeInTheDocument();
    expect(requests[0]).toMatchObject({
      peer_db_id: 'scope',
      kind: 'push_grades',
      dry_run: true,
      reviewed_only: true,
    });

    await user.click(screen.getByRole('button', { name: 'Apply this preview' }));
    expect(await screen.findByText(/Send reviewed grades complete/))
      .toBeInTheDocument();
    expect(requests[1]).toMatchObject({
      preview_id: 'grade-preview',
    });
  });

  it('drops a stale preview and requires a new preview', async () => {
    const result = {
      kind: 'push_planning',
      dry_run: true,
      source_db_id: 'local',
      destination_db_id: 'scope',
      exposuretemplate: counts,
      project: { ...counts, updated: 1 },
      ruleweight: counts,
      target: counts,
      exposureplan: counts,
      acquiredimage: null,
      imagedata: null,
      grades: null,
      grade_filled: 0,
      grade_preserved: 0,
      imagedata_bytes: 0,
      total_inserted: 0,
      total_updated: 1,
    };
    server.use(
      http.post('/api/databases/local/sync/preview', () =>
        HttpResponse.json({
          success: true,
          data: {
            preview_id: 'stale-preview',
            created_at: 1,
            expires_at: 4_102_444_800,
            result,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.post(
        '/api/databases/local/sync/previews/stale-preview/apply',
        () =>
          HttpResponse.json(
            {
              success: false,
              data: null,
              error:
                'This preview is stale because a source or destination database changed. Preview again.',
              status: null,
            },
            { status: 409 }
          )
      )
    );

    const user = userEvent.setup();
    render(<SchedulerSyncControls databases={[local, telescope]} />, {
      wrapper: wrapper(),
    });

    await user.click(screen.getByRole('button', { name: 'Send planning' }));
    await user.click(screen.getByRole('button', { name: 'Preview changes' }));
    await user.click(
      await screen.findByRole('button', { name: 'Apply this preview' })
    );

    expect(await screen.findByText(/Apply failed: This preview is stale/))
      .toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Apply this preview' })
    ).not.toBeInTheDocument();
  });
});
