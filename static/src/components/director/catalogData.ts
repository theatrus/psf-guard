import { isAxiosError } from 'axios';
import { apiClient } from '../../api/client';
import type { DirectorIdentity, DirectorIdentityPage, DirectorMappingPage } from '../../api/directorTypes';

async function allPages<T>(read: (after?: string) => Promise<{ items: T[]; next_after: string | null }>): Promise<T[]> {
  const items: T[] = [];
  const cursors = new Set<string>();
  let after: string | undefined;
  do {
    const page = await read(after);
    items.push(...page.items);
    if (items.length > 4096 || cursors.size >= 64) throw new Error('Too many Director records to map in this view.');
    if (page.next_after && cursors.has(page.next_after)) throw new Error('Director returned a repeated page. Refresh the catalog.');
    if (page.next_after) cursors.add(page.next_after);
    after = page.next_after ?? undefined;
  } while (after);
  return items;
}

export async function loadCatalog(slug: string) {
  const discovery = await apiClient.discoverDirectorCatalog(slug);
  let rig: DirectorIdentity | null = null;
  let firstPage = true;
  // Metadata admission is deliberately serial; do not fan out these requests.
  const mappings = await allPages(async after => {
    const page: DirectorMappingPage = await apiClient.getDirectorMappings(slug, after);
    if (page.catalog_identity?.id !== discovery.catalog_identity?.id
      || page.catalog_identity?.origin_instance_id !== discovery.catalog_identity?.origin_instance_id) {
      throw new Error('Catalog identity changed. Refresh the catalog.');
    }
    if (!firstPage && JSON.stringify(page.rig ?? null) !== JSON.stringify(rig)) {
      throw new Error('Database rig changed. Refresh the catalog.');
    }
    rig = page.rig ?? null;
    firstPage = false;
    return page;
  });
  const projects = await allPages(async after => {
    const page: DirectorIdentityPage = await apiClient.getDirectorIdentities('projects', after);
    return page;
  });
  return { discovery, mappings, projects, rig: rig as DirectorIdentity | null };
}

export type CatalogData = Awaited<ReturnType<typeof loadCatalog>>;

/** Director admits one metadata request at a time and answers 503 while busy; wait, do not fail. */
export function retryWhenBusy(count: number, error: Error): boolean {
  const status = isAxiosError(error) ? error.response?.status
    : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
  return status === 503 && count < 5;
}
