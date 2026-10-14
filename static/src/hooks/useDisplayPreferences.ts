import { useSyncExternalStore } from 'react';
import { DEFAULT_IMAGE_ORDER, parseImageOrder, type ImageOrder } from '../utils/imageOrder';
import {
  DEFAULT_MULTI_PROJECT_MODE,
  DEFAULT_SINGLE_PROJECT_MODE,
  MULTI_PROJECT_MODES,
  SINGLE_PROJECT_MODES,
} from '../types/grouping';
import { THUMBNAIL_SIZE_MAX, THUMBNAIL_SIZE_MIN } from '../utils/thumbnailSizing';
import {
  fieldsFromScoring,
  normalizeScoring,
  SCORING_DEFAULTS,
  SCORING_PREFERENCE_KEYS,
  scoringFromFields,
  storedScoringFields,
} from '../utils/scoring';

/**
 * Card display choices shared by every surface that renders an ImageCard.
 * One preference, not one per view: the grid and the Sequence view must
 * never disagree about what a card shows.
 *
 * They live in this browser's storage. With a user signed in to a server,
 * they are that user's: kept under the user's name here and in their
 * document on the server (see useAccountPreferences), so they follow the
 * person to another browser. The browser's own copy, from before anyone
 * signed in, goes to the first account to sign in here and to no other.
 * With no account, as in the desktop app, the browser's own copy is all
 * there is.
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
  /*
   * The Scoring control's defaults: what each view scores with until the
   * person changes it there. See useScoringPreferences.
   */
  scoringPenaltySatellite?: number;
  scoringPenaltyPointing?: number;
  scoringPenaltyTemporal?: number;
  scoringHfrRejectAbove?: number | null;
  scoringStarCountRejectBelow?: number | null;
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
  ...fieldsFromScoring(SCORING_DEFAULTS),
};

/** The preferences a signed-in user's server document holds: all but the
 *  detail view's folded panels, which stay in each browser. */
export type AccountPreferenceKey = Exclude<keyof DisplayPreferences, 'detailSections'>;
export const ACCOUNT_PREFERENCE_KEYS = (Object.keys(DEFAULTS) as (keyof DisplayPreferences)[])
  .filter((key): key is AccountPreferenceKey => key !== 'detailSections');

/** Where the Scoring control kept its one setting before it had defaults.
 *  It moves into the browser's own preferences on the first load. */
export const LEGACY_SCORING_KEY = 'psf-guard.penalty-scales';

type Listener = () => void;
/** Told which account preferences a change in this tab made. */
type LocalChangeListener = (changed: AccountPreferenceKey[]) => void;

const listeners = new Set<Listener>();
const localChangeListeners = new Set<LocalChangeListener>();

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
    ...fieldsFromScoring(scoringFromFields(parsed)),
  };
}

/** Whose preferences these are: a signed-in user's name, or null for the
 *  browser's own. */
let account: string | null = null;

function keyFor(owner: string | null): string {
  return owner === null ? STORAGE_KEY : `${STORAGE_KEY}@${encodeURIComponent(owner)}`;
}

/** Set once the first account to sign in here has claimed the browser's
 *  own copy, so it is handed to one account at most. */
const HANDED_OVER_KEY = `${STORAGE_KEY}.handed-over`;

function readKey(key: string): StoredPreferences | null {
  try {
    const raw = window.localStorage.getItem(key);
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    // Anything but an object, such as a stray number, is no preferences.
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? parsed as StoredPreferences
      : null;
  } catch {
    return null;
  }
}

function handOverPending(): boolean {
  try {
    return window.localStorage.getItem(HANDED_OVER_KEY) === null && readKey(keyFor(null)) !== null;
  } catch {
    return false;
  }
}

/** A user's copy, or, before they have one here, the browser's own while
 *  no account has claimed it: a first sign-in starts from what this
 *  browser already chose. */
function readStored(): Required<DisplayPreferences> {
  const stored = readKey(keyFor(account))
    ?? (account !== null && handOverPending() ? readKey(keyFor(null)) : null);
  return sanitizeDisplayPreferences(stored ?? {});
}

function persist(): void {
  try {
    window.localStorage.setItem(keyFor(account), JSON.stringify(current));
  } catch {
    // Keep the in-memory preference even when it cannot be persisted.
  }
}

/**
 * Before the scoring defaults, the Scoring control kept the values it last
 * used in one key of its own. They become this browser's own defaults,
 * unless it already has some, and go to the first account to sign in here
 * with the rest of the browser's choices (claimBrowserPreferences, or
 * claimBrowserScoring for an account whose document predates them).
 */
export function migrateLegacyScoring(): void {
  try {
    const raw = window.localStorage.getItem(LEGACY_SCORING_KEY);
    if (raw === null) return;
    const own = readKey(keyFor(null)) ?? {};
    const hasDefaults = Object.values(SCORING_PREFERENCE_KEYS).some((key) => key in own);
    if (!hasDefaults) {
      let legacy: unknown = null;
      try {
        legacy = JSON.parse(raw);
      } catch {
        // Unreadable: nothing to carry over.
      }
      if (legacy && typeof legacy === 'object') {
        window.localStorage.setItem(
          keyFor(null),
          JSON.stringify({
            ...own,
            ...fieldsFromScoring(normalizeScoring(legacy as Record<string, unknown>)),
          }),
        );
      }
    }
    window.localStorage.removeItem(LEGACY_SCORING_KEY);
  } catch {
    // Storage is unavailable; the built-in defaults apply.
  }
}

migrateLegacyScoring();
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

/** A choice made in this tab. A signed-in user's account hears which of
 *  their preferences it changed, to save just those. */
export function setDisplayPreferences(next: DisplayPreferencesUpdate): void {
  const previous = current;
  current = sanitizeDisplayPreferences({
    ...next,
    detailSections: next.detailSections ?? current.detailSections,
  });
  persist();
  notify();
  // By value, so a preference that holds an object counts only a real change.
  const changed = ACCOUNT_PREFERENCE_KEYS.filter(
    (key) => JSON.stringify(previous[key]) !== JSON.stringify(current[key]),
  );
  if (changed.length > 0) localChangeListeners.forEach((listener) => listener(changed));
}

/**
 * Show the signed-in user's preferences as their server document has them:
 * its keys, and the default for each it leaves out. The detail sections
 * stay as this browser has them. Not a change to save back.
 */
export function adoptAccountPreferences(document: Record<string, unknown>): void {
  current = sanitizeDisplayPreferences({
    ...(document as StoredPreferences),
    detailSections: current.detailSections,
  });
  persist();
  notify();
}

/**
 * The browser's own choices, for the first account to sign in here, and
 * only the keys it stored. Claiming them marks them handed over, so no
 * later account gets them: null once claimed, or when there are none.
 */
export function claimBrowserPreferences(): Partial<Record<AccountPreferenceKey, unknown>> | null {
  if (!handOverPending()) return null;
  const stored = readKey(keyFor(null)) ?? {};
  try {
    window.localStorage.setItem(HANDED_OVER_KEY, '1');
  } catch {
    // Unwritable storage cannot keep the mark; nor could it keep the copy.
  }
  const sanitized = sanitizeDisplayPreferences(stored);
  // The one-switch build's chip choice reads as both chips.
  const chosen = new Set<string>(Object.keys(stored));
  if ('showSecondaryScore' in stored) {
    chosen.add('showNightChip');
    chosen.add('showAllChip');
  }
  const claimed: Partial<Record<AccountPreferenceKey, unknown>> = {};
  for (const key of ACCOUNT_PREFERENCE_KEYS) {
    if (chosen.has(key)) claimed[key] = sanitized[key];
  }
  return claimed;
}

/**
 * The browser's own scoring defaults, for the first account to sign in
 * here whose document was saved before the scoring defaults existed: its
 * document wins for everything it holds, and these fill the keys it lacks.
 * Null when another account has claimed the browser's choices, when the
 * document holds any scoring default, or when the browser has none.
 * Claims nothing; retireBrowserPreferences does that.
 */
export function claimBrowserScoring(
  document: Record<string, unknown>,
): Partial<Record<AccountPreferenceKey, unknown>> | null {
  if (!handOverPending()) return null;
  const keys = Object.values(SCORING_PREFERENCE_KEYS);
  if (keys.some((key) => key in document)) return null;
  const stored = storedScoringFields(readKey(keyFor(null)));
  if (Object.keys(stored).length === 0) return null;
  const sanitized = sanitizeDisplayPreferences(stored);
  const claimed: Partial<Record<AccountPreferenceKey, unknown>> = {};
  for (const key of keys) {
    if (key in stored) claimed[key] = sanitized[key];
  }
  return claimed;
}

/** Mark the browser's own copy claimed without handing it over, as when
 *  the first account to sign in here already has a document. */
export function retireBrowserPreferences(): void {
  if (!handOverPending()) return;
  try {
    window.localStorage.setItem(HANDED_OVER_KEY, '1');
  } catch {
    // As above.
  }
}

/** Hear the account preferences each change in this tab makes. Another
 *  tab's change, or the server's document arriving, is not one. */
export function subscribeLocalPreferenceChanges(listener: LocalChangeListener): () => void {
  localChangeListeners.add(listener);
  return () => {
    localChangeListeners.delete(listener);
  };
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
