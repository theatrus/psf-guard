import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { CloudAssessment, RigWeather } from '../../api/weatherTypes';
import { useDirectorClock } from '../../hooks/useDirectorClock';
import { formatAge } from './dashboardModel';
import { PROVIDER_LABEL, PROVIDER_URL } from './forecastLabels';

const number = (value: number | null | undefined, suffix: string) => value == null || !Number.isFinite(value) ? '—' : `${value.toFixed(1).replace(/\.0$/, '')}${suffix}`;
const state = (cloud: CloudAssessment, evaluated: number, now: number) => {
  if (cloud.state === 'unknown') {
    if (cloud.reason === 'forecast_location_mismatch') return 'location mismatch';
    if (cloud.reason === 'forecast_location_unavailable') return 'location unavailable';
    return 'unknown or stale';
  }
  return now < evaluated || now >= evaluated + cloud.valid_for_ms ? 'unknown or stale' : cloud.state === 'dewy' ? 'near dew point' : cloud.state;
};

export function RigWeatherSummary({ weather, now }: { weather?: RigWeather; now: number }) {
  if (!weather) return <span className="director-muted">Weather not reported</span>;
  const r = weather.reported;
  const f = weather.forecast;
  return <div aria-label="Rig weather">
    <div><strong>NINA reported:</strong> {r ? <>{r.sample.source_name}{!r.sample.connected && ' (disconnected)'} · {number(r.sample.values.cloud_percent, '% cloud')} · {state(weather.reported_cloud, weather.evaluated_at_ms, now)}<span className="director-muted"> · {formatAge(now - r.sample.reported_at_ms)}</span></> : 'unavailable'}</div>
    <div><strong>Forecast:</strong> {f ? <>{PROVIDER_LABEL[f.provider]} · {number(f.hour?.cloud_total, '% cloud')} · {state(weather.forecast_conditions, weather.evaluated_at_ms, now)} <small className="director-muted"><a href={PROVIDER_URL[f.provider]} target="_blank" rel="noreferrer">{f.attribution}</a></small></> : 'unavailable'}</div>
  </div>;
}

export default function RigWeatherPanel({ rig }: { rig: string }) {
  const now = useDirectorClock();
  const [historyOpen, setHistoryOpen] = useState(false);
  const current = useQuery({ queryKey: ['directorWeather', rig], queryFn: () => apiClient.getDirectorWeather(rig), refetchInterval: 15_000, refetchOnWindowFocus: false });
  const history = useQuery({ queryKey: ['directorWeatherHistory', rig], queryFn: () => apiClient.getDirectorWeatherHistory(rig), enabled: historyOpen, refetchOnWindowFocus: false });
  const weather = current.data;
  const values = weather?.reported?.sample.values;
  return <section aria-label="Weather telemetry">
    <div className="director-toolbar"><h4>Weather</h4><button type="button" disabled={current.isFetching} onClick={() => { void current.refetch(); if (historyOpen) void history.refetch(); }}>Refresh weather</button></div>
    {current.isPending && <p role="status">Loading weather...</p>}
    {current.isError && <p role="alert">Weather could not be loaded. {current.error.message}</p>}
    {weather && <>
      <RigWeatherSummary weather={weather} now={now} />
      {weather.forecast_conditions.reason === 'forecast_location_mismatch' && <p role="status">The site's forecast does not match this rig's location. In the Site tab, select the site where the rig is located and check the rig's latitude and longitude.</p>}
      {weather.forecast_conditions.reason === 'forecast_location_unavailable' && <p role="status">The forecast location could not be checked. Set the planning site's coordinates under Settings → Presets → Sites and check the rig's location in its Site tab.</p>}
      <p>Clear at or under {weather.cloud_threshold_percent}% cloud. Configure the provider and threshold in Settings → Presets → Sites.</p>
      {weather.dew_limit_celsius != null && <p>Forecasts also require air more than {weather.dew_limit_celsius} °C above its dew point, using the site's weather rule.</p>}
      {weather.forecast?.hour && <p>Forecast air {number(weather.forecast.hour.temperature_celsius, ' °C')} · dew point {number(weather.forecast.hour.dew_point_celsius, ' °C')} · humidity {number(weather.forecast.hour.humidity_percent, '%')}</p>}
      <p className="director-muted">NINA's report time records when it supplied the values. Its source may be a sensor or a weather service; the source's measurement time is unavailable. Forecasts remain predictions.</p>
      {values && <dl className="director-weather-values">
        <dt>Temperature</dt><dd>{number(values.temperature_celsius, ' °C')}</dd>
        <dt>Dew point</dt><dd>{number(values.dew_point_celsius, ' °C')}</dd>
        <dt>Humidity</dt><dd>{number(values.humidity_percent, '%')}</dd>
        <dt>Pressure</dt><dd>{number(values.pressure_hpa, ' hPa')}</dd>
        <dt>Wind</dt><dd>{number(values.wind_speed_m_s, ' m/s')} · gust {number(values.wind_gust_m_s, ' m/s')}</dd>
        <dt>Rain rate</dt><dd>{number(values.rain_mm_per_hour, ' mm/h')}</dd>
        <dt>Sky temperature</dt><dd>{number(values.sky_temperature_celsius, ' °C')}</dd>
        <dt>Sky quality</dt><dd>{number(values.sky_quality_mag_arcsec2, ' mag/arcsec²')}</dd>
      </dl>}
      {weather.forecast && <p className="director-muted">Forecast fetched {formatAge(now - weather.forecast.fetched_at_ms)}</p>}
    </>}
    <details onToggle={event => setHistoryOpen(event.currentTarget.open)}><summary>Reported weather history (24 hours)</summary>
      {history.isPending && historyOpen && <p role="status">Loading weather history...</p>}
      {history.isError && <p role="alert">Weather history could not be loaded.</p>}
      {history.data?.length === 0 && <p>No weather reports in the last 24 hours.</p>}
      {!!history.data?.length && <div className="director-table-scroll"><table><thead><tr><th>Reported</th><th>Source</th><th>Cloud</th><th>Temperature</th><th>Humidity</th><th>Wind</th></tr></thead>
        <tbody>{history.data.map(row => <tr key={row.sample.sample_id}><td>{new Date(row.sample.reported_at_ms).toLocaleString()}</td><td>{row.sample.source_name}{!row.sample.connected && ' (disconnected)'}</td><td>{number(row.sample.values.cloud_percent, '%')}</td><td>{number(row.sample.values.temperature_celsius, ' °C')}</td><td>{number(row.sample.values.humidity_percent, '%')}</td><td>{number(row.sample.values.wind_speed_m_s, ' m/s')}</td></tr>)}</tbody></table></div>}
    </details>
  </section>;
}
