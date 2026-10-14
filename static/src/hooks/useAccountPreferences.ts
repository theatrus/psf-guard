import { useEffect } from 'react';
import { apiClient } from '../api/client';
import {
  adoptAccountPreferences,
  claimBrowserPreferences,
  claimBrowserScoring,
  getDisplayPreferences,
  retireBrowserPreferences,
  setPreferencesAccount,
  subscribeLocalPreferenceChanges,
  type AccountPreferenceKey,
} from './useDisplayPreferences';

/** How long after the last change the preferences are saved. */
export const SAVE_DELAY_MS = 800;
/** The first wait before reading or saving again after a failure; each
 *  further failure doubles it, up to RETRY_MAX_MS. */
export const RETRY_FIRST_MS = 1_000;
export const RETRY_MAX_MS = 60_000;

/** The wait before attempt `failures + 1`. */
export function retryDelay(failures: number): number {
  return Math.min(RETRY_MAX_MS, RETRY_FIRST_MS * 2 ** Math.max(0, failures - 1));
}

/** Whether a failed request is worth sending again: no answer, a server
 *  fault, or a request to slow down. A refusal, such as no user signed in,
 *  is not. */
function worthRetrying(error: unknown): boolean {
  const status = (error as { response?: { status?: number } } | null)?.response?.status;
  return status === undefined || status >= 500 || status === 408 || status === 429;
}

/**
 * Keep the display preferences in the signed-in user's document on the
 * server, so the View panel's choices follow them to another browser.
 *
 * - On sign-in, or when the app opens signed in, the user's document
 *   decides what this browser shows. A failed read is tried again, with a
 *   growing wait, and nothing is saved until one succeeds: a save made
 *   from this browser's guess could undo choices made elsewhere.
 * - A change is saved as a PATCH of just the keys it changed, once changes
 *   stop for SAVE_DELAY_MS, and when the page closes with a save still
 *   waiting. A change made before the document arrived is applied over it
 *   and saved then.
 * - An empty document is a new account: this browser's own choices become
 *   theirs if they are the first account to sign in here, and any copy
 *   this browser kept under their name, from an earlier account of the
 *   same name, is set aside.
 * - Another tab's change is that tab's to save.
 * - With no user signed in, as in the desktop app or a server without
 *   accounts, nothing is sent: the browser keeps its own copy.
 *
 * A link's own parameters still win over any of these; they only supply
 * the defaults (see useGridState and useImageOrder).
 */
export function useAccountPreferences(username: string | undefined): void {
  useEffect(() => {
    if (!username) {
      setPreferencesAccount(null);
      return;
    }
    setPreferencesAccount(username);

    let cancelled = false;
    /** A read has succeeded, so a save builds on the server's document. */
    let ready = false;
    /** Keys changed here and not yet saved. */
    const unsaved = new Set<AccountPreferenceKey>();
    let saving = false;
    let saveFailures = 0;
    let saveTimer: ReturnType<typeof setTimeout> | undefined;
    let readTimer: ReturnType<typeof setTimeout> | undefined;

    const changesToSend = (): Record<string, unknown> | null => {
      if (unsaved.size === 0) return null;
      const preferences = getDisplayPreferences();
      const changes: Record<string, unknown> = {};
      for (const key of unsaved) changes[key] = preferences[key] ?? null;
      unsaved.clear();
      return changes;
    };

    const schedule = (delay = SAVE_DELAY_MS) => {
      if (!ready || cancelled || unsaved.size === 0) return;
      if (saveTimer !== undefined) clearTimeout(saveTimer);
      saveTimer = setTimeout(() => void save(), delay);
    };

    const save = async () => {
      saveTimer = undefined;
      // One save at a time, so the server applies them in the order made.
      if (!ready || cancelled || saving) return;
      const changes = changesToSend();
      if (!changes) return;
      saving = true;
      try {
        await apiClient.patchMyPreferences(changes);
        saveFailures = 0;
      } catch (error) {
        // Keep the keys for the next try, unless changed again since.
        for (const key of Object.keys(changes) as AccountPreferenceKey[]) unsaved.add(key);
        if (!worthRetrying(error)) {
          // Refused: wait for the next change rather than ask again.
          saving = false;
          return;
        }
        saveFailures += 1;
        saving = false;
        schedule(retryDelay(saveFailures));
        return;
      }
      saving = false;
      schedule();
    };

    /** Send what is waiting now, in a request that outlives the page. */
    const flush = () => {
      if (saveTimer !== undefined) clearTimeout(saveTimer);
      saveTimer = undefined;
      if (!ready) return;
      const changes = changesToSend();
      if (changes) void apiClient.patchMyPreferences(changes, { keepalive: true }).catch(() => {});
    };

    const unsubscribe = subscribeLocalPreferenceChanges((changed) => {
      for (const key of changed) unsaved.add(key);
      schedule();
    });
    window.addEventListener('pagehide', flush);

    const adopt = (document: Record<string, unknown>) => {
      const local = getDisplayPreferences();
      let base = document;
      if (Object.keys(document).length > 0) {
        // Their document wins over what this browser had for them. One
        // saved before the scoring defaults existed takes this browser's,
        // if they are the first account here, and saves them.
        const scoring = claimBrowserScoring(document);
        if (scoring) {
          base = { ...document, ...scoring };
          for (const key of Object.keys(scoring) as AccountPreferenceKey[]) unsaved.add(key);
        }
        retireBrowserPreferences();
      } else {
        // A new account, or one re-created under an old name: start from
        // this browser's own choices if no account has claimed them, else
        // from the defaults, never from a copy kept for that name.
        const claimed = claimBrowserPreferences();
        if (claimed) {
          base = claimed;
          for (const key of Object.keys(claimed) as AccountPreferenceKey[]) unsaved.add(key);
        }
      }
      // Changes made while the document was on its way stay.
      const overlay: Record<string, unknown> = {};
      for (const key of unsaved) overlay[key] = local[key];
      adoptAccountPreferences({ ...base, ...overlay });
      ready = true;
      schedule(0);
    };

    const read = async (failures: number) => {
      readTimer = undefined;
      try {
        const remote = await apiClient.getMyPreferences();
        if (cancelled) return;
        adopt(remote.preferences);
      } catch (error) {
        if (cancelled || !worthRetrying(error)) return;
        readTimer = setTimeout(() => void read(failures + 1), retryDelay(failures + 1));
      }
    };
    void read(0);

    return () => {
      cancelled = true;
      if (readTimer !== undefined) clearTimeout(readTimer);
      flush();
      unsubscribe();
      window.removeEventListener('pagehide', flush);
      // Signed out: what the next person sees is not this user's.
      setPreferencesAccount(null);
    };
  }, [username]);
}
