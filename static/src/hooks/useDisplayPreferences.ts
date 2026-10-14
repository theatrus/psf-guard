import { useSyncExternalStore } from 'react';
import { DEFAULT_IMAGE_ORDER, parseImageOrder, type ImageOrder } from '../utils/imageOrder';
import {
  DEFAULT_MULTI_PROJECT_MODE,
  DEFAULT_SINGLE_PROJECT_MODE,
  MULTI_PROJECT_MODES,
  SINGLE_PROJECT_MODES,
} from '../types/grouping';
import { THUMBNAIL_SIZE_MAX, THUMBNAIL_SIZE_MIN } from '../utils/thumbnailSizing';

/**
 * Card display choices shared by every surface that renders an ImageCard.
 * One preference, not one per view: the grid and the Sequence view must
 * never disagree about what a card shows.
 *
 * They live in this browser's storage. With a user signed in to a server,
 * they are that user's: kept under the user's name here and in their
 * document on the server (see useAccountPreferences), so they follow the
 * person to another browser. With no account, as in the desktop app, the
 * browser's own copy is all there is.
 */
/** How the top-bar project picker organizes its list. */
export type ProjectPickerGrouping = 'activity' | 'database';
/** How the Library lists projects on load: one row of pills each, or the full card. */
export type LibraryDensity = 'compact' | 'detailed';
/** The image detail's sidebar sections that fold. */
export const DETAIL_SECTIONS = ['info', 'quality', 'calibration', 'sky', 'satellites'] as const;
export type DetailSectionId = (typeof DETAIL_SECTIONS)[number];
/** A section the viewer opened or folded by hand. */
export type DetailSectionChoice = 'open' | 'closed';
/** The sections the viewer chose for; one left out follows its default. */
export type DetailSectionChoices = Partial<Record<DetailSectionId, DetailSectionChoice>>;
/** How Images groups one project's frames. */
export type ProjectGrouping = (typeof SINGLE_PROJECT_MODES)[number];
/** How Images groups every project's frames, under All projects. */
export type AllProjectsGrouping = (typeof MULTI_PROJECT_MODES)[number];

export interface DisplayPreferences {
  /** Show the smaller "night <score>" chip — the per-session basis shown
   * when the main badge is the all-sessions score. */
  showNightChip: boolean;
  /** Show the smaller "all <score>" chip — the all-sessions basis shown
   * when the main badge is a single session's score. */
  showAllChip: boolean;
  /** Mark lights the calibration library serves poorly, or that are worth
   * a look, with a chip from the background calibration health results. */
  showCalibrationChip: boolean;
  /** Move to the next image after accept/reject/pending. Holding Shift
   * while grading does the opposite of this setting for that one grade. */
  advanceOnGrade: boolean;
  /** Group the top-bar project picker by recent activity or by database. */
  projectPickerGrouping: ProjectPickerGrouping;
  /** Whether Library projects open as compact rows or full cards. */
  libraryDensity: LibraryDensity;
  /** The image detail's sidebar sections the viewer opened or folded. They
   *  stay in this browser: a phone and a desk want different panels open. */
  detailSections: DetailSectionChoices;
  /*
   * The View panel's choices, used whenever a link names none. Each is
   * always set once read; a caller that leaves one out keeps its default.
   */
  /** The order Images and Sequence show their frames in. */
  imageOrder?: ImageOrder;
  /** Images' grouping for one project. */
  imageGrouping?: ProjectGrouping;
  /** Images' grouping under All projects. */
  imageGroupingAllProjects?: AllProjectsGrouping;
  /** Images' thumbnail size, in pixels. */
  gridThumbnailSize?: number;
  /** Sequence's thumbnail size, in pixels. */
  sequenceThumbnailSize?: number;
}

/** A whole set of preferences, where leaving out the detail sections
 *  keeps them as they are: a caller that changes a chip does not reset
 *  which sections are open. */
export type DisplayPreferencesUpdate = Omit<DisplayPreferences, 'detailSections'> & {
  detailSections?: DetailSectionChoices;
};

const STORAGE_KEY = 'psf-guard.display-preferences';
export const DEFAULT_GRID_THUMBNAIL_SIZE = 300;
export const DEFAULT_SEQUENCE_THUMBNAIL_SIZE = 150;
const DEFAULTS: Required<DisplayPreferences> = {
  showNightChip: true,
  showAllChip: true,
  showCalibrationChip: true,
  advanceOnGrade: true,
  projectPickerGrouping: 'activity',
  libraryDensity: 'compact',
  detailSections: {},
  imageOrder: DEFAULT_IMAGE_ORDER,
  imageGrouping: DEFAULT_SINGLE_PROJECT_MODE as ProjectGrouping,
  imageGroupingAllProjects: DEFAULT_MULTI_PROJECT_MODE as AllProjectsGrouping,
  gridThumbnailSize: DEFAULT_GRID_THUMBNAIL_SIZE,
  sequenceThumbnailSize: DEFAULT_SEQUENCE_THUMBNAIL_SIZE,
};

type Listener = () => void;

const listeners = new Set<Listener>();

interface StoredPreferences extends Partial<Record<keyof DisplayPreferences, unknown>> {
  /** Earlier builds stored one switch for both chip types. */
  showSecondaryScore?: unknown;
}

function oneOf<T extends string>(value: unknown, choices: readonly T[], fallback: T): T {
  return choices.includes(value as T) ? value as T : fallback;
}

function thumbnailSize(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isInteger(value)
    && value >= THUMBNAIL_SIZE_MIN && value <= THUMBNAIL_SIZE_MAX
    ? value
    : fallback;
}

function flag(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

function sanitizeSections(value: unknown): DetailSectionChoices {
  if (!value || typeof value !== 'object') return {};
  const stored = value as Record<string, unknown>;
  const choices: DetailSectionChoices = {};
  for (const id of DETAIL_SECTIONS) {
    const choice = stored[id];
    if (choice === 'open' || choice === 'closed') choices[id] = choice;
  }
  return choices;
}

/** Every preference, each a value it may hold: a stored or served value
 *  that is missing or not one of them reads as the default. */
export function sanitizeDisplayPreferences(parsed: StoredPreferences): Required<DisplayPreferences> {
  const legacy =
    typeof parsed.showSecondaryScore === 'boolean'
      ? parsed.showSecondaryScore
      : undefined;
  return {
    showNightChip: flag(parsed.showNightChip, legacy ?? DEFAULTS.showNightChip),
    showAllChip: flag(parsed.showAllChip, legacy ?? DEFAULTS.showAllChip),
    showCalibrationChip: flag(parsed.showCalibrationChip, DEFAULTS.showCalibrationChip),
    advanceOnGrade: flag(parsed.advanceOnGrade, DEFAULTS.advanceOnGrade),
    projectPickerGrouping: oneOf(parsed.projectPickerGrouping, ['activity', 'database'], DEFAULTS.projectPickerGrouping),
    libraryDensity: oneOf(parsed.libraryDensity, ['compact', 'detailed'], DEFAULTS.libraryDensity),
    detailSections: sanitizeSections(parsed.detailSections),
    imageOrder: parseImageOrder(typeof parsed.imageOrder === 'string' ? parsed.imageOrder : null) ?? DEFAULTS.imageOrder,
    imageGrouping: oneOf(parsed.imageGrouping, SINGLE_PROJECT_MODES as readonly ProjectGrouping[], DEFAULTS.imageGrouping),
    imageGroupingAllProjects: oneOf(
      parsed.imageGroupingAllProjects,
      MULTI_PROJECT_MODES as readonly AllProjectsGrouping[],
      DEFAULTS.imageGroupingAllProjects,
    ),
    gridThumbnailSize: thumbnailSize(parsed.gridThumbnailSize, DEFAULTS.gridThumbnailSize),
    sequenceThumbnailSize: thumbnailSize(parsed.sequenceThumbnailSize, DEFAULTS.sequenceThumbnailSize),
  };
}

/** Whose preferences these are: a signed-in user's name, or null for the
 *  browser's own. */
let account: string | null = null;

function keyFor(owner: string | null): string {
  return owner === null ? STORAGE_KEY : `${STORAGE_KEY}@${encodeURIComponent(owner)}`;
}

function readKey(key: string): StoredPreferences | null {
  try {
    const raw = window.localStorage.getItem(key);
    return raw ? JSON.parse(raw) as StoredPreferences : null;
  } catch {
    return null;
  }
}

/** A user's copy, or, before they have one here, the browser's own: a
 *  first sign-in starts from what this browser already chose. */
function readStored(): Required<DisplayPreferences> {
  const stored = readKey(keyFor(account)) ?? (account === null ? null : readKey(keyFor(null)));
  return sanitizeDisplayPreferences(stored ?? {});
}

let current = readStored();

function notify(): void {
  listeners.forEach((listener) => listener());
}

// A second tab writes the same localStorage key: adopt its value so two
// windows show the same cards the same way.
if (typeof window !== 'undefined') {
  window.addEventListener('storage', (event) => {
    if (event.key !== keyFor(account)) return;
    current = readStored();
    notify();
  });
}

export function setDisplayPreferences(next: DisplayPreferencesUpdate): void {
  current = sanitizeDisplayPreferences({
    ...next,
    detailSections: next.detailSections ?? current.detailSections,
  });
  try {
    window.localStorage.setItem(keyFor(account), JSON.stringify(current));
  } catch {
    // Keep the in-memory preference even when it cannot be persisted.
  }
  notify();
}

/** Remember that the viewer opened or folded one detail section. */
export function setDetailSection(id: DetailSectionId, choice: DetailSectionChoice): void {
  setDisplayPreferences({ ...current, detailSections: { ...current.detailSections, [id]: choice } });
}

/** The preferences now, outside React. */
export function getDisplayPreferences(): Required<DisplayPreferences> {
  return current;
}

/**
 * Make the preferences a signed-in user's (their name) or the browser's
 * own (null). Each keeps its own copy in this browser, so one person's
 * choices never become another's.
 */
export function setPreferencesAccount(username: string | null): void {
  if (username === account) return;
  account = username;
  current = readStored();
  notify();
}

/** Whether this browser has a saved copy of its own preferences, which a
 *  first sign-in may hand to the account. */
export function browserHasStoredPreferences(): boolean {
  return readKey(keyFor(null)) !== null;
}

export function subscribeDisplayPreferences(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function snapshot(): DisplayPreferences {
  return current;
}

/** Subscribe to the shared preference. Returns the current snapshot. */
export function useDisplayPreferences(): DisplayPreferences {
  return useSyncExternalStore(subscribeDisplayPreferences, snapshot);
}
