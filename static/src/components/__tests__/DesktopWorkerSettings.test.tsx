import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DesktopWorker, SaveDesktopWorker } from '../../utils/tauri';
import DesktopWorkerSettings from '../DesktopWorkerSettings';

const native = vi.hoisted(() => ({
  get: vi.fn(), save: vi.fn(), start: vi.fn(), stop: vi.fn(), pause: vi.fn(),
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
    settings_path: '/home/ann/.config/psf-guard/worker.json',
  };
  native.get.mockImplementation(async () => worker);
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

describe('run a worker on this computer', () => {
  it('saves the settings, then starts, shows and pauses the worker', async () => {
    mount(<DesktopWorkerSettings />);
    const start = await screen.findByRole('button', { name: 'Start the worker' });
    expect(start).toBeDisabled();
    expect(screen.getByText(/Save the server's address and a worker token/)).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('Server address'), { target: { value: 'http://observatory:3000' } });
    fireEvent.change(screen.getByLabelText('Worker token'), { target: { value: 'psfw_secret' } });
    await userEvent.click(screen.getByRole('checkbox', { name: /RC-Astro tools/ }));
    await userEvent.click(screen.getByRole('radio', { name: /shares with the server/ }));
    fireEvent.change(screen.getByLabelText('Server folder 1'), { target: { value: '/srv/astro' } });
    fireEvent.change(screen.getByLabelText("This computer's folder 1"), { target: { value: 'Z:\\astro' } });
    await userEvent.click(screen.getByRole('checkbox', { name: 'Start the worker when I log in' }));
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(native.save).toHaveBeenCalledTimes(1));
    expect(native.save.mock.calls[0][0]).toEqual({
      server: 'http://observatory:3000',
      name: null,
      files: 'shared',
      maps: [{ server: '/srv/astro', local: 'Z:\\astro' }],
      work_dir: null,
      rc_astro: null,
      work: ['stack'],
      slots: 1,
      token: 'psfw_secret',
      start_at_login: true,
    });
    expect(await screen.findByLabelText('Worker token')).toHaveAttribute(
      'placeholder', 'Saved; enter a new token to replace it',
    );

    await userEvent.click(await screen.findByRole('button', { name: 'Start the worker' }));
    expect(await screen.findByText(/Connected to http:\/\/observatory:3000 as studio-pc/)).toBeInTheDocument();
    expect(screen.getByText('Stack of 30 frames, Ha channel · 50%')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Pause taking tasks' }));
    expect(native.pause).toHaveBeenCalledWith(true);
    expect(await screen.findByText(/Paused: taking no new tasks/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Resume taking tasks' })).toBeInTheDocument();
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
    await userEvent.click(screen.getByRole('checkbox', { name: /RC-Astro tools/ }));
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText('Choose at least one kind of work.')).toBeInTheDocument();
    expect(native.save).not.toHaveBeenCalled();

    await userEvent.click(stack);
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(native.save).toHaveBeenCalledTimes(1));
    expect(native.save.mock.calls[0][0]).toMatchObject({ token: null, work: ['stack'], slots: 2, name: 'studio-pc' });
  });

  it('points Linux at the systemd service', async () => {
    worker = { ...worker, supported: false };
    mount(<DesktopWorkerSettings />);
    expect(await screen.findByText(/psf-guard-worker/)).toBeInTheDocument();
    expect(screen.queryByLabelText('Server address')).not.toBeInTheDocument();
  });
});
