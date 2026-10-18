import { useCallback, useEffect, useRef, useState } from 'react';
import { isTauriApp } from '../utils/tauri';

export const UI_ZOOM_KEY = 'psf-guard.ui-zoom-percent';
const MIN_ZOOM = 50;
const MAX_ZOOM = 200;

function savedZoom(): number {
  try {
    const value = Number(localStorage.getItem(UI_ZOOM_KEY));
    return Number.isInteger(value) && value >= MIN_ZOOM && value <= MAX_ZOOM ? value : 100;
  } catch { return 100; }
}

export interface DesktopZoom {
  percent: number;
  busy: boolean;
  zoomIn: () => void;
  zoomOut: () => void;
  reset: () => void;
}

/** Whole-window zoom belongs to this desktop installation, not an account. */
export function useDesktopZoom(): { controls?: DesktopZoom; error: string } {
  const enabled = isTauriApp();
  const [percent, setPercent] = useState(savedZoom);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const requested = useRef(percent);
  const applied = useRef(100);
  const queue = useRef(Promise.resolve());
  const pending = useRef(0);

  const apply = useCallback((next: number, persist = true) => {
    requested.current = next;
    pending.current += 1;
    setBusy(true);
    // Serialize the native calls so a slow response cannot restore an older scale.
    queue.current = queue.current.then(async () => {
      try {
        const { getCurrentWebview } = await import('@tauri-apps/api/webview');
        await getCurrentWebview().setZoom(next / 100);
        applied.current = next;
        setPercent(next);
        setError('');
        if (persist) {
          try { localStorage.setItem(UI_ZOOM_KEY, String(next)); }
          catch { setError('UI zoom changed, but PSF Guard could not save it on this device.'); }
        }
      } catch {
        if (requested.current === next) requested.current = applied.current;
        setPercent(applied.current);
        setError('PSF Guard could not change the UI zoom. Try again from the menu.');
      } finally {
        pending.current -= 1;
        setBusy(pending.current > 0);
      }
    });
  }, []);

  const zoomIn = useCallback(() => apply(Math.min(MAX_ZOOM, requested.current + 10)), [apply]);
  const zoomOut = useCallback(() => apply(Math.max(MIN_ZOOM, requested.current - 10)), [apply]);
  const reset = useCallback(() => apply(100), [apply]);

  useEffect(() => {
    if (!enabled) return;
    apply(savedZoom(), false);
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey || event.isComposing) return;
      const action = event.key === '+' || event.key === '=' ? zoomIn
        : event.key === '-' ? zoomOut : event.key === '0' ? reset : null;
      if (!action) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      action();
    };
    // Capture before dialogs and image shortcuts, including when an input has focus.
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [enabled, apply, zoomIn, zoomOut, reset]);

  return { controls: enabled ? { percent, busy, zoomIn, zoomOut, reset } : undefined, error };
}
