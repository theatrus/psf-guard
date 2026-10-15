/**
 * Opening the settings modal from elsewhere in the app.
 *
 * The modal lives in `App`, so any component that wants it open asks by
 * window event rather than by prop drilling. The intent rides along so a
 * caller can land the user directly on the form they asked for instead of
 * making them find it again inside the modal.
 */

/** Which form the settings modal should open on, if any. */
export type SettingsIntent =
  | 'add'
  | 'create'
  | 'rigs'
  | 'templates'
  | 'stacking'
  | 'storage'
  | 'review'
  | 'calibration';

/** A calibration rig whose own settings the Calibration tab should open on. */
export interface SettingsRig {
  uuid: string;
  name: string;
}

export const OPEN_SETTINGS_EVENT = 'psf-guard:open-settings';

export interface OpenSettingsDetail {
  intent?: SettingsIntent;
  /** With `calibration`: the rig to open. */
  rig?: SettingsRig;
}

/** Ask `App` to open settings, optionally on a specific form. */
export function openSettings(intent?: SettingsIntent, options: { rig?: SettingsRig } = {}): void {
  const detail: OpenSettingsDetail = {};
  if (intent) detail.intent = intent;
  if (options.rig) detail.rig = options.rig;
  window.dispatchEvent(new CustomEvent<OpenSettingsDetail>(OPEN_SETTINGS_EVENT, { detail }));
}

/** Read the intent back off an event, tolerating a bare `dispatchEvent`. */
export function settingsIntentOf(event: Event): SettingsIntent | null {
  const detail = (event as CustomEvent<OpenSettingsDetail>).detail;
  return detail?.intent ?? null;
}

/** Read the rig back off an event, if one rode along. */
export function settingsRigOf(event: Event): SettingsRig | null {
  const detail = (event as CustomEvent<OpenSettingsDetail>).detail;
  return detail?.rig ?? null;
}
