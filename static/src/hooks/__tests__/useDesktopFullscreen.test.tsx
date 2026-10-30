import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useDesktopFullscreen } from '../useDesktopFullscreen';

const { isTauriApp, isFullscreen, setFullscreen } = vi.hoisted(() => ({
  isTauriApp: vi.fn(),
  isFullscreen: vi.fn(),
  setFullscreen: vi.fn(),
}));
vi.mock('../../utils/tauri', () => ({ isTauriApp }));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ isFullscreen, setFullscreen }),
}));

const windowsAgent = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/130.0';
const macAgent = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15';

function pressF11(options: KeyboardEventInit = {}) {
  const event = new KeyboardEvent('keydown', { key: 'F11', cancelable: true, ...options });
  window.dispatchEvent(event);
  return event;
}

beforeEach(() => {
  isTauriApp.mockReturnValue(true);
  isFullscreen.mockReset().mockResolvedValue(false);
  setFullscreen.mockReset().mockResolvedValue(undefined);
  vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue(windowsAgent);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('desktop full screen', () => {
  it('turns full screen on and off with F11 on Windows and Linux', async () => {
    renderHook(useDesktopFullscreen);
    expect(pressF11().defaultPrevented).toBe(true);
    await waitFor(() => expect(setFullscreen).toHaveBeenLastCalledWith(true));
    isFullscreen.mockResolvedValue(true);
    pressF11();
    await waitFor(() => expect(setFullscreen).toHaveBeenLastCalledWith(false));
  });

  it('leaves F11 with a modifier, and other keys, alone', () => {
    renderHook(useDesktopFullscreen);
    expect(pressF11({ ctrlKey: true }).defaultPrevented).toBe(false);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'F10' }));
    expect(isFullscreen).not.toHaveBeenCalled();
  });

  it('leaves full screen to macOS’s own controls, and to the browser', () => {
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue(macAgent);
    const mac = renderHook(useDesktopFullscreen);
    expect(pressF11().defaultPrevented).toBe(false);
    mac.unmount();

    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue(windowsAgent);
    isTauriApp.mockReturnValue(false);
    renderHook(useDesktopFullscreen);
    expect(pressF11().defaultPrevented).toBe(false);
    expect(isFullscreen).not.toHaveBeenCalled();
  });
});
