import { useState } from 'react';
import type { LibraryDensity } from './useDisplayPreferences';

/** Which project rows show their full card. In the compact view a row is
 *  folded unless opened (or it is the project the user came from); in the
 *  detailed view a card shows unless folded. Both the Library and the plan
 *  list share this, so the two lists behave the same way. */
export function useFoldedRows(density: LibraryDensity) {
  const [opened, setOpened] = useState<Set<string>>(() => new Set());
  const [folded, setFolded] = useState<Set<string>>(() => new Set());
  const showsCard = (key: string, isCurrent = false) =>
    density === 'detailed' ? !folded.has(key) : opened.has(key) || (isCurrent && !folded.has(key));
  const toggle = (key: string, isCurrent = false) => {
    const shown = showsCard(key, isCurrent);
    setOpened(current => { const next = new Set(current); if (shown) next.delete(key); else next.add(key); return next; });
    setFolded(current => { const next = new Set(current); if (shown) next.add(key); else next.delete(key); return next; });
  };
  const reset = () => { setOpened(new Set()); setFolded(new Set()); };
  return { showsCard, toggle, reset };
}
