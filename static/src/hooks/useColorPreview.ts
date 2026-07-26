import { useEffect, useState } from 'react';

/**
 * Whether one-shot-color frames are shown in colour, shared across every view.
 *
 * The grid, the overview, and the detail view all render the same exposures,
 * so a preference held per component would let them disagree — the frame you
 * clicked would change appearance on the way in. This keeps one answer, and
 * remembers it, because it describes how someone likes to look at their data
 * rather than anything about a particular image.
 *
 * Mono frames are unaffected either way: with no `BAYERPAT` there is no
 * colour to recover.
 */
const STORAGE_KEY = 'psf-guard.color-preview';

type Listener = (enabled: boolean) => void;

const listeners = new Set<Listener>();

function readStored(): boolean {
  try {
    // Default on: a colour camera's frame should look like what it recorded.
    // Only an explicit opt-out turns it off.
    return window.localStorage.getItem(STORAGE_KEY) !== 'false';
  } catch {
    // Private browsing and similar can refuse storage. The preference is a
    // convenience, not something worth failing a render over.
    return true;
  }
}

let enabled = readStored();

export function setColorPreview(next: boolean): void {
  if (next === enabled) return;
  enabled = next;
  try {
    window.localStorage.setItem(STORAGE_KEY, String(next));
  } catch {
    // Keep the in-memory preference even when it cannot be persisted.
  }
  listeners.forEach((listener) => listener(next));
}

export function colorPreviewEnabled(): boolean {
  return enabled;
}

/** Subscribe to the shared preference. Returns its current value. */
export function useColorPreview(): boolean {
  const [value, setValue] = useState(enabled);
  useEffect(() => {
    listeners.add(setValue);
    // A change between the initial render and this effect would otherwise be
    // missed.
    setValue(enabled);
    return () => {
      listeners.delete(setValue);
    };
  }, []);
  return value;
}
