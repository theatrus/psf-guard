import { Profiler, type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { apiClient } from '../../api/client';
import AuthGate from '../../auth/AccessContext';
import type { DesktopServerStatus } from '../../utils/tauri';
import DesktopServerSettings, { DesktopServerGate } from '../DesktopServerSettings';
import TauriSettings from '../TauriSettings';
import UnsavedMark from '../UnsavedMark';

const native = vi.hoisted(() => ({
  isTauriApp: vi.fn(), getSettings: vi.fn(), savePort: vi.fn(), restart: vi.fn(),
}));
vi.mock('../../utils/tauri', async importOriginal => {
  const original = await importOriginal<typeof import('../../utils/tauri')>();
  return { ...original, isTauriApp: native.isTauriApp,
    desktopServer: { getSettings: native.getSettings, savePort: native.savePort },
    tauriConfig: { ...original.tauriConfig, restartServer: native.restart },
  };
});

let status: DesktopServerStatus;
beforeEach(() => {
  native.isTauriApp.mockReturnValue(true);
  status = { server_port: null, active_server_port: null, server_url: 'http://127.0.0.1:55001', starting: false, error: null };
  native.getSettings.mockImplementation(async () => status);
  native.savePort.mockImplementation(async (port: number | null) => {
    status = { ...status, server_port: port };
    return status;
  });
  native.restart.mockResolvedValue('http://127.0.0.1:3030');
});

function testClient() {
  return new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
}

function mount(children: ReactNode, client = testClient()) {
  return render(<QueryClientProvider client={client}>{children}</QueryClientProvider>);
}

describe('desktop server settings', () => {
  it('keeps a pending port restart disabled while the port has unsaved edits', async () => {
    status = { ...status, server_port: 3030 };
    mount(<MemoryRouter><TauriSettings isOpen onClose={() => {}} /></MemoryRouter>);
    await userEvent.click(await screen.findByRole('tab', { name: 'Server' }));
    await userEvent.click(await screen.findByRole('radio', { name: 'Fixed port' }));
    fireEvent.change(screen.getByRole('spinbutton', { name: 'Server port' }), { target: { value: '4040' } });
    const buttons = screen.getAllByRole('button', { name: /^Restart server$/i });
    expect(buttons).toHaveLength(1);
    for (const button of buttons) {
      expect(button).toBeDisabled();
      await userEvent.click(button);
    }
    expect(native.restart).not.toHaveBeenCalled();
  });

  it('offers no general restart when the active port matches the saved setting', async () => {
    mount(<MemoryRouter><TauriSettings isOpen onClose={() => {}} /></MemoryRouter>);
    expect(await screen.findByRole('button', { name: 'Open logs' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Restart server$/i })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('tab', { name: 'Server' }));
    await screen.findByRole('radio', { name: 'Automatic' });
    expect(screen.queryByRole('button', { name: /^Restart server$/i })).not.toBeInTheDocument();
  });

  it('does not restart over another form with unsaved edits', async () => {
    status = { ...status, server_port: 3030 };
    const reloaded = vi.fn();
    const { rerender } = mount(<><DesktopServerSettings onRestarted={reloaded} /><UnsavedMark label="Rig profile" dirty /></>);
    const restart = await screen.findByRole('button', { name: 'Restart server' });
    expect(restart).toBeDisabled();
    await userEvent.click(restart);
    expect(native.restart).not.toHaveBeenCalled();
    rerender(<QueryClientProvider client={testClient()}><DesktopServerSettings onRestarted={reloaded} /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Restart server' })).toBeEnabled());
  });

  it('saves a fixed port separately from restarting, then reloads only after readiness', async () => {
    const reloaded = vi.fn();
    let ready!: (url: string) => void;
    native.restart.mockImplementation(() => new Promise<string>(resolve => { ready = resolve; }));
    mount(<DesktopServerSettings onRestarted={reloaded} />);
    await userEvent.click(await screen.findByRole('radio', { name: 'Fixed port' }));
    fireEvent.change(screen.getByRole('spinbutton', { name: 'Server port' }), { target: { value: '3030' } });
    await userEvent.click(screen.getByRole('button', { name: 'Save port' }));
    await waitFor(() => expect(native.savePort).toHaveBeenCalledWith(3030, expect.anything()));
    expect(native.restart).not.toHaveBeenCalled();
    expect(screen.getByText(/Running at/)).toHaveTextContent('http://127.0.0.1:55001');
    expect(await screen.findByRole('status')).toHaveTextContent('Port 3030 is saved. Restart');
    await userEvent.click(screen.getByRole('button', { name: 'Restart server' }));
    expect(reloaded).not.toHaveBeenCalled();
    ready('http://127.0.0.1:3030');
    await waitFor(() => expect(reloaded).toHaveBeenCalledOnce());
  });

  it('restores a saved fixed port and can return to Automatic', async () => {
    status = { ...status, server_port: 3030, active_server_port: 3030, server_url: 'http://127.0.0.1:3030' };
    mount(<DesktopServerSettings />);
    expect(await screen.findByRole('spinbutton', { name: 'Server port' })).toHaveValue(3030);
    await userEvent.click(screen.getByRole('radio', { name: 'Automatic' }));
    await userEvent.click(screen.getByRole('button', { name: 'Save port' }));
    await waitFor(() => expect(native.savePort).toHaveBeenCalledWith(null, expect.anything()));
    expect(await screen.findByRole('status')).toHaveTextContent('Automatic port selection is saved');
  });

  it('shows a saved fixed port in the first render with settings', () => {
    status = { ...status, server_port: 3030, active_server_port: 3030, server_url: 'http://127.0.0.1:3030' };
    const client = testClient();
    client.setQueryData(['desktopServerSettings'], status);
    // Read the DOM at every commit: a render that shows Automatic before the
    // saved port is what a test or a person can catch between two commits.
    const shown: string[] = [];
    const record = () => {
      if (!document.querySelector('[aria-label="Desktop server"]')) return;
      const port = document.querySelector<HTMLInputElement>('input[type="number"]');
      shown.push(port ? port.value : 'Automatic');
    };
    mount(<Profiler id="desktop-server" onRender={record}><DesktopServerSettings /></Profiler>, client);
    expect(shown.length).toBeGreaterThan(0);
    expect(shown.filter(port => port !== '3030')).toEqual([]);
  });

  it('a refetch updates an unedited port choice and keeps an edited one', async () => {
    status = { ...status, server_port: 3030 };
    const client = testClient();
    mount(<DesktopServerSettings />, client);
    expect(await screen.findByRole('spinbutton', { name: 'Server port' })).toHaveValue(3030);
    status = { ...status, server_port: 4040 };
    await act(() => client.refetchQueries({ queryKey: ['desktopServerSettings'] }));
    await waitFor(() => expect(screen.getByRole('spinbutton', { name: 'Server port' })).toHaveValue(4040));
    await userEvent.click(screen.getByRole('radio', { name: 'Automatic' }));
    status = { ...status, server_port: 5050 };
    await act(() => client.refetchQueries({ queryKey: ['desktopServerSettings'] }));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Port 5050 is saved'));
    expect(screen.getByRole('radio', { name: 'Automatic' })).toBeChecked();
    expect(screen.getByRole('button', { name: 'Save port' })).toBeEnabled();
  });

  it('shows a refused port without restarting or hiding the running address', async () => {
    native.savePort.mockRejectedValue(new Error('Port 3030 is already in use.'));
    mount(<DesktopServerSettings />);
    await userEvent.click(await screen.findByRole('radio', { name: 'Fixed port' }));
    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '3030' } });
    await userEvent.click(screen.getByRole('button', { name: 'Save port' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Port 3030 is already in use');
    expect(screen.getByText(/Running at/)).toHaveTextContent('http://127.0.0.1:55001');
    expect(native.restart).not.toHaveBeenCalled();
  });

  it('validates fixed ports before saving', async () => {
    mount(<DesktopServerSettings />);
    await userEvent.click(await screen.findByRole('radio', { name: 'Fixed port' }));
    for (const value of ['0', '1023', '65536', '3000.5', '']) {
      fireEvent.change(screen.getByRole('spinbutton'), { target: { value } });
      fireEvent.submit(screen.getByRole('button', { name: 'Save port' }).closest('form')!);
      expect(await screen.findByRole('alert')).toHaveTextContent('1024 to 65535');
    }
    expect(native.savePort).not.toHaveBeenCalled();
  });

  it('keeps recovery settings available without making HTTP requests to an occupied port', async () => {
    status = { ...status, server_port: 3030, server_url: null, error: '127.0.0.1:3030 is already in use.' };
    const http = vi.spyOn(apiClient, 'getAuthStatus');
    mount(<DesktopServerGate><AuthGate><p>Catalog</p></AuthGate></DesktopServerGate>);
    expect(await screen.findByRole('heading', { name: 'Desktop server settings' })).toBeInTheDocument();
    expect(await screen.findByRole('alert')).toHaveTextContent('already in use');
    expect(http).not.toHaveBeenCalled();
    expect(screen.queryByText('Catalog')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('radio', { name: 'Automatic' }));
    await userEvent.click(screen.getByRole('button', { name: 'Save port' }));
    await waitFor(() => expect(native.savePort).toHaveBeenCalledWith(null, expect.anything()));
  });

  it('can replace unreadable desktop settings with Automatic', async () => {
    status = { ...status, server_url: null, error: 'Could not read desktop.json' };
    mount(<DesktopServerSettings />);
    const section = await screen.findByRole('region', { name: 'Desktop server' });
    await userEvent.click(within(section).getByRole('button', { name: 'Save port' }));
    await waitFor(() => expect(native.savePort).toHaveBeenCalledWith(null, expect.anything()));
  });

  it('waits for the owned server to be ready before mounting the HTTP app', async () => {
    status = { ...status, starting: true, server_url: null };
    const http = vi.spyOn(apiClient, 'getAuthStatus');
    mount(<DesktopServerGate><AuthGate><p>Catalog</p></AuthGate></DesktopServerGate>);
    expect(await screen.findByText('Starting desktop server…')).toBeInTheDocument();
    expect(http).not.toHaveBeenCalled();
    status = { ...status, starting: false, server_url: 'http://127.0.0.1:3030' };
    expect(await screen.findByText('Catalog')).toBeInTheDocument();
    expect(http).toHaveBeenCalledOnce();
  });

  it('does not ask for desktop settings in a browser', () => {
    native.isTauriApp.mockReturnValue(false);
    mount(<DesktopServerGate><p>Browser catalog</p></DesktopServerGate>);
    expect(screen.getByText('Browser catalog')).toBeInTheDocument();
    expect(native.getSettings).not.toHaveBeenCalled();
  });
});
