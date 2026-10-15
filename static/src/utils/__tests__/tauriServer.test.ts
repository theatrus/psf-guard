import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { desktopServer, getServerUrl, tauriConfig } from '../tauri';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
beforeEach(() => { vi.stubGlobal('__TAURI_INTERNALS__', {}); });
afterEach(() => { vi.unstubAllGlobals(); });

describe('desktop server bridge', () => {
  it('propagates a server failure instead of guessing another process URL', async () => {
    invoke.mockRejectedValue('The chosen port is occupied');
    await expect(getServerUrl()).rejects.toThrow('The chosen port is occupied');
  });

  it('returns the actual server URL and passes fixed and automatic preferences to native code', async () => {
    invoke.mockResolvedValue('http://127.0.0.1:3030');
    await expect(getServerUrl()).resolves.toBe('http://127.0.0.1:3030');
    await desktopServer.savePort(3030);
    expect(invoke).toHaveBeenLastCalledWith('set_desktop_server_port', { port: 3030 });
    await desktopServer.savePort(null);
    expect(invoke).toHaveBeenLastCalledWith('set_desktop_server_port', { port: null });
  });

  it('preserves the restart error so Settings can show the conflicting port', async () => {
    invoke.mockRejectedValue('127.0.0.1:3030 is in use');
    await expect(tauriConfig.restartServer()).rejects.toThrow('127.0.0.1:3030 is in use');
  });
});
