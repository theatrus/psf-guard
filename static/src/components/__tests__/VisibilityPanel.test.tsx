import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import VisibilityPanel from '../director/VisibilityPanel';
import { formatHours } from '../director/visibilityFormat';
import type { DirectorFeasibility, DirectorRigFeasibility } from '../../api/directorTypes';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
const noon = Date.UTC(2026, 8, 25, 19, 53);
function rig(name: string, id: string, hoursUp: number, custom: boolean, inPlan: boolean): DirectorRigFeasibility {
  const samples = [];
  for (let i = 0; i < 288; i++) {
    const t = noon + i * 300_000;
    const hour = i / 12;
    const sun = 40 * Math.cos(((hour - 12) / 24) * 2 * Math.PI) * -1;
    samples.push({ t_ms: t, sun_altitude_degrees: sun, moon_altitude_degrees: 30 * Math.sin((hour / 24) * 2 * Math.PI),
      targets: [{ altitude_degrees: 20 + 50 * Math.max(0, Math.sin(((hour - 4) / 20) * Math.PI)), azimuth_degrees: (hour * 15) % 360, horizon_altitude_degrees: custom ? 10 + 30 * (i % 2) : null, allowed: hoursUp > 0, meridian_blocked: custom && hour > 13 && hour < 14.5 }] });
  }
  const night = { date: '2026-09-25', noon_ms: noon, dusk_ms: noon + 7 * 3_600_000, dawn_ms: noon + 17 * 3_600_000, dark_hours: 10, moon_illumination: 0.96, moon_hours_up_in_dark: 6,
    targets: [{ id: 'center', hours_up: hoursUp, hours_up_moon_down: hoursUp / 2, hours_lost_to_meridian: custom ? 1.5 : 0, transit_ms: custom ? noon + 14 * 3_600_000 : null, max_altitude_degrees: hoursUp > 0 ? 71 : -5, min_moon_separation_degrees: 34 }] };
  return { rig: { id, name, revision: 1 }, catalog_name: name, site: { latitude_degrees: 34.2, longitude_degrees: -118.3, elevation_meters: 400 }, custom_horizon: custom,
    limits: { minimum_altitude_degrees: 25, maximum_altitude_degrees: 90, meridian_exclusion: { before_ms: 0, after_ms: 0 } },
    nights: [night, { ...night, date: '2026-09-26', moon_illumination: 0.9 }], curve: { night, samples }, hours_needed: inPlan ? 12 : null, nights_to_complete: inPlan && hoursUp > 0 ? 2 : null, in_plan: inPlan };
}
function mount(view: DirectorFeasibility, compact = false) {
  const bodies: unknown[] = [];
  server.use(http.post('/api/director/v1/projects/project/feasibility', async ({ request }) => { bodies.push(await request.json()); return ok(view); }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={client}><VisibilityPanel projectId="project" center={{ ra_degrees: 38.2, dec_degrees: 61.45 }} compact={compact} /></QueryClientProvider>);
  return bodies;
}

describe('visibility panel', () => {
  it('says how long the target is visible tonight, draws the chart, and estimates the nights owed', async () => {
    const bodies = mount({ center: { ra_degrees: 38.2, dec_degrees: 61.45 }, target_name: 'IC 1805', nights: 7, warnings: [],
      rigs: [rig('RedCat 61', 'a', 6.2, true, true), rig('C925 data', 'b', 0, false, false)] });
    expect(await screen.findByTestId('visibility-verdict')).toHaveTextContent('Visible 6.2 h tonight from RedCat 61 (10.0 h dark, peak 71°, 1.5 h lost to the meridian pause). Moon 96% lit, 34° away, up 6.0 h of the dark.');
    expect(document.querySelector('.visibility-chart .paused')?.getAttribute('d')).toMatch(/^M/);
    expect(document.querySelector('.visibility-chart .transit')).toBeInTheDocument();
    expect(bodies[0]).toEqual({ center: { ra_degrees: 38.2, dec_degrees: 61.45 } });
    expect(screen.getByRole('img', { name: 'Altitude of the target tonight at RedCat 61' })).toBeInTheDocument();
    expect(document.querySelector('.visibility-chart .target')?.getAttribute('d')).toMatch(/^M/);
    expect(document.querySelector('.visibility-chart .horizon')?.getAttribute('d')).toMatch(/^M/);
    expect(document.querySelectorAll('.visibility-chart .band-dark, .visibility-chart .band-deep').length).toBeGreaterThan(0);
    expect(screen.getByText(/custom horizon and limit/)).toBeInTheDocument();
    expect(screen.getByTestId('visibility-estimate')).toHaveTextContent("owes the plan 12.0 h; at this week's rate that is about 2 nights.");
    expect(screen.getAllByRole('row')).toHaveLength(3);
    fireEvent.change(screen.getByLabelText('Visibility rig'), { target: { value: 'b' } });
    expect(await screen.findByTestId('visibility-verdict')).toHaveTextContent('Not visible tonight from C925 data: never above 25° while dark.');
    expect(screen.getByText(/minimum altitude 25°/)).toBeInTheDocument();
    expect(screen.queryByTestId('visibility-estimate')).not.toBeInTheDocument();
  });

  it('folds the nights table away under the framing stage and keeps the verdict, chart and estimate', async () => {
    mount({ center: { ra_degrees: 38.2, dec_degrees: 61.45 }, target_name: 'IC 1805', nights: 7, warnings: [], rigs: [rig('RedCat 61', 'a', 6.2, true, true)] }, true);
    expect(await screen.findByTestId('visibility-verdict')).toHaveTextContent('Visible 6.2 h tonight');
    expect(document.querySelector('.visibility.is-compact')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: 'Altitude of the target tonight at RedCat 61' }).getAttribute('viewBox')).toBe('0 0 720 132');
    expect(screen.getByTestId('visibility-estimate')).toBeInTheDocument();
    const more = document.querySelector('details.visibility-more')!;
    expect(more).not.toHaveAttribute('open');
    expect(more.querySelector('summary')).toHaveTextContent('Legend and the next 2 nights');
    expect(more.querySelector('.visibility-legend')).toBeInTheDocument();
    expect(more.querySelectorAll('tbody tr')).toHaveLength(2);
  });

  it('shows the server warnings when no rig has a site', async () => {
    mount({ center: { ra_degrees: 1, dec_degrees: 1 }, target_name: 'Target', nights: 7, rigs: [], warnings: ['No rig has a site yet, so nothing can be timed.'] });
    expect(await screen.findByText('No rig has a site yet, so nothing can be timed.')).toBeInTheDocument();
    expect(formatHours(0)).toBe('none');
    expect(formatHours(0.5)).toBe('30 min');
  });
});
