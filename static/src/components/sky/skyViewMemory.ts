import type { SkyFrame, SkyMode } from '../../utils/skyProjection';

/**
 * Where the Sky view was left: frame, shape, turn, zoom, and the cuts. Kept
 * for the browser session so leaving for Images and coming back lands on
 * the same sky, and a reload in the same tab does too. Nothing here is
 * scope, so it stays out of the URL that the other views carry along.
 */
export interface SkyViewMemory {
  frame: SkyFrame;
  mode: SkyMode;
  centerLon: number;
  centerLat: number;
  zoom: number;
  acceptedOnly: boolean;
  showBackdrop: boolean;
  showStacks: boolean;
  hiddenRigs: string[];
  hiddenFilters: string[];
  fromNight: string | null;
  asOfNight: string | null;
}

const KEY = 'psf-guard:sky-view:v1';

let held: Partial<SkyViewMemory> | null = null;

function storage(): Storage | null {
  try {
    return typeof window !== 'undefined' ? window.sessionStorage : null;
  } catch {
    return null;
  }
}

/** What was remembered, if anything. Fields missing or unreadable are left out. */
export function recallSkyView(): Partial<SkyViewMemory> {
  if (held) return held;
  try {
    const raw = storage()?.getItem(KEY);
    const parsed = raw ? (JSON.parse(raw) as Partial<SkyViewMemory>) : {};
    held = typeof parsed === 'object' && parsed ? parsed : {};
  } catch {
    held = {};
  }
  return held;
}

/** Merge a change into what is remembered. */
export function rememberSkyView(patch: Partial<SkyViewMemory>): void {
  held = { ...recallSkyView(), ...patch };
  try {
    storage()?.setItem(KEY, JSON.stringify(held));
  } catch {
    // Storage may be full or refused; the in-memory copy still serves this session.
  }
}

/** Forget the remembered view. */
export function forgetSkyView(): void {
  held = {};
  try {
    storage()?.removeItem(KEY);
  } catch {
    // Nothing to do.
  }
}
