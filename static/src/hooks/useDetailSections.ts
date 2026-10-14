import { useState } from 'react';
import {
  setDetailSection,
  useDisplayPreferences,
  type DetailSectionChoice,
  type DetailSectionId,
} from './useDisplayPreferences';

export interface DetailSectionState {
  open: boolean;
  /** Open or fold it by hand, which the display preferences remember. */
  toggle: () => void;
}

/**
 * A fold-away panel of the image detail's sidebar that is open until the
 * viewer folds it. The choice lives in the display preferences, so it
 * holds from image to image and across reloads.
 */
export function useDetailSection(id: Exclude<DetailSectionId, 'calibration'>): DetailSectionState {
  const choice = useDisplayPreferences().detailSections[id];
  const open = choice !== 'closed';
  return { open, toggle: () => setDetailSection(id, open ? 'closed' : 'open') };
}

/**
 * Whether the Calibration panel is open.
 *
 * The viewer's choice has three values, kept in the display preferences:
 * - none ("auto", the default): open on a light the calibration check
 *   marked Poor or Check, folded on one with no finding;
 * - "open" or "closed": set by opening or folding the panel by hand, and
 *   then the rule for every light, marked or not.
 *
 * `forced` overrides all three: the viewer came to look at calibration.
 */
export function calibrationSectionOpen(
  choice: DetailSectionChoice | undefined,
  marked: boolean,
  forced: boolean,
): boolean {
  if (forced) return true;
  if (choice) return choice === 'open';
  return marked;
}

/**
 * The Calibration panel's open state; see `calibrationSectionOpen`.
 *
 * `purpose` is why the viewer is looking at calibration, empty when they
 * are not: the finding chosen in the address (`calibFocus`, which the 🧪
 * chip sets) or the Images calibration filter (`calib`). It opens the panel
 * whatever the choice. Folding the panel by hand still works: it stores
 * "closed" and keeps the panel folded until the purpose changes, such as
 * another chip or filter. Moving between images keeps the purpose, so the
 * panel stays as the viewer left it.
 */
export function useCalibrationSection(marked: boolean, purpose: string): DetailSectionState {
  const choice = useDisplayPreferences().detailSections.calibration;
  const [foldedFor, setFoldedFor] = useState<string | null>(null);
  const forced = purpose !== '' && foldedFor !== purpose;
  const open = calibrationSectionOpen(choice, marked, forced);
  const toggle = () => {
    if (open && purpose) setFoldedFor(purpose);
    setDetailSection('calibration', open ? 'closed' : 'open');
  };
  return { open, toggle };
}
