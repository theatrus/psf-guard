import { useSyncExternalStore } from 'react';

/** Response header naming the frontend build the server holds. */
export const BUILD_HEADER = 'x-psf-guard-build';

/** The build this page came from, stamped into index.html by the Vite build
 *  (`vite.config.ts`). The dev server leaves it out; then nothing is compared. */
const pageBuild = () =>
  document.querySelector<HTMLMetaElement>('meta[name="psf-guard-build"]')?.content || null;

/** The server's build, once it differs from this page's. */
let newerBuild: string | null = null;
const listeners = new Set<() => void>();

/** Whether the browser may have kept a reply with this Cache-Control and
 *  handed it back without asking the server. Such a reply names the build of
 *  the day it was fetched, not the server's: a survey image kept for a day
 *  would say PSF Guard was updated after every reload. */
export function keptByBrowser(cacheControl: unknown): boolean {
  const value = typeof cacheControl === 'string' ? cacheControl.toLowerCase() : '';
  if (value.includes('no-store') || value.includes('no-cache')) return false;
  return value.includes('immutable') || /(^|,)\s*max-age=0*[1-9]/.test(value);
}

/** Note the build the server named on a reply, unless the reply may have come
 *  from the browser's cache. */
export function noteServerBuild(build: unknown, cacheControl?: unknown) {
  if (keptByBrowser(cacheControl)) return;
  if (typeof build !== 'string' || !build || build === newerBuild) return;
  const page = pageBuild();
  if (!page || build === page) return;
  newerBuild = build;
  listeners.forEach(listener => listener());
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
};

/** The server's build when it is not this page's, else null. */
export const useNewerBuild = () => useSyncExternalStore(subscribe, () => newerBuild);

const RELOADED_FOR = 'psf-guard-reloaded-for';

/** Reload into the server's build. A tab that already reloaded for that build
 *  and came back with the old page (a cache between it and the server) does
 *  not try again, or it would reload on every change of view. */
export function reloadForNewerBuild(reload = () => window.location.reload()): boolean {
  if (!newerBuild) return false;
  try {
    if (sessionStorage.getItem(RELOADED_FOR) === newerBuild) return false;
    sessionStorage.setItem(RELOADED_FOR, newerBuild);
  } catch {
    // Without storage there is no record of an earlier try; reload anyway.
  }
  reload();
  return true;
}

/** The views a newer build is loaded between. Moving inside one (grid to an
 *  image, image to image) keeps the page, and its scroll and selection. */
const VIEW_OF: Record<string, string> = {
  '': 'overview',
  grid: 'images',
  detail: 'images',
  compare: 'images',
  director: 'plan',
};

export const viewOf = (pathname: string) => {
  const first = pathname.split('/')[1] ?? '';
  return VIEW_OF[first] ?? first;
};
