import { describe, expect, it } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import ProcessingToolsSettings from '../ProcessingToolsSettings';
import type { RcAstroCapabilities, SeizaNoiseModel } from '../../api/types';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null });
}

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const noiseVersion = 'noise-v1-d33 cpu f32, seiza-model 0.1.0, band scale 5';

const seizaMissing: SeizaNoiseModel = {
  installed: {
    folder: '/data/psf-guard/noise-model',
    runtime_problem: 'ONNX Runtime is not installed',
    model_problem: 'The noise model is not installed',
  },
  runtime_download: '1.31.1',
  download: { running: false, message: '' },
};

const seizaReady: SeizaNoiseModel = {
  installed: {
    folder: '/data/psf-guard/noise-model',
    runtime_path: '/data/psf-guard/noise-model/onnxruntime/libonnxruntime.so',
    runtime_version: '1.31.1',
    model: 'noise-v1-d33',
  },
  runtime_download: '1.31.1',
  download: { running: false, message: '' },
};

const rcAstro: RcAstroCapabilities['providers'] = [
  { category: 'sharpen', provider: 'bxt', name: 'RC-Astro BlurXTerminator', state: 'ready',
    version: '2.6.6', this_server: true, workers: ['studio'] },
  { category: 'denoise', provider: 'nxt', name: 'RC-Astro NoiseXTerminator', state: 'not_licensed',
    reason: 'RC-Astro NoiseXTerminator is not licensed on this server (Trial expired)',
    remedy: 'install_rc_astro', version: '2.6.6', this_server: true, workers: [] },
  { category: 'star_removal', provider: 'sxt', name: 'RC-Astro StarXTerminator', state: 'not_installed',
    reason: 'rc-astro is not installed on this server or on a connected worker',
    remedy: 'install_rc_astro', this_server: false, workers: [] },
];

function answer(seiza: SeizaNoiseModel | undefined, ready = false): RcAstroCapabilities {
  return {
    available: true,
    executable: '/usr/local/bin/rc-astro',
    tools: [],
    this_server: true, workers: ['studio'], tool_workers: {},
    providers: [
      ...rcAstro!,
      ready
        ? { category: 'denoise', provider: 'noise-model', name: 'Seiza noise model', state: 'ready',
          version: noiseVersion, this_server: true, workers: [] }
        : { category: 'denoise', provider: 'noise-model', name: 'Seiza noise model', state: 'not_installed',
          reason: 'The Seiza noise model is not downloaded.', remedy: 'download_noise_model',
          this_server: false, workers: [] },
    ],
    seiza,
  };
}

const row = (provider: string) =>
  within(document.querySelector<HTMLElement>(`[data-provider='${provider}']`)!);

describe('ProcessingToolsSettings', () => {
  it('lists every provider with its state, version and where it is installed', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(answer(seizaReady, true))));
    render(<ProcessingToolsSettings canManage />, { wrapper: wrapper() });

    expect(await screen.findByRole('heading', { name: 'Processing tools' })).toBeInTheDocument();
    await screen.findByText('RC-Astro BlurXTerminator');
    expect(row('bxt').getByText('Ready')).toBeInTheDocument();
    expect(row('bxt').getByText('Sharpening · 2.6.6 · on this server, studio')).toBeInTheDocument();
    expect(row('nxt').getByText('Not licensed')).toBeInTheDocument();
    expect(row('nxt').getByText(/is not licensed on this server \(Trial expired\)/)).toBeInTheDocument();
    expect(row('sxt').getByText('Not installed')).toBeInTheDocument();
    // RC-Astro tools have no download.
    expect(row('sxt').queryByRole('button')).toBeNull();
    expect(row('noise-model').getByText('Ready')).toBeInTheDocument();
    expect(row('noise-model').getByText(`Noise reduction · ${noiseVersion} · on this server`)).toBeInTheDocument();
    expect(screen.getByText('/usr/local/bin/rc-astro')).toBeInTheDocument();
    expect(screen.getByText('Installed · 1.31.1')).toBeInTheDocument();
  });

  it('downloads nothing until asked, then downloads both missing parts', async () => {
    let state = answer(seizaMissing);
    const requested: unknown[] = [];
    server.use(
      http.get('/api/tools/rc-astro', () => ok(state)),
      http.post('/api/tools/noise-model/download', async ({ request }) => {
        requested.push(await request.json());
        const download = {
          running: true, part: 'runtime' as const, message: 'Downloading ONNX Runtime 1.31.1…',
          bytes_completed: 5 * 1024 * 1024, bytes_total: 11 * 1024 * 1024,
        };
        state = answer({ ...seizaMissing, download });
        return ok(download);
      }),
    );
    render(<ProcessingToolsSettings canManage />, { wrapper: wrapper() });

    const both = await screen.findByRole('button', { name: 'Download both' });
    expect(row('noise-model').getByText('The Seiza noise model is not downloaded.')).toBeInTheDocument();
    expect(requested).toEqual([]);

    await userEvent.click(both);
    expect(requested).toEqual([{ parts: ['runtime', 'model'] }]);
    const progress = await screen.findByRole('status');
    expect(progress).toHaveTextContent('Downloading ONNX Runtime 1.31.1…');
    expect(progress).toHaveTextContent('5.0 MB of 11 MB');
    // One download at a time.
    expect(screen.getByRole('button', { name: 'Download the noise model' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Download ONNX Runtime' })).toBeDisabled();
  });

  it('downloads ONNX Runtime or the model on its own', async () => {
    const requested: unknown[] = [];
    server.use(
      http.get('/api/tools/rc-astro', () => ok(answer(seizaMissing))),
      http.post('/api/tools/noise-model/download', async ({ request }) => {
        requested.push(await request.json());
        return ok({ running: false, message: '' });
      }),
    );
    render(<ProcessingToolsSettings canManage />, { wrapper: wrapper() });
    await userEvent.click(await screen.findByRole('button', { name: 'Download ONNX Runtime' }));
    await waitFor(() => expect(requested).toEqual([{ parts: ['runtime'] }]));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Download the noise model' })).toBeEnabled());
    await userEvent.click(screen.getByRole('button', { name: 'Download the noise model' }));
    await waitFor(() => expect(requested).toEqual([{ parts: ['runtime'] }, { parts: ['model'] }]));
  });

  it('stops a download under way', async () => {
    let stopped = 0;
    server.use(
      http.get('/api/tools/rc-astro', () => ok(answer({ ...seizaMissing, download: {
        running: true, part: 'model', message: 'Downloading noise-v1-d33.onnx…',
      } }))),
      http.post('/api/tools/noise-model/download/stop', () => { stopped += 1; return ok(true); }),
    );
    render(<ProcessingToolsSettings canManage />, { wrapper: wrapper() });
    await userEvent.click(await screen.findByRole('button', { name: 'Stop download' }));
    await waitFor(() => expect(stopped).toBe(1));
  });

  it('offers an update of an installed model', async () => {
    const requested: unknown[] = [];
    server.use(
      http.get('/api/tools/rc-astro', () => ok(answer(seizaReady, true))),
      http.post('/api/tools/noise-model/download', async ({ request }) => {
        requested.push(await request.json());
        return ok({ running: true, part: 'model', message: 'Starting…' });
      }),
    );
    render(<ProcessingToolsSettings canManage />, { wrapper: wrapper() });

    const update = await screen.findByRole('button', { name: 'Update the noise model' });
    expect(update).toHaveTextContent('Update');
    expect(screen.queryByRole('button', { name: 'Download both' })).toBeNull();
    await userEvent.click(update);
    expect(requested).toEqual([{ parts: ['model'] }]);
  });

  it('asks for ONNX Runtime again when it is installed already', async () => {
    const requested: unknown[] = [];
    server.use(
      http.get('/api/tools/rc-astro', () => ok(answer(seizaReady, true))),
      http.post('/api/tools/noise-model/download', async ({ request }) => {
        requested.push(await request.json());
        return ok({ running: true, part: 'runtime', message: 'Starting…' });
      }),
    );
    render(<ProcessingToolsSettings canManage />, { wrapper: wrapper() });

    // Without force the server answers that the release is installed.
    await userEvent.click(await screen.findByRole('button', { name: 'Download ONNX Runtime again' }));
    await waitFor(() => expect(requested).toEqual([{ parts: ['runtime'], force: true }]));
  });

  it('saves the path of an ONNX Runtime the person installed', async () => {
    let state = answer(seizaMissing);
    const saved: unknown[] = [];
    server.use(
      http.get('/api/tools/rc-astro', () => ok(state)),
      http.put('/api/settings/noise-model', async ({ request }) => {
        saved.push(await request.json());
        const seiza = { ...seizaReady, onnxruntime: '/opt/onnxruntime/lib' };
        state = answer(seiza, true);
        // The answer is the Seiza part alone.
        return ok(seiza);
      }),
    );
    render(<ProcessingToolsSettings canManage />, { wrapper: wrapper() });

    const path = await screen.findByRole('textbox', { name: 'ONNX Runtime path' });
    const save = screen.getByRole('button', { name: 'Save path' });
    expect(save).toBeDisabled();
    await userEvent.type(path, ' /opt/onnxruntime/lib ');
    await userEvent.click(save);
    await waitFor(() => expect(saved).toEqual([{ onnxruntime: '/opt/onnxruntime/lib' }]));
    // With a path set, PSF Guard offers no download of its own.
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: /Download ONNX Runtime/ })).toBeNull());
    expect(path).toHaveValue('/opt/onnxruntime/lib');
    await waitFor(() => expect(row('noise-model').getByText('Ready')).toBeInTheDocument());
  });

  it('shows a viewer no path field, and no ONNX Runtime download where there is none', async () => {
    server.use(http.get('/api/tools/rc-astro', () =>
      ok(answer({ ...seizaMissing, runtime_download: undefined }))));
    render(<ProcessingToolsSettings canManage={false} />, { wrapper: wrapper() });

    expect(await screen.findByText(
      'PSF Guard has no download for this computer. Install ONNX Runtime 1.17 or later and set its path.'
    )).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Download ONNX Runtime' })).toBeNull();
    expect(screen.queryByRole('textbox', { name: 'ONNX Runtime path' })).toBeNull();
    // The model alone has a download, so there is no "both".
    expect(screen.queryByRole('button', { name: 'Download both' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Download the noise model' })).toBeEnabled();
  });

  it('lists the RC-Astro tools of a server that sends no provider list', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok({
      available: true,
      tools: [{ schema_version: 6, cli_version: '2.6.6', key: 'bxt', name: 'RC-Astro BlurXTerminator',
        ml_version: 4, licensed: false, license_message: 'Trial expired', parameters: [] }],
    })));
    render(<ProcessingToolsSettings canManage />, { wrapper: wrapper() });
    await screen.findByText('RC-Astro BlurXTerminator');
    expect(row('bxt').getByText('Not licensed')).toBeInTheDocument();
    expect(row('bxt').getByText('Trial expired')).toBeInTheDocument();
    // Nothing of Seiza's shows.
    expect(screen.queryByText('ONNX Runtime')).toBeNull();
    expect(screen.queryByRole('textbox', { name: 'ONNX Runtime path' })).toBeNull();
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('scrolls into view when a processing card sent the person here', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(answer(seizaMissing))));
    const scrolled: Element[] = [];
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function scrollIntoView() { scrolled.push(this); };
    try {
      render(<ProcessingToolsSettings canManage focus="processing_tools" />, { wrapper: wrapper() });
      const heading = await screen.findByRole('heading', { name: 'Processing tools' });
      expect(scrolled).toEqual([heading.closest('section')]);
    } finally {
      Element.prototype.scrollIntoView = original;
    }
  });
});
