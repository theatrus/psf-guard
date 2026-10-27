import { createContext, useContext, useEffect, useMemo, useRef, useSyncExternalStore } from 'react';

/*
 * Forms register unsaved edits with useUnsavedChanges. Settings asks before
 * closing, restart controls stay disabled, and the browser asks before
 * unloading the page while forms have unsaved edits.
 */

const pending = new Map<symbol, string>();
/** A settings page change must not discard or warn about forms behind the modal. */
export const UnsavedScopeContext = createContext<Set<symbol> | null>(null);
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
  const scope = useContext(UnsavedScopeContext);
  useEffect(() => {
    scope?.add(key);
    return () => { scope?.delete(key); };
  }, [key, scope]);
  useEffect(() => {
    setPending(key, dirty ? label : null);
  }, [dirty, key, label, scope]);
  useEffect(() => () => setPending(key, null), [key]);
}

/** Unsaved forms in registration order, optionally limited to one scope. */
export function useUnsavedLabels(scope?: ReadonlySet<symbol>): readonly string[] {
  const labels = useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    () => snapshot,
  );
  return useMemo(() => scope
    ? [...pending].filter(([key]) => scope.has(key)).map(([, label]) => label)
    : labels, [labels, scope]);
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
