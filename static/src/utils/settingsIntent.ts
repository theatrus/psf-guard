/**
 * Opening the settings modal from elsewhere in the app.
 *
 * The modal lives in `App`, so any component that wants it open asks by
 * window event rather than by prop drilling. The intent rides along so a
 * caller can land the user directly on the form they asked for instead of
 * making them find it again inside the modal.
 */

import type { SettingsSection } from '../components/settingsSections';

/** Section IDs stay stable when navigation labels or groups change. */
export type SettingsIntent = 'add' | 'create' | SettingsSection;

/** A calibration rig whose own settings the Calibration tab should open on. */
export interface SettingsRig {
  uuid: string;
  name: string;
}

export const OPEN_SETTINGS_EVENT = 'psf-guard:open-settings';

export interface OpenSettingsDetail {
  intent?: SettingsIntent;
  /** Registration slug for Rig setup, Database & files or Sync. */
  databaseId?: string;
  /** With `calibration`: the rig to open. */
  rig?: SettingsRig;
  /** With `calibration`: the setting to bring into view and focus, by its
   *  key: the rig's own when a rig rides along, else the global one. With
   *  `catalogs`: the catalog package to select. */
  field?: string;
}

/** Ask `App` to open settings, optionally on a specific form. */
export function openSettings(intent?: SettingsIntent, options: { rig?: SettingsRig; field?: string; databaseId?: string } = {}): void {
  const detail: OpenSettingsDetail = {};
  if (intent) detail.intent = intent;
  if (options.databaseId) detail.databaseId = options.databaseId;
  if (options.rig) detail.rig = options.rig;
  if (options.field) detail.field = options.field;
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

/** Read the setting to focus back off an event, if one rode along. */
export function settingsFieldOf(event: Event): string | null {
  const detail = (event as CustomEvent<OpenSettingsDetail>).detail;
  return detail?.field ?? null;
}

/** The explicitly selected registration; never infer it from a rig name. */
export function settingsDatabaseOf(event: Event): string | null {
  const value = (event as CustomEvent<OpenSettingsDetail>).detail?.databaseId;
  return typeof value === 'string' && value.length > 0 ? value : null;
}
