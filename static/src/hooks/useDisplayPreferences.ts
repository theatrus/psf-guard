import { useSyncExternalStore } from 'react';

/**
 * Card display choices shared by every surface that renders an ImageCard.
 * One preference, not one per view: the grid and the Sequence view must
 * never disagree about what a card shows.
 */
export interface DisplayPreferences {
  /** Show the smaller "night <score>" chip — the per-session basis shown
   * when the main badge is the all-sessions score. */
  showNightChip: boolean;
  /** Show the smaller "all <score>" chip — the all-sessions basis shown
   * when the main badge is a single session's score. */
  showAllChip: boolean;
}

const STORAGE_KEY = 'psf-guard.display-preferences';
const DEFAULTS: DisplayPreferences = { showNightChip: true, showAllChip: true };

type Listener = () => void;

const listeners = new Set<Listener>();

interface StoredPreferences extends Partial<DisplayPreferences> {
  /** Earlier builds stored one switch for both chip types. */
  showSecondaryScore?: boolean;
}

function sanitize(parsed: StoredPreferences): DisplayPreferences {
  const legacy =
    typeof parsed.showSecondaryScore === 'boolean'
      ? parsed.showSecondaryScore
      : undefined;
  return {
    showNightChip:
      typeof parsed.showNightChip === 'boolean'
        ? parsed.showNightChip
        : legacy ?? DEFAULTS.showNightChip,
    showAllChip:
      typeof parsed.showAllChip === 'boolean'
        ? parsed.showAllChip
        : legacy ?? DEFAULTS.showAllChip,
  };
}

function readStored(): DisplayPreferences {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    return sanitize(JSON.parse(raw) as StoredPreferences);
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
