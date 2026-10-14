import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { act, render, renderHook, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { RETRY_FIRST_MS, SAVE_DELAY_MS, retryDelay, useAccountPreferences } from '../useAccountPreferences';
import {
  getDisplayPreferences,
  setDisplayPreferences,
  setPreferencesAccount,
} from '../useDisplayPreferences';

const BROWSER_KEY = 'psf-guard.display-preferences';

type Document = Record<string, unknown>;

/**
 * The server's documents by user, as `/api/me/preferences` keeps them: a
 * PATCH merges its keys and a null drops one. Records every save, and
 * fails the test on a PUT, which this build never sends.
 */
function fakeServer(documents: Record<string, Document>, user: () => string) {
  const saves: { user: string; changes: Document }[] = [];
  server.use(
    http.get('/api/me/preferences', () => HttpResponse.json({
      success: true, error: null, status: 'ready',
      data: { schema_version: 1, preferences: documents[user()] ?? {} },
    })),
    http.patch('/api/me/preferences', async ({ request }) => {
      const body = await request.json() as { schema_version: number; preferences: Document };
      expect(body.schema_version).toBe(1);
      const merged = { ...(documents[user()] ?? {}) };
      for (const [key, value] of Object.entries(body.preferences)) {
        if (value === null) delete merged[key];
        else merged[key] = value;
      }
      documents[user()] = merged;
      saves.push({ user: user(), changes: body.preferences });
      return HttpResponse.json({
        success: true, error: null, status: 'ready',
        data: { schema_version: 1, preferences: merged },
      });
    }),
    http.put('/api/me/preferences', () => {
      throw new Error('This build saves with PATCH');
    }),
  );
  return saves;
}

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** The app's shape: AuthGate shows the sign-in form instead of AppContent,
 *  so signing out unmounts the component that holds the hook. */
function SignedIn({ name }: { name: string }) {
  useAccountPreferences(name);
  return <p>signed in as {name}</p>;
}
function App({ user }: { user: string | null }) {
  return user ? <SignedIn name={user} /> : <p>Sign in to PSF Guard</p>;
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

  it('applies the signed-in user\'s saved choices, and the default for any it leaves out', async () => {
    window.localStorage.setItem(`${BROWSER_KEY}@alice`, JSON.stringify({ libraryDensity: 'detailed' }));
    const saves = fakeServer({ alice: { imageOrder: 'newest', imageGrouping: 'session', gridThumbnailSize: 450 } }, () => 'alice');
    renderHook(() => useAccountPreferences('alice'));
    await waitFor(() => expect(getDisplayPreferences().imageOrder).toBe('newest'));
    expect(getDisplayPreferences().imageGrouping).toBe('session');
    expect(getDisplayPreferences().gridThumbnailSize).toBe(450);
    // The server's document decides: another browser reset this one.
    expect(getDisplayPreferences().libraryDensity).toBe('compact');
    // Applying them is not a change to send back.
    await wait(SAVE_DELAY_MS + 200);
    expect(saves).toHaveLength(0);
    // They are kept under the user's name, not as the browser's own.
    expect(window.localStorage.getItem(`${BROWSER_KEY}@alice`)).toContain('"newest"');
    expect(window.localStorage.getItem(BROWSER_KEY)).toBeNull();
  });

  it('saves only the keys a change made, once changes stop', async () => {
    const saves = fakeServer({ alice: { imageOrder: 'oldest', gridThumbnailSize: 450 } }, () => 'alice');
    renderHook(() => useAccountPreferences('alice'));
    await waitFor(() => expect(getDisplayPreferences().gridThumbnailSize).toBe(450));

    act(() => setDisplayPreferences({ ...getDisplayPreferences(), imageOrder: 'newest' }));
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), imageGrouping: 'date' }));
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), showAllChip: false }));
    // Folding a detail panel stays in this browser.
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), detailSections: { sky: 'closed' } }));
    expect(saves).toHaveLength(0);
    await waitFor(() => expect(saves).toHaveLength(1), { timeout: SAVE_DELAY_MS * 4 });
    expect(saves[0].changes).toEqual({
      imageOrder: 'newest',
      imageGrouping: 'date',
      showAllChip: false,
    });
  });

  it('never undoes what another browser saved since this one loaded', async () => {
    const documents: Record<string, Document> = { alice: { imageOrder: 'newest' } };
    const saves = fakeServer(documents, () => 'alice');
    renderHook(() => useAccountPreferences('alice'));
    await waitFor(() => expect(getDisplayPreferences().imageOrder).toBe('newest'));

    // Hours later, on another device, Alice chooses Date grouping.
    documents.alice = { ...documents.alice, imageGrouping: 'date' };
    // This old tab changes a chip.
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), showAllChip: false }));
    await waitFor(() => expect(saves).toHaveLength(1), { timeout: SAVE_DELAY_MS * 4 });
    expect(saves[0].changes).toEqual({ showAllChip: false });
    expect(documents.alice).toEqual({ imageOrder: 'newest', imageGrouping: 'date', showAllChip: false });
  });

  it('sends nothing until a read succeeds, and tries the read again', async () => {
    const documents: Record<string, Document> = { alice: { imageOrder: 'newest', imageGrouping: 'session' } };
    const saves = fakeServer(documents, () => 'alice');
    let reads = 0;
    server.use(
      http.get('/api/me/preferences', () => {
        reads += 1;
        if (reads === 1) return HttpResponse.json({ success: false, error: 'busy' }, { status: 503 });
        return HttpResponse.json({
          success: true, error: null, status: 'ready',
          data: { schema_version: 1, preferences: documents.alice },
        });
      }),
    );
    renderHook(() => useAccountPreferences('alice'));
    await waitFor(() => expect(reads).toBe(1));

    // A change while the server could not answer: kept here, not sent.
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), showNightChip: false }));
    await wait(SAVE_DELAY_MS + 100);
    expect(saves).toHaveLength(0);
    expect(documents.alice).toEqual({ imageOrder: 'newest', imageGrouping: 'session' });

    // The read is tried again; then the change goes, and only it.
    await waitFor(() => expect(reads).toBe(2), { timeout: RETRY_FIRST_MS * 3 });
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].changes).toEqual({ showNightChip: false });
    expect(documents.alice).toEqual({ imageOrder: 'newest', imageGrouping: 'session', showNightChip: false });
    expect(getDisplayPreferences().imageOrder).toBe('newest');
    expect(getDisplayPreferences().showNightChip).toBe(false);
  });

  it('waits longer after each failure, up to a minute', () => {
    expect(retryDelay(1)).toBe(RETRY_FIRST_MS);
    expect(retryDelay(2)).toBe(RETRY_FIRST_MS * 2);
    expect(retryDelay(3)).toBe(RETRY_FIRST_MS * 4);
    expect(retryDelay(20)).toBe(60_000);
  });

  it('after a refusal, keeps the browser\'s copy and saves nothing', async () => {
    let patches = 0;
    server.use(
      http.get('/api/me/preferences', () => HttpResponse.json({ success: false }, { status: 404 })),
      http.patch('/api/me/preferences', () => {
        patches += 1;
        return HttpResponse.json({}, { status: 500 });
      }),
    );
    renderHook(() => useAccountPreferences('alice'));
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), imageOrder: 'newest' }));
    await wait(RETRY_FIRST_MS + SAVE_DELAY_MS);
    expect(patches).toBe(0);
    expect(getDisplayPreferences().imageOrder).toBe('newest');
  });

  it('keeps a change made while the document was on its way', async () => {
    const documents: Record<string, Document> = { alice: { imageOrder: 'oldest', imageGrouping: 'session' } };
    const saves = fakeServer(documents, () => 'alice');
    let answer: () => void = () => {};
    const answered = new Promise<void>((resolve) => { answer = resolve; });
    server.use(
      http.get('/api/me/preferences', async () => {
        await answered;
        return HttpResponse.json({
          success: true, error: null, status: 'ready',
          data: { schema_version: 1, preferences: documents.alice },
        });
      }),
    );
    renderHook(() => useAccountPreferences('alice'));
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), imageOrder: 'newest' }));
    await wait(50);
    answer();

    await waitFor(() => expect(getDisplayPreferences().imageGrouping).toBe('session'));
    // The document did not undo it...
    expect(getDisplayPreferences().imageOrder).toBe('newest');
    // ...and it is saved, alone.
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].changes).toEqual({ imageOrder: 'newest' });
    expect(documents.alice).toEqual({ imageOrder: 'newest', imageGrouping: 'session' });
  });

  it('hands this browser\'s own choices to the first account to sign in, and to no other', async () => {
    window.localStorage.setItem(BROWSER_KEY, JSON.stringify({ libraryDensity: 'detailed', imageOrder: 'newest' }));
    setPreferencesAccount(null);
    const documents: Record<string, Document> = {};
    let user = 'alice';
    const saves = fakeServer(documents, () => user);
    const app = render(<App user="alice" />);
    // Before the server answers, Alice already sees what this browser chose.
    expect(getDisplayPreferences().libraryDensity).toBe('detailed');
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toEqual({ user: 'alice', changes: { libraryDensity: 'detailed', imageOrder: 'newest' } });

    // Alice signs out; Bob, new too, signs in on the same browser.
    app.rerender(<App user={null} />);
    user = 'bob';
    app.rerender(<App user="bob" />);
    expect(getDisplayPreferences().libraryDensity).toBe('compact');
    await wait(SAVE_DELAY_MS + 200);
    expect(getDisplayPreferences().libraryDensity).toBe('compact');
    expect(getDisplayPreferences().imageOrder).toBe('oldest');
    expect(saves.filter((save) => save.user === 'bob')).toHaveLength(0);
    app.unmount();
  });

  it('retires this browser\'s choices when the first account has its own', async () => {
    window.localStorage.setItem(BROWSER_KEY, JSON.stringify({ libraryDensity: 'detailed' }));
    const documents: Record<string, Document> = { alice: { imageOrder: 'newest' } };
    let user = 'alice';
    const saves = fakeServer(documents, () => user);
    const app = render(<App user="alice" />);
    await waitFor(() => expect(getDisplayPreferences().imageOrder).toBe('newest'));
    expect(getDisplayPreferences().libraryDensity).toBe('compact');

    app.rerender(<App user={null} />);
    user = 'bob';
    app.rerender(<App user="bob" />);
    await wait(SAVE_DELAY_MS + 200);
    expect(getDisplayPreferences().libraryDensity).toBe('compact');
    expect(saves).toHaveLength(0);
    app.unmount();
  });

  it('starts a re-created account clean, setting aside the old account\'s copy here', async () => {
    // An earlier "alice" used this browser; the account was removed and
    // made again, and the new one has saved nothing.
    window.localStorage.setItem(`${BROWSER_KEY}@alice`, JSON.stringify({
      imageOrder: 'newest', imageGrouping: 'date', detailSections: { sky: 'closed' },
    }));
    window.localStorage.setItem(`${BROWSER_KEY}.handed-over`, '1');
    const saves = fakeServer({}, () => 'alice');
    render(<App user="alice" />);
    await waitFor(() => expect(getDisplayPreferences().imageOrder).toBe('oldest'));
    expect(getDisplayPreferences().imageGrouping).toBe('filter');
    expect(JSON.parse(window.localStorage.getItem(`${BROWSER_KEY}@alice`)!).imageGrouping).toBe('filter');
    // The panels folded in this browser are this browser's.
    expect(getDisplayPreferences().detailSections).toEqual({ sky: 'closed' });
    await wait(SAVE_DELAY_MS + 200);
    expect(saves).toHaveLength(0);
  });

  it('signing out unmounts the app: a waiting change is sent, and the next person sees none of it', async () => {
    const documents: Record<string, Document> = { alice: { imageOrder: 'newest' }, bob: {} };
    let user = 'alice';
    const saves = fakeServer(documents, () => user);
    window.localStorage.setItem(`${BROWSER_KEY}.handed-over`, '1');
    const app = render(<App user="alice" />);
    await waitFor(() => expect(getDisplayPreferences().imageOrder).toBe('newest'));

    // A change still waiting for its save when Alice signs out.
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), imageGrouping: 'session' }));
    app.rerender(<App user={null} />);
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toEqual({ user: 'alice', changes: { imageGrouping: 'session' } });
    // The sign-in form shows the browser's own choices, not hers.
    expect(getDisplayPreferences().imageOrder).toBe('oldest');
    expect(getDisplayPreferences().imageGrouping).toBe('filter');

    user = 'bob';
    app.rerender(<App user="bob" />);
    await wait(SAVE_DELAY_MS + 200);
    expect(getDisplayPreferences().imageOrder).toBe('oldest');
    expect(getDisplayPreferences().imageGrouping).toBe('filter');
    expect(saves).toHaveLength(1);
    expect(documents.bob).toEqual({});
    app.unmount();
  });

  it('leaves another tab\'s change for that tab to save', async () => {
    const saves = fakeServer({ alice: { imageOrder: 'oldest' } }, () => 'alice');
    renderHook(() => useAccountPreferences('alice'));
    await waitFor(() => expect(window.localStorage.getItem(`${BROWSER_KEY}@alice`)).not.toBeNull());

    // The other tab writes the shared copy; this one adopts it.
    window.localStorage.setItem(`${BROWSER_KEY}@alice`, JSON.stringify({ imageOrder: 'newest' }));
    act(() => {
      window.dispatchEvent(new StorageEvent('storage', { key: `${BROWSER_KEY}@alice` }));
    });
    expect(getDisplayPreferences().imageOrder).toBe('newest');
    await wait(SAVE_DELAY_MS + 200);
    expect(saves).toHaveLength(0);
  });

  it('with no account, keeps the browser\'s copy and asks the server nothing', async () => {
    let requests = 0;
    server.use(
      http.get('/api/me/preferences', () => {
        requests += 1;
        return HttpResponse.json({}, { status: 404 });
      }),
      http.patch('/api/me/preferences', () => {
        requests += 1;
        return HttpResponse.json({}, { status: 404 });
      }),
    );
    renderHook(() => useAccountPreferences(undefined));
    act(() => setDisplayPreferences({ ...getDisplayPreferences(), imageOrder: 'newest' }));
    await wait(SAVE_DELAY_MS + 200);
    expect(requests).toBe(0);
    expect(window.localStorage.getItem(BROWSER_KEY)).toContain('"newest"');
  });
});
