import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import WbppRunDialog from '../WbppRunDialog';
import { DEFAULT_WBPP_OPTIONS } from '../../api/types';
import type { WbppRunProgress } from '../../api/types';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const running: WbppRunProgress = {
  running: true,
  stage: 'running',
  scope: 'Sh2 86',
  work_dir: '/runs/alpha/Sh2_86-1',
  output_dir: '/runs/alpha/Sh2_86-1/wbpp-out',
  free_bytes_at_start: null,
  options: null,
  frames: 40,
  lights: 30,
  missing_files: 0,
  command: null,
  pid: 4242,
  started_at: 1_705_352_400,
  finished_at: null,
  exit_code: null,
  wbpp_stage: 'Begin registration of light frames',
  wbpp_steps: 3,
  wbpp_elapsed: null,
  log_path: null,
  log_tail: [],
  log_errors: [],
  outputs: [],
  error: null,
  project_id: 1,
  publish: null,
};

const pixinsight = {
  binary: null,
  detection: {
    install: {
      binary: '/opt/PixInsight/bin/PixInsight.sh',
      root: '/opt/PixInsight',
      bpp_main: '/opt/PixInsight/src/scripts/BatchPreprocessing/BPP-Main.js',
      wbpp_version: '3.1.0',
    },
    source: 'detected',
    checked: [],
    problem: null,
  },
  display: { kind: 'own' },
  ready: true,
  runs_dir: null,
  runs_dir_free_bytes: null,
};

function mockCommon() {
  server.use(
    http.get('/api/settings/pixinsight', () => ok(pixinsight)),
    http.get('/api/databases', () =>
      ok([{ id: 'alpha', name: 'Alpha', database_path: '/a.sqlite', image_directories: [], remote_image_upload: { enabled: false } }])
    ),
    http.get('/api/db/alpha/projects/:id/processing-settings', () => ok({ process_folder: null }))
  );
}

describe('WbppRunDialog while another project runs', () => {
  it('opens under the clicked project, names the busy run, and queues behind it', async () => {
    mockCommon();
    let started: unknown = null;
    let queued: unknown[] = [];
    server.use(
      http.get('/api/db/alpha/wbpp/runs/current', () =>
        ok({ started: true, queued, progress: running })
      ),
      http.post('/api/db/alpha/wbpp/runs', async ({ request }) => {
        started = await request.json();
        queued = [{ id: 'q1', scope: 'NGC 6820', project_id: 2, target_id: null, position: 1, queued_at: 5 }];
        return ok({ started: false, queue_id: 'q1', queued, progress: running });
      }),
      http.delete('/api/db/alpha/wbpp/runs/queue/q1', () => {
        queued = [];
        return ok({ started: true, queued, progress: running });
      })
    );
    render(
      <WbppRunDialog
        request={{ dbId: 'alpha', scope: { project_id: 2 }, label: 'NGC 6820' }}
        defaultOptions={DEFAULT_WBPP_OPTIONS}
        onClose={() => {}}
      />,
      { wrapper: wrapper() }
    );
    expect(await screen.findByText('Stack with WBPP — NGC 6820')).toBeInTheDocument();
    expect(await screen.findByText('PixInsight is busy:')).toBeInTheDocument();
    expect(screen.getByText(/WBPP Sh2 86: Begin registration/)).toBeInTheDocument();
    expect(screen.queryByText('Stop PixInsight')).not.toBeInTheDocument();

    const queue = await screen.findByRole('button', { name: 'Queue stacking' });
    fireEvent.click(queue);
    await waitFor(() => expect(started).not.toBeNull());
    expect((started as { scope_label: string; project_id: number }).scope_label).toBe('NGC 6820');
    expect((started as { project_id: number }).project_id).toBe(2);
    expect(await screen.findByText('WBPP NGC 6820 is next in line.')).toBeInTheDocument();
    expect(screen.getByText('Stack with WBPP — NGC 6820')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Remove from queue' }));
    expect(await screen.findByRole('button', { name: 'Queue stacking' })).toBeInTheDocument();

    // Show that run switches the view to the running project, with its name.
    fireEvent.click(screen.getByRole('button', { name: 'Show that run' }));
    expect(await screen.findByText('Stack with WBPP — Sh2 86')).toBeInTheDocument();
    expect(screen.getByText('PixInsight is running WBPP')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Stop PixInsight' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Back to NGC 6820' }));
    expect(await screen.findByText('Stack with WBPP — NGC 6820')).toBeInTheDocument();
  });

  it("shows the project's own run under its name", async () => {
    mockCommon();
    server.use(
      http.get('/api/db/alpha/wbpp/runs/current', () =>
        ok({ started: true, queued: [], progress: running })
      )
    );
    render(
      <WbppRunDialog
        request={{ dbId: 'alpha', scope: { project_id: 1 }, label: 'Sh2 86' }}
        defaultOptions={DEFAULT_WBPP_OPTIONS}
        onClose={() => {}}
      />,
      { wrapper: wrapper() }
    );
    expect(await screen.findByText('PixInsight is running WBPP')).toBeInTheDocument();
    expect(screen.getByText('Stack with WBPP — Sh2 86')).toBeInTheDocument();
    expect(screen.queryByText('PixInsight is busy:')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /stacking$/ })).not.toBeInTheDocument();
  });
});
