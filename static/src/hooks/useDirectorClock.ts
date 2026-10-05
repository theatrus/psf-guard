import { useSyncExternalStore } from 'react';

// One clock for all live surfaces, even while their network query is offline.
let now = Date.now();
let timer: ReturnType<typeof setInterval> | undefined;
const listeners = new Set<() => void>();
const snapshot = () => now;
function subscribe(listener: () => void) {
  listeners.add(listener);
  if (!timer) {
    now = Date.now();
    timer = setInterval(() => { now = Date.now(); listeners.forEach(notify => notify()); }, 5000);
  }
  return () => {
    listeners.delete(listener);
    if (!listeners.size) { clearInterval(timer); timer = undefined; }
  };
}

export function useDirectorClock() {
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}
