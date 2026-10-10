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
