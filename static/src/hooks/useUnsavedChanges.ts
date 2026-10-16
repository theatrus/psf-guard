import { useEffect, useRef, useSyncExternalStore } from 'react';

/*
 * Forms with edits their Save has not sent yet. A form registers while it
 * holds some (useUnsavedChanges); Settings asks before it closes or reloads
 * while any are registered, and the browser asks before the page goes.
 */

const pending = new Map<symbol, string>();
const listeners = new Set<() => void>();
let snapshot: readonly string[] = [];

function changed() {
  snapshot = [...pending.values()];
  listeners.forEach((listener) => listener());
}

function warnBeforeUnload(event: BeforeUnloadEvent) {
  event.preventDefault();
  // Older browsers show the prompt only when this is set.
  event.returnValue = '';
}

function setPending(key: symbol, label: string | null) {
  const before = pending.size;
  if (label === null) pending.delete(key);
  else pending.set(key, label);
  if (before === 0 && pending.size > 0) window.addEventListener('beforeunload', warnBeforeUnload);
  if (before > 0 && pending.size === 0) window.removeEventListener('beforeunload', warnBeforeUnload);
  changed();
}

/** Register `label` ("C925 setup") while `dirty`: its form holds edits its
 *  Save has not sent. Leaving the page, or unmounting, takes it off. */
export function useUnsavedChanges(label: string, dirty: boolean): void {
  const key = useRef<symbol>(Symbol(label)).current;
  useEffect(() => {
    setPending(key, dirty ? label : null);
  }, [dirty, key, label]);
  useEffect(() => () => setPending(key, null), [key]);
}

/** Every form with unsaved edits, by label, in the order they began. */
export function useUnsavedLabels(): readonly string[] {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    () => snapshot,
  );
}

/** The labels registered now, outside React. */
export function unsavedLabels(): readonly string[] {
  return snapshot;
}

/** "C925 setup", "C925 setup and Folsom", "A, B and C". */
export function describeUnsaved(labels: readonly string[]): string {
  if (labels.length <= 1) return labels[0] ?? '';
  return `${labels.slice(0, -1).join(', ')} and ${labels[labels.length - 1]}`;
}
