import { Profiler, type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
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

  it('says which lights the calibration settings left out and which settings matched', async () => {
    mockCommon();
    server.use(
      http.get('/api/db/alpha/wbpp/runs/current', () =>
        ok({
          started: true,
          queued: [],
          progress: {
            ...running,
            uncalibrated_lights: 2,
            calibration_settings: ['Use darks: off (for Askar 120 · ASI2600MM Pro)'],
          },
        })
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
    expect(
      await screen.findByText(/2 light\(s\) excluded because they cannot be calibrated \(same rules as stacks\)/)
    ).toBeInTheDocument();
    expect(
      screen.getByText('Calibration frames matched with: Use darks: off (for Askar 120 · ASI2600MM Pro).')
    ).toBeInTheDocument();
  });

  it('names the flat changes in force and the lights WBPP cannot calibrate as a stack would', async () => {
    mockCommon();
    server.use(
      http.get('/api/db/alpha/wbpp/runs/current', () =>
        ok({
          started: true,
          queued: [],
          progress: {
            ...running,
            flat_changes: ['new dust: 1 shadow near (1423, 3545), between 2026-09-17 and 2026-09-21'],
            flat_warnings: 3,
          },
        })
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
    const line = await screen.findByTestId('wbpp-run-flat-changes');
    expect(line).toHaveTextContent('Flats chosen around measured changes: new dust: 1 shadow near (1423, 3545)');
    expect(line).toHaveTextContent('3 lights use flats from across a change');
  });
});

describe('saving WBPP masters', () => {
  it('locks copy controls from the request through completion and shows progress', async () => {
    mockCommon();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    let progress: WbppRunProgress = { ...running, running: false, stage: 'complete', finished_at: 10 };
    let copies = 0;
    let release!: () => void;
    const pending = new Promise<void>((resolve) => { release = resolve; });
    server.use(
      http.get('/api/databases', () => ok([{ id: 'alpha', process_directory: '/process' }])),
      http.get('/api/db/alpha/wbpp/runs/current', () => ok({ started: false, queued: [], progress })),
      http.post('/api/db/alpha/wbpp/runs/current/publish', async () => {
        copies++;
        await pending;
        progress = { ...progress, publish: {
          state: 'running', directory: '/process/Sh2 86/master', copied: 1, total: 3,
          current_file: 'masterLight_L.xisf', skipped_existing: 0, conflicts: [], errors: [], finished_at: null,
        } };
        return ok({ started: false, queued: [], progress });
      })
    );
    render(<QueryClientProvider client={client}>
      <WbppRunDialog request={{ dbId: 'alpha', scope: { project_id: 1 }, label: 'Sh2 86' }}
        defaultOptions={DEFAULT_WBPP_OPTIONS} onClose={() => {}} />
    </QueryClientProvider>);
    fireEvent.click(await screen.findByRole('button', { name: 'Save masters' }));
    const copying = await screen.findByRole('button', { name: 'Copying…' });
    expect(copying).toBeDisabled();
    expect(screen.getByRole('textbox', { name: 'Process folder' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Stack again' })).not.toBeInTheDocument();
    fireEvent.click(copying);
    release();
    expect(await screen.findByText(/1 of 3 files processed.*masterLight_L.xisf/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copying…' })).toBeDisabled();
    expect(copies).toBe(1);
    progress = { ...progress, publish: { ...progress.publish!, state: 'complete', copied: 3, current_file: null, finished_at: 20 } };
    await client.invalidateQueries({ queryKey: ['db', 'alpha', 'wbpp-run'] });
    expect(await screen.findByRole('button', { name: 'Save masters' })).toBeEnabled();
    expect(screen.getByRole('textbox', { name: 'Process folder' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Stack again' })).toBeEnabled();
  });

  it("shows the project's last folder in the first render with the finished run", () => {
    mockCommon();
    const finished: WbppRunProgress = { ...running, running: false, stage: 'complete', finished_at: 10 };
    const databases = [{ id: 'alpha', name: 'Alpha', process_directory: '/process' }];
    const status = { started: false, queued: [], progress: finished };
    const settings = { split_exposure_groups: false, process_folder: 'Sh2 86 masters' };
    server.use(
      http.get('/api/databases', () => ok(databases)),
      http.get('/api/db/alpha/wbpp/runs/current', () => ok(status)),
      http.get('/api/db/alpha/projects/:id/processing-settings', () => ok(settings))
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    client.setQueryData(['databases'], databases);
    client.setQueryData(['db', 'alpha', 'wbpp-run'], status);
    client.setQueryData(['pixinsight-settings'], pixinsight);
    client.setQueryData(['db', 'alpha', 'project', 1, 'processing-settings'], settings);
    // Read the DOM at every commit: a render that shows the finished run with
    // an empty folder is what a test or a person can catch between two commits.
    const shown: string[] = [];
    const record = () => {
      const input = document.querySelector<HTMLInputElement>('input[aria-label="Process folder"]');
      if (input) shown.push(input.value);
    };
    render(
      <QueryClientProvider client={client}>
        <Profiler id="wbpp-run-dialog" onRender={record}>
          <WbppRunDialog request={{ dbId: 'alpha', scope: { project_id: 1 }, label: 'Sh2 86' }}
            defaultOptions={DEFAULT_WBPP_OPTIONS} onClose={() => {}} />
        </Profiler>
      </QueryClientProvider>
    );
    expect(shown.length).toBeGreaterThan(0);
    expect(shown.filter((value) => value !== 'Sh2 86 masters')).toEqual([]);
  });

  it("follows the project's folder until the person edits it", async () => {
    mockCommon();
    let folder = 'Sh2 86 masters';
    server.use(
      http.get('/api/databases', () => ok([{ id: 'alpha', process_directory: '/process' }])),
      http.get('/api/db/alpha/wbpp/runs/current', () => ok({ started: false, queued: [],
        progress: { ...running, running: false, stage: 'complete', finished_at: 10 } })),
      http.get('/api/db/alpha/projects/:id/processing-settings', () => ok({ split_exposure_groups: false, process_folder: folder }))
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const settingsKey = ['db', 'alpha', 'project', 1, 'processing-settings'];
    render(<QueryClientProvider client={client}>
      <WbppRunDialog request={{ dbId: 'alpha', scope: { project_id: 1 }, label: 'Sh2 86' }}
        defaultOptions={DEFAULT_WBPP_OPTIONS} onClose={() => {}} />
    </QueryClientProvider>);
    const field = await screen.findByRole('textbox', { name: 'Process folder' });
    await waitFor(() => expect(field).toHaveValue('Sh2 86 masters'));
    folder = 'Sh2 86/2026';
    await act(() => client.refetchQueries({ queryKey: settingsKey }));
    await waitFor(() => expect(field).toHaveValue('Sh2 86/2026'));
    fireEvent.change(field, { target: { value: 'Sh2 86 retry' } });
    folder = 'Sh2 86/2027';
    await act(() => client.refetchQueries({ queryKey: settingsKey }));
    expect(field).toHaveValue('Sh2 86 retry');
  });

  it('explains the missing Gaia catalog without hiding the original error', async () => {
    mockCommon();
    server.use(http.get('/api/db/alpha/wbpp/runs/current', () => ok({ started: false, queued: [],
      progress: { ...running, running: false, stage: 'complete', finished_at: 10,
        log_errors: ['*** Error: No database files have been selected.'] },
    })));
    render(<WbppRunDialog request={{ dbId: 'alpha', scope: { project_id: 1 }, label: 'Sh2 86' }}
      defaultOptions={DEFAULT_WBPP_OPTIONS} onClose={() => {}} />, { wrapper: wrapper() });
    expect(await screen.findByText(/PixInsight has no Gaia catalog files selected/)).toBeInTheDocument();
    expect(screen.getByText('*** Error: No database files have been selected.')).toBeInTheDocument();
  });
});
