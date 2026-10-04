import { useEffect, useState } from 'react';

/**
 * Whether the sky overlay is drawn over a solved frame: the viewer's last
 * explicit choice, remembered across frames and reloads.
 *
 * A solution being present says nothing about whether someone wants to see
 * it. Background quality analysis solves frames on its own, so following
 * the data would switch the overlay on for frames nobody asked about. Off
 * until the viewer turns it on.
 */
const STORAGE_KEY = 'psf-guard.sky-overlay';

type Listener = (enabled: boolean) => void;

const listeners = new Set<Listener>();

function readStored(): boolean {
  try {
    return window.localStorage.getItem(STORAGE_KEY) === 'true';
  } catch {
    // Storage can be refused (private browsing); the overlay then starts off.
    return false;
  }
}

let enabled = readStored();

export function setSkyOverlay(next: boolean): void {
  if (next === enabled) return;
  enabled = next;
  try {
    window.localStorage.setItem(STORAGE_KEY, String(next));
  } catch {
    // Keep the in-memory choice even when it cannot be persisted.
  }
  listeners.forEach((listener) => listener(next));
}

export function skyOverlayEnabled(): boolean {
  return enabled;
}

/** Subscribe to the shared choice. Returns its current value. */
export function useSkyOverlay(): boolean {
  const [value, setValue] = useState(enabled);
  useEffect(() => {
    listeners.add(setValue);
    setValue(enabled);
    return () => {
      listeners.delete(setValue);
    };
  }, []);
  return value;
}
