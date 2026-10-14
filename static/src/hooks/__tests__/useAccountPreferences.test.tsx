import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { SAVE_DELAY_MS, useAccountPreferences } from '../useAccountPreferences';
import {
  getDisplayPreferences,
  setDisplayPreferences,
  setPreferencesAccount,
} from '../useDisplayPreferences';

const BROWSER_KEY = 'psf-guard.display-preferences';

/** The server's documents by user, and every save it received. */
function fakeServer(documents: Record<string, Record<string, unknown>>, user: () => string) {
  const saves: { user: string; preferences: Record<string, unknown> }[] = [];
  server.use(
    http.get('/api/me/preferences', () => HttpResponse.json({
      success: true, error: null, status: 'ready',
      data: { schema_version: 1, preferences: documents[user()] ?? {} },
    })),
    http.put('/api/me/preferences', async ({ request }) => {
      const body = await request.json() as { schema_version: number; preferences: Record<string, unknown> };
      expect(body.schema_version).toBe(1);
      documents[user()] = body.preferences;
      saves.push({ user: user(), preferences: body.preferences });
      return HttpResponse.json({
        success: true, error: null, status: 'ready',
        data: { schema_version: 1, preferences: body.preferences },
      });
    }),
  );
  return saves;
}

describe('account preferences', () => {
  beforeEach(() => {
    window.localStorage.clear();
    setPreferencesAccount(null);
    setDisplayPreferences({ ...getDisplayPreferences(), imageOrder: 'oldest' });
    window.localStorage.clear();
  });
  afterEach(() => {
    setPreferencesAccount(null);
    window.localStorage.clear();
  });

  it('applies the signed-in user\'s saved choices', async () => {
    const saves = fakeServer({ alice: { imageOrder: 'newest', imageGrouping: 'session', gridThumbnailSize: 450 } }, () => 'alice');
    renderHook(() => useAccountPreferences('alice'));
    await waitFor(() => expect(getDisplayPreferences().imageOrder).toBe('newest'));
    expect(getDisplayPreferences().imageGrouping).toBe('session');
    expect(getDisplayPreferences().gridThumbnailSize).toBe(450);
    // Applying them is not a change to send back.
    await new Promise((resolve) => setTimeout(resolve, SAVE_DELAY_MS + 200));
    expect(saves).toHaveLength(0);
    // They are kept under the user's name, not as the browser's own.
    expect(window.localStorage.getItem(`${BROWSER_KEY}@alice`)).toContain('"newest"');
    expect(window.localStorage.getItem(BROWSER_KEY)).toBeNull();
  });

  it('saves changes once they stop', async () => {
    const saves = fakeServer({ alice: { imageOrder: 'oldest' } }, () => 'alice');
    renderHook(() => useAccountPreferences('alice'));
    await waitFor(() => expect(getDisplayPreferences().imageOrder).toBe('oldest'));
    await new Promise((resolve) => setTimeout(resolve, 50));

    act(() => setDisplayPreferences({ ...getDisplayPreferences(), imageOrder: 'newest' }));
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), imageGrouping: 'date' }));
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), showAllChip: false }));
    expect(saves).toHaveLength(0);
    await waitFor(() => expect(saves).toHaveLength(1), { timeout: SAVE_DELAY_MS * 4 });
    expect(saves[0].preferences).toMatchObject({
      imageOrder: 'newest',
      imageGrouping: 'date',
      showAllChip: false,
    });
    // The detail view's folded panels stay in this browser.
    expect(saves[0].preferences).not.toHaveProperty('detailSections');
  });

  it('hands this browser\'s own choices to an account that has none', async () => {
    window.localStorage.setItem(BROWSER_KEY, JSON.stringify({ libraryDensity: 'detailed', imageOrder: 'newest' }));
    setPreferencesAccount(null);
    const saves = fakeServer({}, () => 'alice');
    renderHook(() => useAccountPreferences('alice'));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].preferences).toMatchObject({ libraryDensity: 'detailed', imageOrder: 'newest' });
  });

  it('does not hand one user\'s choices to the next', async () => {
    const documents: Record<string, Record<string, unknown>> = { alice: { imageOrder: 'newest' } };
    let user = 'alice';
    const saves = fakeServer(documents, () => user);
    const { rerender, unmount } = renderHook(({ name }) => useAccountPreferences(name), {
      initialProps: { name: 'alice' as string | undefined },
    });
    await waitFor(() => expect(getDisplayPreferences().imageOrder).toBe('newest'));

    // Alice signs out; Bob, who has saved nothing, signs in here.
    rerender({ name: undefined });
    expect(getDisplayPreferences().imageOrder).toBe('oldest');
    user = 'bob';
    rerender({ name: 'bob' });
    await new Promise((resolve) => setTimeout(resolve, SAVE_DELAY_MS + 200));
    expect(getDisplayPreferences().imageOrder).toBe('oldest');
    expect(saves.filter((save) => save.user === 'bob')).toHaveLength(0);
    unmount();
  });

  it('with no account, keeps the browser\'s copy and asks the server nothing', async () => {
    let requests = 0;
    server.use(
      http.get('/api/me/preferences', () => {
        requests += 1;
        return HttpResponse.json({}, { status: 404 });
      }),
      http.put('/api/me/preferences', () => {
        requests += 1;
        return HttpResponse.json({}, { status: 404 });
      }),
    );
    renderHook(() => useAccountPreferences(undefined));
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), imageOrder: 'newest' }));
    await new Promise((resolve) => setTimeout(resolve, SAVE_DELAY_MS + 200));
    expect(requests).toBe(0);
    expect(window.localStorage.getItem(BROWSER_KEY)).toContain('"newest"');
  });
});
