import { useEffect } from 'react';
import { apiClient } from '../api/client';
import {
  browserHasStoredPreferences,
  getDisplayPreferences,
  sanitizeDisplayPreferences,
  setDisplayPreferences,
  setPreferencesAccount,
  subscribeDisplayPreferences,
  type DisplayPreferences,
} from './useDisplayPreferences';

/** How long after the last change the preferences are saved. */
export const SAVE_DELAY_MS = 800;

/** The preferences as the server keeps them: every known key, set. The
 *  detail view's folded panels stay in this browser. */
export function preferencesDocument(preferences: DisplayPreferences): Record<string, unknown> {
  const document: Record<string, unknown> = { ...sanitizeDisplayPreferences(preferences) };
  delete document.detailSections;
  return document;
}

/**
 * Keep the display preferences in the signed-in user's document on the
 * server, so the View panel's choices follow them to another browser.
 *
 * - On sign-in, or when the app opens signed in, the user's document
 *   replaces what this browser shows. If they have none yet, this
 *   browser's own saved choices become theirs.
 * - Each change is saved once changes stop for SAVE_DELAY_MS, and when the
 *   page closes with a save still waiting.
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
    let ready = false;
    let saved = '';
    let timer: ReturnType<typeof setTimeout> | undefined;

    const save = async (options: { keepalive?: boolean } = {}) => {
      timer = undefined;
      const document = preferencesDocument(getDisplayPreferences());
      const json = JSON.stringify(document);
      if (json === saved) return;
      try {
        await apiClient.saveMyPreferences(document, options);
        saved = json;
      } catch {
        // Kept in this browser; the next change tries again.
      }
    };
    const schedule = () => {
      if (!ready || cancelled) return;
      if (timer !== undefined) clearTimeout(timer);
      timer = setTimeout(() => void save(), SAVE_DELAY_MS);
    };
    const flush = () => {
      if (timer === undefined) return;
      clearTimeout(timer);
      void save({ keepalive: true });
    };
    const unsubscribe = subscribeDisplayPreferences(schedule);
    window.addEventListener('pagehide', flush);

    void (async () => {
      try {
        const remote = await apiClient.getMyPreferences();
        if (cancelled) return;
        if (Object.keys(remote.preferences).length > 0) {
          const applied = {
            ...getDisplayPreferences(),
            ...remote.preferences,
          } as DisplayPreferences;
          saved = JSON.stringify(preferencesDocument(applied));
          setDisplayPreferences(applied);
        } else if (browserHasStoredPreferences()) {
          // A first sign-in: what this browser chose becomes the account's.
          ready = true;
          await save();
        }
      } catch {
        // The server could not say: keep this browser's copy for now.
      }
      if (!cancelled) ready = true;
    })();

    return () => {
      cancelled = true;
      flush();
      unsubscribe();
      window.removeEventListener('pagehide', flush);
    };
  }, [username]);
}
