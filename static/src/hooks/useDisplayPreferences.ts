import { useSyncExternalStore } from 'react';

/**
 * Card display choices shared by every surface that renders an ImageCard.
 * One preference, not one per view: the grid and the Sequence view must
 * never disagree about what a card shows.
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
  /** The image detail's sidebar sections the viewer opened or folded. */
  detailSections: DetailSectionChoices;
}

/** A whole set of preferences, where leaving out the detail sections
 *  keeps them as they are: a caller that changes a chip does not reset
 *  which sections are open. */
export type DisplayPreferencesUpdate = Omit<DisplayPreferences, 'detailSections'> & {
  detailSections?: DetailSectionChoices;
};

const STORAGE_KEY = 'psf-guard.display-preferences';
const DEFAULTS: DisplayPreferences = {
  showNightChip: true,
  showAllChip: true,
  showCalibrationChip: true,
  advanceOnGrade: true,
  projectPickerGrouping: 'activity',
  libraryDensity: 'compact',
  detailSections: {},
};

type Listener = () => void;

const listeners = new Set<Listener>();

interface StoredPreferences extends Partial<DisplayPreferences> {
  /** Earlier builds stored one switch for both chip types. */
  showSecondaryScore?: boolean;
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

function sanitize(parsed: StoredPreferences): DisplayPreferences {
  const legacy =
    typeof parsed.showSecondaryScore === 'boolean'
      ? parsed.showSecondaryScore
      : undefined;
  return {
    showNightChip:
      typeof parsed.showNightChip === 'boolean'
        ? parsed.showNightChip
        : legacy ?? DEFAULTS.showNightChip,
    showAllChip:
      typeof parsed.showAllChip === 'boolean'
        ? parsed.showAllChip
        : legacy ?? DEFAULTS.showAllChip,
    showCalibrationChip:
      typeof parsed.showCalibrationChip === 'boolean'
        ? parsed.showCalibrationChip
        : DEFAULTS.showCalibrationChip,
    advanceOnGrade:
      typeof parsed.advanceOnGrade === 'boolean'
        ? parsed.advanceOnGrade
        : DEFAULTS.advanceOnGrade,
    projectPickerGrouping:
      parsed.projectPickerGrouping === 'database' ? 'database' : DEFAULTS.projectPickerGrouping,
    libraryDensity: parsed.libraryDensity === 'detailed' ? 'detailed' : DEFAULTS.libraryDensity,
    detailSections: sanitizeSections(parsed.detailSections),
  };
}

function readStored(): DisplayPreferences {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    return sanitize(JSON.parse(raw) as StoredPreferences);
  } catch {
    return { ...DEFAULTS };
  }
}

let current = readStored();

// A second tab writes the same localStorage key: adopt its value so two
// windows show the same cards the same way.
if (typeof window !== 'undefined') {
  window.addEventListener('storage', (event) => {
    if (event.key !== STORAGE_KEY) return;
    current = readStored();
    listeners.forEach((listener) => listener());
  });
}

export function setDisplayPreferences(next: DisplayPreferencesUpdate): void {
  current = sanitize({ ...next, detailSections: next.detailSections ?? current.detailSections });
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(current));
  } catch {
    // Keep the in-memory preference even when it cannot be persisted.
  }
  listeners.forEach((listener) => listener());
}

/** Remember that the viewer opened or folded one detail section. */
export function setDetailSection(id: DetailSectionId, choice: DetailSectionChoice): void {
  setDisplayPreferences({ ...current, detailSections: { ...current.detailSections, [id]: choice } });
}

function subscribe(listener: Listener): () => void {
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
  return useSyncExternalStore(subscribe, snapshot);
}
