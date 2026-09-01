import { useSyncExternalStore } from 'react';
import type { PenaltyScaleParams } from '../api/types';

/**
 * How hard each kind of event evidence hits the quality score, shared
 * across every scoring surface (Sequence view, grid badges, detail panel).
 *
 * Each value multiplies that evidence's built-in penalty: 0 ignores it,
 * 1 (the default) keeps the calibrated behavior, 2 doubles the hit. Kept
 * as one shared preference so a satellite trail cannot cost a frame its
 * badge in one view and nothing in another.
 */
export interface ScoringPreferences {
  satellite: number;
  pointing: number;
  temporal: number;
  /** Absolute HFR ceiling: recommend rejecting frames whose measured HFR
   * exceeds this. `null` (the default) turns the check off. */
  hfrRejectAbove: number | null;
  /** Absolute star-count floor: recommend rejecting frames with fewer
   * measured stars. `null` (the default) turns the check off. */
  starCountRejectBelow: number | null;
}

const STORAGE_KEY = 'psf-guard.penalty-scales';

/** The calibrated defaults. The single source for the store's fallback,
 * the control's Reset button, and its "changed" indicator. */
export const SCORING_DEFAULTS: ScoringPreferences = {
  satellite: 1,
  pointing: 1,
  temporal: 1,
  hfrRejectAbove: null,
  starCountRejectBelow: null,
};
const DEFAULTS = SCORING_DEFAULTS;

type Listener = () => void;

const listeners = new Set<Listener>();

function sanitize(value: unknown): number {
  const scale = typeof value === 'number' && Number.isFinite(value) ? value : 1;
  return Math.min(2, Math.max(0, scale));
}

/** A reject limit is any positive finite number; everything else is "off". */
function sanitizeLimit(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null;
}

function normalize(parsed: Partial<ScoringPreferences>): ScoringPreferences {
  return {
    satellite: sanitize(parsed.satellite),
    pointing: sanitize(parsed.pointing),
    temporal: sanitize(parsed.temporal),
    hfrRejectAbove: sanitizeLimit(parsed.hfrRejectAbove),
    starCountRejectBelow: sanitizeLimit(parsed.starCountRejectBelow),
  };
}

function readStored(): ScoringPreferences {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    return normalize(JSON.parse(raw) as Partial<ScoringPreferences>);
  } catch {
    return { ...DEFAULTS };
  }
}

let current = readStored();

// A second tab writes the same localStorage key: adopt its value so two
// windows cannot score the same frame differently.
if (typeof window !== 'undefined') {
  window.addEventListener('storage', (event) => {
    if (event.key !== STORAGE_KEY) return;
    current = readStored();
    listeners.forEach((listener) => listener());
  });
}

export function setScoringPreferences(next: ScoringPreferences): void {
  current = normalize(next);
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(current));
  } catch {
    // Keep the in-memory preference even when it cannot be persisted.
  }
  listeners.forEach((listener) => listener());
}

export function scoringPreferences(): ScoringPreferences {
  return current;
}

/** A preference snapshot as request query params; defaults are omitted so
 * the server's calibrated behavior needs no parameters at all. Pure — pass
 * the same snapshot used for the query key so key and payload agree. */
export function penaltyParamsOf(preferences: ScoringPreferences): PenaltyScaleParams {
  const params: PenaltyScaleParams = {};
  if (preferences.satellite !== 1) params.penalty_satellite = preferences.satellite;
  if (preferences.pointing !== 1) params.penalty_pointing = preferences.pointing;
  if (preferences.temporal !== 1) params.penalty_temporal = preferences.temporal;
  if (preferences.hfrRejectAbove != null) params.hfr_reject_above = preferences.hfrRejectAbove;
  if (preferences.starCountRejectBelow != null) {
    params.star_count_reject_below = preferences.starCountRejectBelow;
  }
  return params;
}

/** The same snapshot as react-query key segments. Every scoring query key
 * spreads this, so a preference change refetches every surface. */
export function penaltyKeyOf(preferences: ScoringPreferences): (number | null)[] {
  return [
    preferences.satellite,
    preferences.pointing,
    preferences.temporal,
    preferences.hfrRejectAbove,
    preferences.starCountRejectBelow,
  ];
}

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function snapshot(): ScoringPreferences {
  return current;
}

/** Subscribe to the shared preference. Returns the current snapshot. */
export function useScoringPreferences(): ScoringPreferences {
  return useSyncExternalStore(subscribe, snapshot);
}
