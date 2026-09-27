import { apiClient } from '../../api/client';
import type { DirectorIdentityPage, DirectorMappingPage } from '../../api/directorTypes';

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
  // Metadata admission is deliberately serial; do not fan out these requests.
  const mappings = await allPages(async after => {
    const page: DirectorMappingPage = await apiClient.getDirectorMappings(slug, after);
    if (page.catalog_identity?.id !== discovery.catalog_identity?.id
      || page.catalog_identity?.origin_instance_id !== discovery.catalog_identity?.origin_instance_id) {
      throw new Error('Catalog identity changed. Refresh the catalog.');
    }
    return page;
  });
  const projects = await allPages(async after => {
    const page: DirectorIdentityPage = await apiClient.getDirectorIdentities('projects', after);
    return page;
  });
  const rigs = await allPages(async after => {
    const page: DirectorIdentityPage = await apiClient.getDirectorIdentities('rigs', after);
    return page;
  });
  return { discovery, mappings, projects, rigs };
}

export type CatalogData = Awaited<ReturnType<typeof loadCatalog>>;
