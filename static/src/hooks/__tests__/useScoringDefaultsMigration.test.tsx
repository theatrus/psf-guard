import { afterEach, describe, expect, it } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import { QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import type { ReactNode } from 'react';
import { server } from '../../test/msw-server';
import { scoringServer, testQueryClient, waitForScoringDefaults } from '../../test/scoringDefaults';
import { useScoringDefaultsMigration } from '../useScoringDefaultsMigration';
import { getDisplayPreferences, setDisplayPreferences } from '../useDisplayPreferences';
import {
  fieldsFromScoring,
  SCORING_DEFAULTS as DEFAULTS,
  stackScoringSettings,
  type ScoringPreferences,
} from '../../utils/scoring';

const BROWSER: ScoringPreferences = { ...DEFAULTS, satellite: 0, hfrRejectAbove: 3 };
const ACCOUNT: ScoringPreferences = { ...DEFAULTS, temporal: 0.5, starCountRejectBelow: 40 };

/** This browser's own scoring defaults, as the display preferences kept
 *  them before the server did. */
function setBrowserScoring(scoring: ScoringPreferences) {
  setDisplayPreferences({ ...getDisplayPreferences(), ...fieldsFromScoring(scoring) });
}

/** The signed-in person's account document, and how often it is read. */
function mockAccount(scoring: ScoringPreferences) {
  const reads = { count: 0 };
  server.use(
    http.get('/api/me/preferences', () => {
      reads.count += 1;
      return HttpResponse.json({
        success: true,
        data: { schema_version: 1, preferences: fieldsFromScoring(scoring) },
        error: null,
      });
    }),
  );
  return reads;
}

function renderMigration(props: { username?: string; canWrite?: boolean } = {}) {
  const client = testQueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const view = renderHook(
    ({ username, canWrite }: { username?: string; canWrite: boolean }) =>
      useScoringDefaultsMigration(username, canWrite),
    { wrapper, initialProps: { username: props.username, canWrite: props.canWrite ?? true } },
  );
  return { client, ...view };
}

/** An account document that arrives only when the test calls `release`. */
function mockSlowAccount(scoring: ScoringPreferences) {
  const reads = { count: 0 };
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  server.use(
    http.get('/api/me/preferences', async () => {
      reads.count += 1;
      await gate;
      return HttpResponse.json({
        success: true,
        data: { schema_version: 1, preferences: fieldsFromScoring(scoring) },
        error: null,
      });
    }),
  );
  return { reads, release };
}

/** Give a copy that was going to be saved time to reach the server. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 50));

afterEach(() => setBrowserScoring(DEFAULTS));

describe('copying a person\'s own scoring defaults to the server', () => {
  it('saves the browser\'s defaults when the server has none', async () => {
    scoringServer.set(DEFAULTS, false);
    setBrowserScoring(BROWSER);
    const { client } = renderMigration();
    await waitFor(() => expect(scoringServer.saves).toEqual([BROWSER]));
    // Only while the server has no copy: a copy saved first wins.
    expect(scoringServer.puts).toEqual([{ value: BROWSER, onlyIfUnstored: true }]);
    // The page then has the copy, stored.
    await waitFor(() => expect(client.getQueryData(['scoring-defaults'])).toMatchObject({ stored: true }));
  });

  it('saves a signed-in person\'s account defaults, not the browser\'s', async () => {
    scoringServer.set(DEFAULTS, false);
    setBrowserScoring(BROWSER);
    const reads = mockAccount(ACCOUNT);
    renderMigration({ username: 'ann' });
    await waitFor(() => expect(scoringServer.saves).toEqual([ACCOUNT]));
    expect(scoringServer.puts).toEqual([{ value: ACCOUNT, onlyIfUnstored: true }]);
    expect(reads.count).toBe(1);
  });

  it('saves the built-in values too, so the first editor decides', async () => {
    scoringServer.set(DEFAULTS, false);
    setBrowserScoring(BROWSER);
    const reads = mockAccount(DEFAULTS);
    const { client } = renderMigration({ username: 'ann' });
    await waitFor(() => expect(scoringServer.saves).toEqual([DEFAULTS]));
    expect(scoringServer.puts).toEqual([{ value: DEFAULTS, onlyIfUnstored: true }]);
    expect(reads.count).toBe(1);
    await waitFor(() => expect(client.getQueryData(['scoring-defaults'])).toMatchObject({ stored: true }));
  });

  it('takes the server\'s copy when another page saved first', async () => {
    // The page read the server before another editor's page saved a copy.
    const OTHER: ScoringPreferences = { ...DEFAULTS, pointing: 0 };
    scoringServer.set(OTHER, true);
    server.use(
      http.get('/api/settings/scoring', () => HttpResponse.json({
        success: true,
        data: {
          scoring: stackScoringSettings(DEFAULTS),
          builtin: stackScoringSettings(DEFAULTS),
          stored: false,
        },
        error: null,
      }), { once: true }),
    );
    setBrowserScoring(BROWSER);
    const { client } = renderMigration();
    await waitFor(() => expect(scoringServer.puts).toEqual([{ value: BROWSER, onlyIfUnstored: true }]));
    await waitFor(() => expect(client.getQueryData(['scoring-defaults'])).toMatchObject({
      scoring: stackScoringSettings(OTHER),
      stored: true,
    }));
    expect(scoringServer.saves).toEqual([]);
    expect(scoringServer.get()).toEqual(OTHER);
  });

  it('saves nothing, and reads nothing, when the server already has defaults', async () => {
    scoringServer.set(DEFAULTS, true);
    setBrowserScoring(BROWSER);
    const reads = mockAccount(ACCOUNT);
    const { client } = renderMigration({ username: 'ann' });
    await waitForScoringDefaults(client);
    await settle();
    expect(reads.count).toBe(0);
    expect(scoringServer.puts).toEqual([]);
  });

  it('saves nothing for a person who cannot change settings', async () => {
    scoringServer.set(DEFAULTS, false);
    setBrowserScoring(BROWSER);
    const reads = mockAccount(ACCOUNT);
    const { client } = renderMigration({ username: 'ann', canWrite: false });
    await waitForScoringDefaults(client);
    await settle();
    expect(reads.count).toBe(0);
    expect(scoringServer.puts).toEqual([]);
  });

  it('copies once, even when re-rendered', async () => {
    scoringServer.set(DEFAULTS, false);
    setBrowserScoring(BROWSER);
    const { rerender } = renderMigration();
    await waitFor(() => expect(scoringServer.saves).toHaveLength(1));
    rerender({ username: undefined, canWrite: true });
    await settle();
    expect(scoringServer.puts).toHaveLength(1);
  });

  it('tries once, even after a failed save leaves the server without defaults', async () => {
    scoringServer.set(DEFAULTS, false);
    scoringServer.failure = { status: 500 };
    const reads = mockAccount(ACCOUNT);
    const { rerender } = renderMigration({ username: 'ann' });
    await waitFor(() => expect(scoringServer.puts).toHaveLength(1));
    await settle();
    // The page still finds the server without defaults, and the hook runs
    // again for another reason, such as a change of access.
    scoringServer.failure = null;
    rerender({ username: 'ann', canWrite: false });
    rerender({ username: 'ann', canWrite: true });
    await settle();
    expect(reads.count).toBe(1);
    expect(scoringServer.puts).toHaveLength(1);
    expect(scoringServer.saves).toEqual([]);
  });

  it('tries again when stopped part way, as when access changes', async () => {
    scoringServer.set(DEFAULTS, false);
    const { reads, release } = mockSlowAccount(ACCOUNT);
    const { rerender } = renderMigration({ username: 'ann' });
    await waitFor(() => expect(reads.count).toBe(1));
    // Access changes while the account document is on its way.
    rerender({ username: 'ann', canWrite: false });
    rerender({ username: 'ann', canWrite: true });
    await waitFor(() => expect(reads.count).toBe(2));
    release();
    await waitFor(() => expect(scoringServer.saves).toEqual([ACCOUNT]));
    await settle();
    expect(scoringServer.puts).toHaveLength(1);
  });
});
