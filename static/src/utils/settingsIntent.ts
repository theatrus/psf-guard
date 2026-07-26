/**
 * Opening the settings modal from elsewhere in the app.
 *
 * The modal lives in `App`, so any component that wants it open asks by
 * window event rather than by prop drilling. The intent rides along so a
 * caller can land the user directly on the form they asked for instead of
 * making them find it again inside the modal.
 */

/** Which form the settings modal should open on, if any. */
export type SettingsIntent = 'add' | 'create';

export const OPEN_SETTINGS_EVENT = 'psf-guard:open-settings';

export interface OpenSettingsDetail {
  intent?: SettingsIntent;
}

/** Ask `App` to open settings, optionally on a specific form. */
export function openSettings(intent?: SettingsIntent): void {
  window.dispatchEvent(
    new CustomEvent<OpenSettingsDetail>(OPEN_SETTINGS_EVENT, {
      detail: intent ? { intent } : {},
    })
  );
}

/** Read the intent back off an event, tolerating a bare `dispatchEvent`. */
export function settingsIntentOf(event: Event): SettingsIntent | null {
  const detail = (event as CustomEvent<OpenSettingsDetail>).detail;
  return detail?.intent ?? null;
}
