import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DesktopWorker, SaveDesktopWorker } from '../../utils/tauri';
import DesktopWorkerSettings from '../DesktopWorkerSettings';

const native = vi.hoisted(() => ({
  get: vi.fn(), save: vi.fn(), start: vi.fn(), stop: vi.fn(), pause: vi.fn(),
  noiseModel: vi.fn(), downloadNoiseModel: vi.fn(),
}));
vi.mock('../../utils/tauri', async importOriginal => {
  const original = await importOriginal<typeof import('../../utils/tauri')>();
  return { ...original, isTauriApp: () => true, desktopWorker: native };
});

let worker: DesktopWorker;
beforeEach(() => {
  worker = {
    supported: true,
    settings: null,
    has_token: false,
    start_at_login: false,
    status: null,
    service: null,
    settings_path: '/home/ann/.config/psf-guard/worker.json',
  };
  native.get.mockImplementation(async () => worker);
  native.noiseModel.mockReset();
  native.downloadNoiseModel.mockReset();
  native.noiseModel.mockImplementation(async () => null);
  native.save.mockImplementation(async (saved: SaveDesktopWorker) => {
    const { token, start_at_login, ...settings } = saved;
    worker = { ...worker, settings, has_token: worker.has_token || !!token, start_at_login };
    return worker;
  });
  native.start.mockImplementation(async () => {
    worker = {
      ...worker,
      status: {
        server: 'http://observatory:3000', name: 'studio-pc', version: '0.13.0',
        connection: 'connected', paused: false, problem: null,
        tasks: [{ id: 'a', label: 'Stack of 30 frames, Ha channel', fraction: 0.5 }],
      },
    };
    return worker;
  });
  native.pause.mockImplementation(async (paused: boolean) => {
    worker = { ...worker, status: worker.status && { ...worker.status, paused, tasks: [] } };
    return worker;
  });
});

function mount(children: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return render(<QueryClientProvider client={client}>{children}</QueryClientProvider>);
}

describe('this computer\'s worker', () => {
  it('saves the settings, then starts, shows and pauses the worker', async () => {
    mount(<DesktopWorkerSettings />);
    const start = await screen.findByRole('button', { name: 'Start worker' });
    expect(start).toBeDisabled();
    expect(screen.getByText(/Save a server address and a worker token/)).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('Server address'), { target: { value: 'http://observatory:3000' } });
    fireEvent.change(screen.getByLabelText('Worker token'), { target: { value: 'psfw_secret' } });
    await userEvent.click(screen.getByRole('checkbox', { name: 'Processing steps' }));
    await userEvent.click(screen.getByRole('checkbox', { name: 'Use shared storage' }));
    fireEvent.change(screen.getByLabelText('Server folder 1'), { target: { value: '/srv/astro' } });
    fireEvent.change(screen.getByLabelText('Folder on this computer 1'), { target: { value: 'Z:\\astro' } });
    await userEvent.click(screen.getByRole('checkbox', { name: 'Start worker at login' }));
    await userEvent.click(screen.getByRole('button', { name: 'Save worker settings' }));

    await waitFor(() => expect(native.save).toHaveBeenCalledTimes(1));
    expect(native.save.mock.calls[0][0]).toEqual({
      server: 'http://observatory:3000',
      name: null,
      files: 'shared',
      maps: [{ server: '/srv/astro', local: 'Z:\\astro' }],
      work_dir: null,
      rc_astro: null,
      onnxruntime: null,
      work: ['stack'],
      slots: 1,
      token: 'psfw_secret',
      start_at_login: true,
    });
    expect(await screen.findByLabelText('Worker token')).toHaveAttribute('placeholder', 'Saved token');
    expect(screen.getByLabelText('Worker token')).toHaveAccessibleDescription('Leave blank to keep the saved token.');
    expect(screen.getByLabelText('Worker name')).toHaveAccessibleDescription("Leave blank to use this computer's host name.");
    expect(screen.getByLabelText('Concurrent tasks')).toHaveAccessibleDescription(/Maximum tasks.*1 to 16/);
    expect(screen.getByLabelText('Temporary task folder')).toHaveAccessibleDescription(/Leave blank to use the system temporary folder/);

    await userEvent.click(await screen.findByRole('button', { name: 'Start worker' }));
    expect(await screen.findByText('Connected to http://observatory:3000.')).toBeInTheDocument();
    expect(screen.getByText('Stack of 30 frames, Ha channel: 50%')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Pause worker' }));
    expect(native.pause).toHaveBeenCalledWith(true);
    expect(await screen.findByText('Paused. Connected to http://observatory:3000.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Resume worker' })).toBeInTheDocument();
  });

  it('keeps a saved token when the field is left blank, and refuses no work at all', async () => {
    worker = {
      ...worker,
      has_token: true,
      settings: {
        server: 'http://observatory:3000', name: 'studio-pc', files: 'remote', maps: [],
        work_dir: null, rc_astro: null, work: [], slots: 2,
      },
    };
    mount(<DesktopWorkerSettings />);
    const stack = await screen.findByRole('checkbox', { name: 'Stack builds' });
    expect(stack).toBeChecked();
    await userEvent.click(stack);
    await userEvent.click(screen.getByRole('checkbox', { name: 'Processing steps' }));
    await userEvent.click(screen.getByRole('button', { name: 'Save worker settings' }));
    expect(await screen.findByText('Select at least one task type: Stack builds or Processing steps.')).toBeInTheDocument();
    expect(native.save).not.toHaveBeenCalled();

    await userEvent.click(stack);
    await userEvent.click(screen.getByRole('button', { name: 'Save worker settings' }));
    await waitFor(() => expect(native.save).toHaveBeenCalledTimes(1));
    expect(native.save.mock.calls[0][0]).toMatchObject({ token: null, work: ['stack'], slots: 2, name: 'studio-pc' });
  });

  it.each(['running', 'changing'] as const)('prevents a desktop worker while the Windows service is %s', async (service) => {
    worker = {
      ...worker,
      has_token: true,
      service,
      settings: {
        server: 'http://observatory:3000', name: null, files: 'remote', maps: [],
        work_dir: null, rc_astro: null, work: [], slots: 1,
      },
    };
    mount(<DesktopWorkerSettings />);
    expect(await screen.findByText(service === 'running'
      ? 'The PSF Guard Worker Windows service is running.'
      : 'The PSF Guard Worker Windows service has not stopped. Stop it in Windows Services to use the desktop worker.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Start worker' })).toBeDisabled();
    expect(screen.getByText(/updates the service's copies/)).toBeInTheDocument();
  });

  it('points Linux at the systemd service', async () => {
    worker = { ...worker, supported: false };
    mount(<DesktopWorkerSettings />);
    expect(await screen.findByText(/psf-guard-worker/)).toBeInTheDocument();
    expect(screen.queryByLabelText('Server address')).not.toBeInTheDocument();
  });

  it('downloads the noise model only when asked, and saves an ONNX Runtime path', async () => {
    worker = {
      ...worker,
      has_token: true,
      settings: {
        server: 'http://observatory:3000', name: null, files: 'remote', maps: [],
        work_dir: null, rc_astro: null, onnxruntime: null, work: [], slots: 1,
      },
    };
    native.noiseModel.mockImplementation(async () => ({
      installed: { folder: '/home/ann/.local/share/psf-guard/noise-model', runtime_problem: 'ONNX Runtime is not installed' },
      runtime_download: '1.31.1',
    }));
    native.downloadNoiseModel.mockImplementation(async () => ({
      installed: { folder: '/home/ann/.local/share/psf-guard/noise-model', runtime_path: '/home/ann/.local/share/psf-guard/noise-model/onnxruntime-1.31.1-a/libonnxruntime.so', runtime_version: '1.31.1', model: 'noise-v1-d33' },
      runtime_download: '1.31.1',
    }));
    mount(<DesktopWorkerSettings />);

    const model = await screen.findByRole('group', { name: 'Noise model' });
    expect(model).toHaveTextContent('ONNX Runtime is not installed; the model is not installed.');
    expect(native.downloadNoiseModel).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Download noise model' }));
    expect(native.downloadNoiseModel).toHaveBeenCalledWith({ runtime: true, model: true });
    expect(await screen.findByText(/Installed: noise-v1-d33, ONNX Runtime 1.31.1/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Update noise model' })).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('ONNX Runtime path'), { target: { value: ' /opt/onnxruntime/lib ' } });
    await userEvent.click(screen.getByRole('button', { name: 'Save worker settings' }));
    await waitFor(() => expect(native.save).toHaveBeenCalledTimes(1));
    expect(native.save.mock.calls[0][0]).toMatchObject({ onnxruntime: '/opt/onnxruntime/lib', work: [] });
  });
});
