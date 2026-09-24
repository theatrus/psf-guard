import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import WbppRunDialog from '../WbppRunDialog';
import { DEFAULT_WBPP_OPTIONS, type WbppRunProgress } from '../../api/types';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const ok = <T,>(data: T) => ({ success: true, data, error: null });

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
};

const idle: WbppRunProgress = {
  running: false,
  stage: '',
  scope: '',
  work_dir: '',
  output_dir: '',
  options: null,
  frames: 0,
  lights: 0,
  missing_files: 0,
  command: null,
  pid: null,
  started_at: null,
  finished_at: null,
  exit_code: null,
  wbpp_stage: null,
  wbpp_steps: 0,
  wbpp_elapsed: null,
  log_path: null,
  log_tail: [],
  log_errors: [],
  outputs: [],
  error: null,
};

const finished: WbppRunProgress = {
  ...idle,
  stage: 'complete',
  scope: 'project Alpha',
  work_dir: '/cache/db/wbpp/Alpha-1',
  output_dir: '/cache/db/wbpp/Alpha-1/wbpp-out',
  frames: 5,
  lights: 3,
  started_at: 1_000,
  finished_at: 1_600,
  exit_code: 0,
  wbpp_elapsed: '00:09:58.000',
  log_path: '/cache/db/wbpp/Alpha-1/wbpp-out/logs/run.log',
  log_tail: ['* WeightedBatchPreprocessing: 00:09:58.000'],
  outputs: [
    { path: 'master/masterLight_BIN-1_FILTER-L.xisf', size_bytes: 104_857_600, kind: 'master' },
    { path: 'logs/run.log', size_bytes: 2048, kind: 'log' },
  ],
};

const request = { dbId: 'alpha', scope: { project_id: 1 }, label: 'Project Alpha' };

describe('WbppRunDialog', () => {
  it('starts a run with the chosen settings and shows what it wrote', async () => {
    let started: unknown = null;
    let progress = idle;
    server.use(
      http.get('/api/settings/pixinsight', () => HttpResponse.json(ok(pixinsight))),
      http.get('/api/db/alpha/wbpp/runs/current', () =>
        HttpResponse.json(ok({ started: progress.running, progress }))
      ),
      http.post('/api/db/alpha/wbpp/runs', async ({ request: req }) => {
        started = await req.json();
        progress = finished;
        return HttpResponse.json(ok({ started: true, progress: finished }));
      })
    );
    render(
      <WbppRunDialog request={request} defaultOptions={DEFAULT_WBPP_OPTIONS} onClose={() => {}} />,
      { wrapper: wrapper() }
    );
    expect(await screen.findByRole('status')).toHaveTextContent('WBPP 3.1.0 found');
    fireEvent.change(screen.getByLabelText(/^Drizzle/), { target: { value: '2x' } });
    fireEvent.change(screen.getByLabelText(/More WBPP parameters/), {
      target: { value: 'maxStars=500\nautocrop=false' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start stacking' }));
    await waitFor(() =>
      expect(started).toEqual({
        project_id: 1,
        include_pending: true,
        options: { ...DEFAULT_WBPP_OPTIONS, drizzle: '2x' },
        extra_params: ['maxStars=500', 'autocrop=false'],
        scope_label: 'Project Alpha',
      })
    );
    expect(await screen.findByText('Finished')).toBeInTheDocument();
    const master = screen.getByRole('link', { name: 'masterLight_BIN-1_FILTER-L.xisf' });
    expect(master).toHaveAttribute(
      'href',
      '/api/db/alpha/wbpp/runs/current/files/wbpp-out/master/masterLight_BIN-1_FILTER-L.xisf'
    );
    expect(screen.getByText('100.0 MiB')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'WBPP log' })).toHaveAttribute(
      'href',
      '/api/db/alpha/wbpp/runs/current/files/wbpp-out/logs/run.log'
    );
    expect(screen.getByRole('button', { name: 'Stack again' })).toBeInTheDocument();
  });

  it('opens on the last run\u2019s results and offers the form again on request', async () => {
    server.use(
      http.get('/api/settings/pixinsight', () => HttpResponse.json(ok(pixinsight))),
      http.get('/api/db/alpha/wbpp/runs/current', () =>
        HttpResponse.json(ok({ started: false, progress: finished }))
      )
    );
    render(
      <WbppRunDialog request={request} defaultOptions={DEFAULT_WBPP_OPTIONS} onClose={() => {}} />,
      { wrapper: wrapper() }
    );
    expect(await screen.findByText('Finished')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Start stacking' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Stack again' }));
    expect(screen.getByRole('button', { name: 'Start stacking' })).toBeInTheDocument();
    expect(screen.queryByText('Finished')).not.toBeInTheDocument();
  });

  it('cannot start without PixInsight, and follows a run already under way', async () => {
    const running: WbppRunProgress = {
      ...idle,
      running: true,
      stage: 'running',
      scope: 'project Alpha',
      lights: 3,
      frames: 5,
      started_at: Math.floor(Date.now() / 1000) - 65,
      wbpp_stage: 'Begin registration of light frames',
      wbpp_steps: 2,
      log_tail: ['Registering 3 frames'],
    };
    server.use(
      http.get('/api/settings/pixinsight', () =>
        HttpResponse.json(
          ok({ ...pixinsight, ready: false, detection: { ...pixinsight.detection, install: null, checked: ['/opt/PixInsight/bin/PixInsight.sh'] } })
        )
      ),
      http.get('/api/db/alpha/wbpp/runs/current', () =>
        HttpResponse.json(ok({ started: true, progress: running }))
      )
    );
    render(
      <WbppRunDialog request={request} defaultOptions={DEFAULT_WBPP_OPTIONS} onClose={() => {}} />,
      { wrapper: wrapper() }
    );
    expect(await screen.findByText('PixInsight is running WBPP')).toBeInTheDocument();
    expect(screen.getByText(/Begin registration of light frames/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Stop PixInsight' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Start stacking' })).not.toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('PixInsight not found');
  });
});
