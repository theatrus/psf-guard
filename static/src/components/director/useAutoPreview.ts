import { useCallback, useEffect, useRef, useState } from 'react';

/** How long inputs must rest before a preview runs again. */
export const PREVIEW_DEBOUNCE_MS = 300;

export interface AutoPreview<T> {
  /** The last preview, or an applied result set in its place. It may be for
   *  older inputs while a new one runs: see `fresh`. */
  result: T | null;
  /** `result` is for the current inputs and nothing newer is on its way. */
  fresh: boolean;
  /** The last request for the current inputs failed. */
  error: unknown;
  /** A request is in flight or waiting out the debounce. */
  loading: boolean;
  /** Run again now for the current inputs: a retry, or after something the
   *  key cannot see changed (the rig rows under a refused send). */
  refresh: () => void;
  /** Put a result in place for the current inputs, as an applied report. */
  replace: (result: T) => void;
}

interface Settled<T> { result: T | null; error: unknown; key: string | null }

/** A preview that keeps itself current. It runs at once when enabled, then
 *  again `delay` ms after `key` stops changing. At most one request is in
 *  flight: a change while it runs waits for it and then runs once more, and
 *  an answer for inputs that have since changed is dropped. Nothing polls;
 *  only a new key or `refresh` asks again. */
export function useAutoPreview<T>({ key, enabled, fetch, delay = PREVIEW_DEBOUNCE_MS }: {
  /** Everything the preview reads that this page can see change. */
  key: string;
  /** Off while the inputs are unknown or a preview would mislead. */
  enabled: boolean;
  fetch: () => Promise<T>;
  delay?: number;
}): AutoPreview<T> {
  const [settled, setSettledState] = useState<Settled<T>>({ result: null, error: null, key: null });
  const [running, setRunning] = useState(false);
  const [waiting, setWaiting] = useState(false);
  const latest = useRef({ key, enabled, fetch });
  latest.current = { key, enabled, fetch };
  const settledKey = useRef<string | null>(null);
  const setSettled = useCallback((next: Settled<T>) => { settledKey.current = next.key; setSettledState(next); }, []);
  const inFlight = useRef<string | null>(null);
  const again = useRef(false);
  const ran = useRef(false);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  const start = useCallback((force: boolean) => {
    const { key: asked, enabled: on, fetch: run } = latest.current;
    if (!on) return;
    if (inFlight.current !== null) {
      // The running request answers for these inputs unless forced.
      if (force || inFlight.current !== asked) again.current = true;
      return;
    }
    if (!force && settledKey.current === asked) return;
    inFlight.current = asked;
    again.current = false;
    ran.current = true;
    setRunning(true);
    const settle = (next: { result: T | null; error: unknown }) => {
      inFlight.current = null;
      if (!alive.current) return;
      // The inputs moved on while this ran: its answer is for the old ones.
      const stale = asked !== latest.current.key;
      if (!stale) setSettled({ ...next, key: asked });
      const forced = again.current;
      if ((stale || forced) && latest.current.enabled) start(forced);
      else setRunning(false);
    };
    run().then(result => settle({ result, error: null }), (error: unknown) => settle({ result: null, error: error ?? new Error('Preview failed') }));
  }, [setSettled]);

  useEffect(() => {
    // Nothing to do while off, or when the answer for these inputs is in.
    if (!enabled || settledKey.current === key) { setWaiting(false); return; }
    // Opening previews at once; a change waits for the inputs to rest.
    const wait = ran.current ? delay : 0;
    setWaiting(true);
    const timer = window.setTimeout(() => { setWaiting(false); start(false); }, wait);
    return () => window.clearTimeout(timer);
  }, [key, enabled, delay, start]);

  const refresh = useCallback(() => start(true), [start]);
  const replace = useCallback((result: T) => setSettled({ result, error: null, key: latest.current.key }), [setSettled]);
  const current = settled.key === key;
  const loading = enabled && (running || waiting);
  return {
    result: settled.result,
    fresh: current && !loading && settled.result !== null,
    error: current ? settled.error : null,
    loading,
    refresh,
    replace,
  };
}
