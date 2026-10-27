import { apiClient } from '../api/client';

export const STORED_PREVIEW_KEY = 'psf-guard.scheduler-sync-preview';

/** Discard only this form's saved preview; server-staged previews keep their own scope. */
export async function discardCatalogSyncPreview(databaseId: string): Promise<void> {
  const key = `${STORED_PREVIEW_KEY}:${databaseId}`;
  const stored = sessionStorage.getItem(key);
  sessionStorage.removeItem(key);
  if (!stored) return;
  try {
    const ref = JSON.parse(stored) as { localDbId?: string; previewId?: string };
    if (ref.localDbId === databaseId && ref.previewId)
      await apiClient.deleteDatabaseSyncPreview(databaseId, ref.previewId).catch(() => undefined);
  } catch { /* An unreadable local reference is already discarded. */ }
}

