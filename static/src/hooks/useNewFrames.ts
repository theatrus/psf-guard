import { useEffect, useMemo, useState } from 'react';
import {
  isNewFrame,
  loadProjectSeenState,
  PROJECT_SEEN_STORAGE_KEY,
  type FrameRecency,
} from '../utils/projectRecency';

/**
 * The IDs of the frames that are new, by the Library's own record of what
 * this browser has seen (see isNewFrame). Read when the view opens, and
 * again when another window updates it.
 */
export function useNewFrameIds(
  dbId: string | null | undefined,
  frames: readonly (FrameRecency & { id: number })[],
): ReadonlySet<number> {
  const [seen, setSeen] = useState(loadProjectSeenState);

  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key === PROJECT_SEEN_STORAGE_KEY) setSeen(loadProjectSeenState());
    };
    window.addEventListener('storage', onStorage);
    return () => window.removeEventListener('storage', onStorage);
  }, []);

  return useMemo(
    () => new Set(dbId ? frames.filter((frame) => isNewFrame(seen, dbId, frame)).map((frame) => frame.id) : []),
    [dbId, frames, seen],
  );
}
