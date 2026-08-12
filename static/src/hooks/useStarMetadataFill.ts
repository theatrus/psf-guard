import { useEffect, useState } from 'react';

/**
 * Whether quality analysis writes its measured star count and HFR into
 * imported images' database metadata, shared across every analyze action.
 *
 * The quality scan, the database-wide backfill, and the import-time analyze
 * option all take the same choice, so a preference held per component would
 * let one action silently disagree with another. The fill only adds missing
 * keys — values a N.I.N.A. catalog carries are never replaced — so it
 * defaults on; only an explicit opt-out turns it off.
 */
const STORAGE_KEY = 'psf-guard.fill-star-metadata';

type Listener = (enabled: boolean) => void;

const listeners = new Set<Listener>();

function readStored(): boolean {
  try {
    return window.localStorage.getItem(STORAGE_KEY) !== 'false';
  } catch {
    // Private browsing and similar can refuse storage. The preference is a
    // convenience, not something worth failing a render over.
    return true;
  }
}

let enabled = readStored();

export function setStarMetadataFill(next: boolean): void {
  if (next === enabled) return;
  enabled = next;
  try {
    window.localStorage.setItem(STORAGE_KEY, String(next));
  } catch {
    // Keep the in-memory preference even when it cannot be persisted.
  }
  listeners.forEach((listener) => listener(next));
}

export function starMetadataFillEnabled(): boolean {
  return enabled;
}

/** Subscribe to the shared preference. Returns its current value. */
export function useStarMetadataFill(): boolean {
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
