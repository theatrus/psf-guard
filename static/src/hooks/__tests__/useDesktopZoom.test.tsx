import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { UI_ZOOM_KEY, useDesktopZoom } from '../useDesktopZoom';

const { isTauriApp, setZoom } = vi.hoisted(() => ({ isTauriApp: vi.fn(), setZoom: vi.fn() }));
vi.mock('../../utils/tauri', () => ({ isTauriApp }));
vi.mock('@tauri-apps/api/webview', () => ({ getCurrentWebview: () => ({ setZoom }) }));

beforeEach(() => {
  localStorage.clear();
  isTauriApp.mockReturnValue(true);
  setZoom.mockReset().mockResolvedValue(undefined);
});

async function desktop() {
  const hook = renderHook(useDesktopZoom);
  await waitFor(() => expect(hook.result.current.controls?.busy).toBe(false));
  return hook;
}

describe('desktop UI zoom', () => {
  it('restores the saved scale and saves changes and reset across mounts', async () => {
    localStorage.setItem(UI_ZOOM_KEY, '130');
    const hook = await desktop();
    expect(setZoom).toHaveBeenLastCalledWith(1.3);
    act(() => hook.result.current.controls!.zoomIn());
    await waitFor(() => expect(hook.result.current.controls?.percent).toBe(140));
    expect(localStorage.getItem(UI_ZOOM_KEY)).toBe('140');
    hook.unmount();
    const next = await desktop();
    expect(setZoom).toHaveBeenLastCalledWith(1.4);
    act(() => next.result.current.controls!.reset());
    await waitFor(() => expect(next.result.current.controls?.percent).toBe(100));
    expect(localStorage.getItem(UI_ZOOM_KEY)).toBe('100');
  });

  it.each(['broken', '0', '49', '201', '125.5'])('ignores invalid saved zoom %s', async value => {
    localStorage.setItem(UI_ZOOM_KEY, value);
    const hook = await desktop();
    expect(hook.result.current.controls?.percent).toBe(100);
    expect(setZoom).toHaveBeenLastCalledWith(1);
  });

  it('queues rapid shortcuts in order and stops them reaching image controls', async () => {
    const hook = await desktop();
    const imageKey = vi.fn();
    window.addEventListener('keydown', imageKey);
    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: '+', metaKey: true, cancelable: true }));
      window.dispatchEvent(new KeyboardEvent('keydown', { key: '=', ctrlKey: true, cancelable: true }));
      window.dispatchEvent(new KeyboardEvent('keydown', { key: '-', metaKey: true, cancelable: true }));
    });
    await waitFor(() => expect(hook.result.current.controls?.busy).toBe(false));
    expect(setZoom.mock.calls.map(([value]) => value)).toEqual([1, 1.1, 1.2, 1.1]);
    expect(imageKey).not.toHaveBeenCalled();
    act(() => window.dispatchEvent(new KeyboardEvent('keydown', { key: '0', ctrlKey: true })));
    await waitFor(() => expect(hook.result.current.controls?.percent).toBe(100));
    window.removeEventListener('keydown', imageKey);
  });

  it('clamps the scale, and leaves unmodified image keys and other shortcuts alone', async () => {
    const hook = await desktop();
    act(() => { for (let i = 0; i < 30; i += 1) hook.result.current.controls!.zoomOut(); });
    await waitFor(() => expect(hook.result.current.controls?.busy).toBe(false));
    expect(hook.result.current.controls?.percent).toBe(50);
    act(() => { for (let i = 0; i < 30; i += 1) hook.result.current.controls!.zoomIn(); });
    await waitFor(() => expect(hook.result.current.controls?.busy).toBe(false));
    expect(hook.result.current.controls?.percent).toBe(200);
    const before = setZoom.mock.calls.length;
    for (const options of [{ key: '+' }, { key: '0' }, { key: 'a', metaKey: true }, { key: '+', ctrlKey: true, altKey: true }]) {
      const event = new KeyboardEvent('keydown', { ...options, cancelable: true });
      window.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
    }
    expect(setZoom).toHaveBeenCalledTimes(before);
  });

  it('keeps the applied scale and saved choice when native zoom fails, then allows retry', async () => {
    const hook = await desktop();
    setZoom.mockRejectedValueOnce(new Error('denied'));
    act(() => hook.result.current.controls!.zoomIn());
    await waitFor(() => expect(hook.result.current.error).toMatch(/could not change/));
    expect(hook.result.current.controls?.percent).toBe(100);
    expect(localStorage.getItem(UI_ZOOM_KEY)).toBeNull();
    act(() => hook.result.current.controls!.zoomIn());
    await waitFor(() => expect(hook.result.current.controls?.percent).toBe(110));
    expect(hook.result.current.error).toBe('');
  });

  it('reports a storage failure without undoing a successful zoom', async () => {
    const hook = await desktop();
    const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('blocked'); });
    act(() => hook.result.current.controls!.zoomIn());
    await waitFor(() => expect(hook.result.current.error).toMatch(/could not save/));
    expect(hook.result.current.controls?.percent).toBe(110);
    write.mockRestore();
  });

  it('does not call Tauri or intercept browser zoom outside the desktop app', () => {
    isTauriApp.mockReturnValue(false);
    const hook = renderHook(useDesktopZoom);
    expect(hook.result.current.controls).toBeUndefined();
    const event = new KeyboardEvent('keydown', { key: '+', ctrlKey: true, cancelable: true });
    window.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(setZoom).not.toHaveBeenCalled();
  });
});
