import { useEffect, useState } from 'react';
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
}

const STORAGE_KEY = 'psf-guard.penalty-scales';
const DEFAULTS: ScoringPreferences = { satellite: 1, pointing: 1, temporal: 1 };

type Listener = (preferences: ScoringPreferences) => void;

const listeners = new Set<Listener>();

function sanitize(value: unknown): number {
  const scale = typeof value === 'number' && Number.isFinite(value) ? value : 1;
  return Math.min(2, Math.max(0, scale));
}

function readStored(): ScoringPreferences {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    const parsed = JSON.parse(raw) as Partial<ScoringPreferences>;
    return {
      satellite: sanitize(parsed.satellite),
      pointing: sanitize(parsed.pointing),
      temporal: sanitize(parsed.temporal),
    };
  } catch {
    return { ...DEFAULTS };
  }
}

let current = readStored();

export function setScoringPreferences(next: ScoringPreferences): void {
  current = {
    satellite: sanitize(next.satellite),
    pointing: sanitize(next.pointing),
    temporal: sanitize(next.temporal),
  };
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(current));
  } catch {
    // Keep the in-memory preference even when it cannot be persisted.
  }
  listeners.forEach((listener) => listener(current));
}

export function scoringPreferences(): ScoringPreferences {
  return current;
}

/** The preference as request query params; defaults are omitted so the
 * server's calibrated behavior needs no parameters at all. */
export function scoringPenaltyParams(): PenaltyScaleParams {
  const params: PenaltyScaleParams = {};
  if (current.satellite !== 1) params.penalty_satellite = current.satellite;
  if (current.pointing !== 1) params.penalty_pointing = current.pointing;
  if (current.temporal !== 1) params.penalty_temporal = current.temporal;
  return params;
}

/** Subscribe to the shared preference. Returns its current value. */
export function useScoringPreferences(): ScoringPreferences {
  const [value, setValue] = useState(current);
  useEffect(() => {
    listeners.add(setValue);
    setValue(current);
    return () => {
      listeners.delete(setValue);
    };
  }, []);
  return value;
}
