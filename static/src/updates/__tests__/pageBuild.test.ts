import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const stamp = (build: string) => {
  const meta = document.createElement('meta');
  meta.name = 'psf-guard-build';
  meta.content = build;
  document.head.append(meta);
};

const load = () => import('../pageBuild');

describe('page build', () => {
  beforeEach(() => {
    vi.resetModules();
    sessionStorage.clear();
  });
  afterEach(() => {
    document.head.querySelectorAll('meta[name="psf-guard-build"]').forEach(meta => meta.remove());
  });

  it('reloads once the server names another build, and only once for it', async () => {
    stamp('aaaa');
    const { noteServerBuild, reloadForNewerBuild } = await load();
    const reload = vi.fn();
    noteServerBuild('aaaa');
    noteServerBuild(undefined);
    expect(reloadForNewerBuild(reload)).toBe(false);

    noteServerBuild('bbbb');
    expect(reloadForNewerBuild(reload)).toBe(true);
    // Came back with the old page: a cache holds it, so stop trying.
    expect(reloadForNewerBuild(reload)).toBe(false);
    expect(reload).toHaveBeenCalledTimes(1);

    noteServerBuild('cccc');
    expect(reloadForNewerBuild(reload)).toBe(true);
    expect(reload).toHaveBeenCalledTimes(2);
  });

  it('ignores a reply the browser may have kept', async () => {
    stamp('aaaa');
    const { keptByBrowser, noteServerBuild, reloadForNewerBuild } = await load();
    // A survey image kept for a day still carries the build it was fetched
    // under; it says nothing about the server now.
    noteServerBuild('old0', 'private, max-age=86400');
    noteServerBuild('old0', 'public, max-age=31536000, immutable');
    expect(reloadForNewerBuild(vi.fn())).toBe(false);
    noteServerBuild('bbbb', 'private, no-store');
    expect(reloadForNewerBuild(vi.fn())).toBe(true);

    expect(keptByBrowser('max-age=0')).toBe(false);
    expect(keptByBrowser('max-age=60, no-cache')).toBe(false);
    expect(keptByBrowser(undefined)).toBe(false);
    expect(keptByBrowser('Private, Max-Age=10')).toBe(true);
  });

  it('compares nothing on an unstamped page', async () => {
    const { noteServerBuild, reloadForNewerBuild } = await load();
    noteServerBuild('bbbb');
    expect(reloadForNewerBuild(vi.fn())).toBe(false);
  });

  it('groups the routes of one view', async () => {
    const { viewOf } = await load();
    expect(viewOf('/')).toBe('overview');
    expect(viewOf('/overview')).toBe('overview');
    expect(viewOf('/grid')).toBe('images');
    expect(viewOf('/detail/42')).toBe('images');
    expect(viewOf('/compare/1/2')).toBe('images');
    expect(viewOf('/sequence')).toBe('sequence');
    expect(viewOf('/director')).toBe('plan');
  });
});
