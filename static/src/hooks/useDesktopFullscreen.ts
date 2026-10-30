import { useEffect } from 'react';
import { isTauriApp } from '../utils/tauri';

/** Whether this page runs on macOS, which brings its own full-screen controls. */
function onMac(): boolean {
  return /Mac/.test(navigator.userAgent);
}

/**
 * F11 puts the desktop app's window into full screen and back, as other
 * Windows and Linux programs do. macOS already has View › Enter Full Screen,
 * ⌃⌘F and the window's green button from the app's standard menu, and F11
 * there belongs to the system. A browser has its own full screen, so this
 * does nothing outside the desktop app.
 */
export function useDesktopFullscreen(): void {
  useEffect(() => {
    if (!isTauriApp() || onMac()) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'F11' || event.repeat) return;
      if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
      event.preventDefault();
      void (async () => {
        try {
          const { getCurrentWindow } = await import('@tauri-apps/api/window');
          const current = getCurrentWindow();
          await current.setFullscreen(!(await current.isFullscreen()));
        } catch (error) {
          console.error('PSF Guard could not change full screen:', error);
        }
      })();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, []);
}
