import { useSyncExternalStore } from 'react';

/**
 * Card display choices shared by every surface that renders an ImageCard.
 * One preference, not one per view: the grid and the Sequence view must
 * never disagree about what a card shows.
 */
export interface DisplayPreferences {
  /** Show the smaller labeled chip with the other score basis ("night" or
   * "all") when it disagrees with the main badge. */
  showSecondaryScore: boolean;
}

const STORAGE_KEY = 'psf-guard.display-preferences';
const DEFAULTS: DisplayPreferences = { showSecondaryScore: true };

type Listener = () => void;

const listeners = new Set<Listener>();

function sanitize(parsed: Partial<DisplayPreferences>): DisplayPreferences {
  return {
    showSecondaryScore:
      typeof parsed.showSecondaryScore === 'boolean'
        ? parsed.showSecondaryScore
        : DEFAULTS.showSecondaryScore,
  };
}

function readStored(): DisplayPreferences {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    return sanitize(JSON.parse(raw) as Partial<DisplayPreferences>);
  } catch {
    return { ...DEFAULTS };
  }
}

let current = readStored();

// A second tab writes the same localStorage key: adopt its value so two
// windows show the same cards the same way.
if (typeof window !== 'undefined') {
  window.addEventListener('storage', (event) => {
    if (event.key !== STORAGE_KEY) return;
    current = readStored();
    listeners.forEach((listener) => listener());
  });
}

export function setDisplayPreferences(next: DisplayPreferences): void {
  current = sanitize(next);
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(current));
  } catch {
    // Keep the in-memory preference even when it cannot be persisted.
  }
  listeners.forEach((listener) => listener());
}

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function snapshot(): DisplayPreferences {
  return current;
}

/** Subscribe to the shared preference. Returns the current snapshot. */
export function useDisplayPreferences(): DisplayPreferences {
  return useSyncExternalStore(subscribe, snapshot);
}
