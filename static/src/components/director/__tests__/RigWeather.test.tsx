import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../../test/msw-server';
import type { RigWeather } from '../../../api/weatherTypes';
import RigWeatherPanel, { RigWeatherSummary } from '../RigWeather';

const now = 1_800_000_000_000;
const weather: RigWeather = {
  rig_id: 'rig', site_id: 'site', evaluated_at_ms: now, cloud_threshold_percent: 30,
  reported: { rig_id: 'rig', received_at_ms: now, sample: { sample_id: 'sample', profile_id: 'profile', source_name: 'Cloud sensor', source_key: 'hash', reported_at_ms: now, connected: true,
    values: { cloud_percent: 85, temperature_c: 7.5, dew_point_c: null, humidity_percent: null, pressure_hpa: null, rain_mm_per_hour: null, sky_brightness_lux: null, sky_quality_mag_arcsec2: null, sky_temperature_c: null, seeing_arcsec: null, wind_direction_degrees: null, wind_speed_m_s: 3.25, wind_gust_m_s: null } } },
  forecast: { provider: 'open_meteo', attribution: 'Weather data by Open-Meteo.com (CC BY 4.0)', fetched_at_ms: now, expires_at_ms: now + 3600000, fresh: true,
    hour: { start_ms: now, end_ms: now + 3600000, cloud_total: 10, cloud_low: null, cloud_mid: null, cloud_high: null } },
  reported_cloud: { state: 'cloudy', reason: 'cloud_cover_above_threshold', valid_for_ms: 30000 },
  forecast_cloud: { state: 'clear', reason: 'cloud_cover_clear', valid_for_ms: 30000 },
};

describe('rig weather', () => {
  it('keeps reported clouds and a contradictory forecast visible with attribution', () => {
    render(<RigWeatherSummary weather={weather} now={now} />);
    expect(screen.getByText(/85% cloud · cloudy/)).toBeInTheDocument();
    expect(screen.getByText(/Open-Meteo · 10% cloud · clear/)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /CC BY 4.0/ })).toHaveAttribute('href', 'https://open-meteo.com/');
  });
  it('expires a cached clear forecast and keeps missing cloud cover unknown', () => {
    render(<RigWeatherSummary weather={{ ...weather, reported: { ...weather.reported!, sample: { ...weather.reported!.sample, values: { ...weather.reported!.sample.values, cloud_percent: null } } } }} now={now + 30000} />);
    expect(screen.getAllByText(/unknown or stale/)).toHaveLength(2);
    expect(screen.queryByText(/Cloud sensor · 0% cloud/)).not.toBeInTheDocument();
  });
  it('loads telemetry and history only for the selected rig and preserves unsupported metrics', async () => {
    let histories = 0;
    server.use(
      http.get('/api/director/v1/rigs/rig/weather', () => HttpResponse.json({ success: true, data: weather })),
      http.get('/api/director/v1/rigs/rig/weather/history', () => { histories++; return HttpResponse.json({ success: true, data: [weather.reported] }); }),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><RigWeatherPanel rig="rig" /></QueryClientProvider>);
    expect(await screen.findByText('7.5 °C')).toBeInTheDocument();
    expect(histories).toBe(0);
    const details = screen.getByText('Reported weather history (24 hours)').closest('details')!;
    details.open = true; fireEvent(details, new Event('toggle'));
    expect(await screen.findByRole('columnheader', { name: 'Reported' })).toBeInTheDocument();
    expect(histories).toBe(1);
  });
});
