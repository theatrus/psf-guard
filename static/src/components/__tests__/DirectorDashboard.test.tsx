import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import DirectorDashboard from '../director/DirectorDashboard';
import { describeNow, formatAge, isExposing } from '../director/dashboardModel';
import type { DirectorRigStatusView } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const NOW = 1_800_000_000_000;
const rig = (id: string, name: string): DirectorRigStatusView['rig'] => ({ id, name, revision: 1 });
const project = { id: 'pppppppp-pppp-4ppp-8ppp-pppppppppppp', name: 'Heart Nebula', revision: 1 };
const online: DirectorRigStatusView = {
  rig: rig('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', 'RedCat'), catalog_slug: 'redcat', catalog_name: 'RedCat 61',
  status: { rig_id: 'a', session_id: 's1', reported_at_ms: NOW - 40_000, received_at_ms: NOW - 40_000,
    payload: { phase: 'exposing', target_name: 'IC 1805 r1c1', operation: 'exposure', operation_started_ms: NOW - 200_000, queue_depth: 2, errors: ['guider lost the star once'] } },
  status_age_ms: 40_000, status_stale: false, checkins: [],
  contacts: { program_pull: { at_ms: NOW - 3_600_000, detail: 'abcdef0123456789' }, check_in: { at_ms: NOW - 120_000, detail: 'ledger-1' }, status: { at_ms: NOW - 40_000, detail: 's1' } },
  connectivity: { state: 'online', last_contact_ms: NOW - 40_000, age_ms: 40_000 }, assignments: [{ project, activation_revision: 2, applied_at_ms: NOW - 86_400_000 }], pending_receipts: 5,
};
const quiet: DirectorRigStatusView = {
  rig: rig('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', 'C925'), catalog_slug: 'c925', catalog_name: 'C925',
  status: { rig_id: 'b', session_id: 's9', reported_at_ms: NOW - 3_000_000, received_at_ms: NOW - 3_000_000, payload: { phase: 'exposing' } },
  status_age_ms: 3_000_000, status_stale: true, checkins: [],
  contacts: { program_pull: null, check_in: null, status: { at_ms: NOW - 3_000_000, detail: 's9' } },
  connectivity: { state: 'offline', last_contact_ms: NOW - 3_000_000, age_ms: 3_000_000 }, assignments: [], pending_receipts: 0,
};
const never: DirectorRigStatusView = {
  rig: rig('cccccccc-cccc-4ccc-8ccc-cccccccccccc', 'Desert'), catalog_slug: 'desert', catalog_name: 'Desert copy', status: null, status_age_ms: null, status_stale: false, checkins: [],
  contacts: { program_pull: null, check_in: null, status: null }, connectivity: { state: 'never', last_contact_ms: null, age_ms: null }, assignments: [], pending_receipts: 0,
};

function mount(rows: DirectorRigStatusView[]) {
  server.use(http.get('/api/director/v1/rigs/status', () => HttpResponse.json(ok(rows))));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={client}><MemoryRouter initialEntries={['/director?db=redcat']}>{children}</MemoryRouter></QueryClientProvider>;
  }
  return render(<DirectorDashboard />, { wrapper: Wrapper });
}

describe('Director live dashboard', () => {
  beforeEach(() => { vi.useFakeTimers({ toFake: ['Date'] }); vi.setSystemTime(NOW); });
  afterEach(() => vi.useRealTimers());

  it('shows each rig with its connectivity, what it is doing, contact ages and assignments', async () => {
    mount([online, quiet, never]);
    expect(await screen.findByTestId('director-dashboard')).toBeInTheDocument();
    expect(screen.getByText('Online')).toBeInTheDocument();
    expect(screen.getByText('exposing, IC 1805 r1c1, exposure for 3 min, 2 queued')).toBeInTheDocument();
    expect(screen.getByText('guider lost the star once')).toBeInTheDocument();
    expect(screen.getByText('5 saved, ungraded')).toBeInTheDocument();
    expect(screen.getByText('rev abcdef01')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Heart Nebula' })).toHaveAttribute('href', `/plan?db=redcat&plan=${project.id}`);
    // An old report is history, not the present.
    expect(screen.getByText('Offline')).toBeInTheDocument();
    expect(screen.getByText('exposing (stale, reported 50 min ago)')).toBeInTheDocument();
    expect(screen.getByText('Never seen')).toBeInTheDocument();
    expect(screen.getByText('No report yet')).toBeInTheDocument();
    expect(screen.getAllByText('never')).toHaveLength(5);
    expect(screen.getAllByText('nothing activated')).toHaveLength(2);
  });

  it('marks live status as the Director plugin early beta, and names Target Scheduler as the main way to run a rig', async () => {
    mount([online]);
    expect(await screen.findByRole('heading', { name: 'Live' })).toBeInTheDocument();
    expect(screen.getByText('Early beta')).toHaveAttribute('title', expect.stringContaining('Target Scheduler with PSF Guard Sync is the main way to run a rig'));
  });

  it('credits the forecast provider once, at the foot of the page', async () => {
    const forecast = { provider: 'open_meteo' as const, attribution: 'Weather data by Open-Meteo.com (CC BY 4.0)', fetched_at_ms: NOW, expires_at_ms: NOW + 3_600_000, fresh: true,
      hour: { start_ms: NOW, end_ms: NOW + 3_600_000, cloud_total: 10, cloud_low: null, cloud_mid: null, cloud_high: null, temperature_celsius: 7, dew_point_celsius: 6, humidity_percent: 93 } };
    const weather = (rig_id: string) => ({ rig_id, site_id: 'site', evaluated_at_ms: NOW, cloud_threshold_percent: 30, dew_limit_celsius: 2, forecast, reported: null,
      reported_cloud: { state: 'unknown' as const, reason: 'no_report', valid_for_ms: 30_000 },
      forecast_conditions: { state: 'clear' as const, reason: 'cloud_cover_clear', valid_for_ms: 30_000 } });
    mount([{ ...online, weather: weather('a') }, { ...quiet, weather: weather('b') }]);
    expect(await screen.findAllByText(/Open-Meteo · 10% cloud/)).toHaveLength(2);
    const credits = screen.getAllByRole('link', { name: forecast.attribution });
    expect(credits).toHaveLength(1);
    expect(screen.getByTestId('forecast-credits')).toContainElement(credits[0]);
  });

  it('shows what the Sync plugin last did and what N.I.N.A. is doing', async () => {
    const sync = { last_contact_ms: NOW - 60_000, plugin_version: '1.5.0.0', sequence_stale: false,
      latest_action: { at_ms: NOW - 180_000, action: 'apply', outcome: 'ok' as const, operation: 'merge', summary: { total_updated: 4 } },
      sequence: { session_id: 's', reported_at_ms: NOW - 1_000, received_at_ms: NOW - 1_000, fresh_for_ms: 45_000, running: true,
        activity: { kind: 'exposing', target_name: 'M 31', exposure_seconds: 300, filter: 'L', completed: 3, total: 30 }, recent: {} } };
    mount([{ ...never, sync }, { ...online, sync: { ...sync, latest_action: null } }, quiet]);
    const rows = await screen.findAllByTestId('sync-plugin');
    expect(rows[0]).toHaveTextContent("Merged N.I.N.A.'s catalog: 4 updated 3 min ago");
    expect(rows[0]).toHaveTextContent('contact 1 min ago, plugin 1.5.0.0');
    expect(rows[1]).toHaveTextContent('no action yet');
    expect(rows[2]).toHaveTextContent('not used');
    const sequences = screen.getAllByTestId('sync-sequence');
    // Without its own report, Desert's row shows the Sync plugin's alone.
    expect(sequences[0]).toHaveTextContent(/^Exposing M 31: 300 s L, 3 of 30$/);
    expect(screen.queryByText('No report yet')).toBeNull();
    // RedCat's Director report stays first; the Sync plugin's follows.
    expect(sequences[1]).toHaveTextContent(/^Sync plugin: Exposing M 31/);
    expect(screen.getByText('exposing, IC 1805 r1c1, exposure for 3 min, 2 queued')).toBeInTheDocument();
  });

  it('says when no rig exists yet', async () => {
    mount([]);
    expect(await screen.findByText(/No rigs yet/)).toBeInTheDocument();
  });

  it('formats ages and reports plainly', () => {
    expect(formatAge(20_000)).toBe('20 s ago');
    expect(formatAge(15 * 60_000)).toBe('15 min ago');
    expect(formatAge(5 * 3_600_000)).toBe('5 h ago');
    expect(formatAge(3 * 86_400_000)).toBe('3 d ago');
    expect(describeNow({ ...never, connectivity: { state: 'online', last_contact_ms: NOW, age_ms: 0 } }, NOW)).toBe('No status report yet');
    expect(describeNow({ ...online, status: { ...online.status!, payload: { state: 'idle', wait_reason: 'clouds', safety: 'unsafe' } } }, NOW)).toBe('idle, waiting: clouds, safety unsafe');
  });

  it('freezes stale operation duration and keeps replay history distinct from current status', async () => {
    const row: DirectorRigStatusView = { ...quiet, status: { ...quiet.status!, payload: {
      phase: 'completed', operation: 'Autofocus', operation_elapsed_ms: 120_000, queue_state: 'Deferred',
    } }, recent_operations: [{ received_at_ms: NOW - 1000, event: { ledger_id: 'ledger', sequence: 1,
      preparation_id: 'prep', event: { kind: 'completed', observation: {
        command: { target_id: 'target', operation: { operation: 'before_target' } },
        completion: { ended_at_ms: NOW - 3_600_000, elapsed_ms: 90000, outcome: { outcome: 'succeeded' } },
      } } } }] };
    expect(describeNow(row, NOW)).toContain('Autofocus for 2 min');
    expect(describeNow(row, NOW + 600_000)).toContain('Autofocus for 2 min');
    mount([row]);
    expect(await screen.findByText('Completed operations (1)')).toBeInTheDocument();
    expect(screen.getByText(/before target: succeeded, 90 s/)).toBeInTheDocument();
    expect(screen.getByText(/completed 1 h ago · received 1 s ago/)).toBeInTheDocument();
  });

  it('expires an explicit lease even when the cached server response was fresh', () => {
    const row = { ...online, status: { ...online.status!, payload: { phase: 'exposing', fresh_for_ms: 45000,
      operation: 'Exposure', operation_elapsed_ms: 10000 } } };
    expect(isExposing(row, NOW)).toBe(true);
    expect(isExposing(row, NOW + 6000)).toBe(false);
    expect(describeNow(row, NOW + 6000)).toContain('Exposure for 10 s (stale');
  });
});
