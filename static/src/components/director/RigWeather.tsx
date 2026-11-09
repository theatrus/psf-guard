import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { CloudAssessment, RigWeather } from '../../api/weatherTypes';
import { useDirectorClock } from '../../hooks/useDirectorClock';
import { formatAge } from './dashboardModel';
import { PROVIDER_LABEL, PROVIDER_URL } from './forecastLabels';
import { openSettings } from '../../utils/settingsIntent';

const number = (value: number | null | undefined, suffix: string) => value == null || !Number.isFinite(value) ? '—' : `${value.toFixed(1).replace(/\.0$/, '')}${suffix}`;
/** The page clock ticks every few seconds and the request took time too,
 *  so a lease is treated as over this much early: an expired decision
 *  never shows as current. */
const LEASE_MARGIN_MS = 5_000;

/** The server's time now, from its evaluation of `weather` plus the time
 *  since this browser received it. The two clocks are never compared.
 *  `null` when the browser clock has jumped back past the fetch, which
 *  leaves the reading's age unknown. */
export function serverNow(weather: RigWeather, browserNow: number, receivedAt: number): number | null {
  const elapsed = browserNow - receivedAt;
  // The page clock may lag the fetch by one tick; more is a jump.
  if (elapsed < -LEASE_MARGIN_MS) return null;
  return weather.evaluated_at_ms + Math.max(0, elapsed);
}

const state = (cloud: CloudAssessment, evaluated: number, now: number | null) => {
  if (cloud.state === 'unknown') {
    if (cloud.reason === 'weather_source_disconnected') return 'disconnected';
    if (cloud.reason === 'forecast_location_mismatch') return 'location mismatch';
    if (cloud.reason === 'forecast_location_unavailable') return 'location unavailable';
    return 'unknown or stale';
  }
  return now === null || now < evaluated || now >= evaluated + cloud.valid_for_ms - LEASE_MARGIN_MS ? 'unknown or stale' : cloud.state === 'dewy' ? 'near dew point' : cloud.state;
};

/** A rig's reported and forecast weather. `compact`, for a table cell,
 *  marks them Observed and Forecast without naming the sensor or provider
 *  (a site has one forecast), and leaves out what is unavailable and the
 *  provider's credit, which the page gives once. `receivedAt` is when this browser fetched `weather`. The
 *  server's times are on its own clock, so ages and leases count from its
 *  evaluation plus the time since the fetch, never from this clock. */
export function RigWeatherSummary({ weather, now: browserNow, receivedAt, compact = false }: { weather?: RigWeather; now: number; receivedAt: number; compact?: boolean }) {
  const r = weather?.reported;
  const f = weather?.forecast;
  const now = weather ? serverNow(weather, browserNow, receivedAt) : null;
  // The server stamped the report when it arrived, on its own clock.
  const reportAge = (received: number) => now === null ? '' : ` · ${formatAge(now - received)}`;
  if (!weather && !compact) return <span className="director-muted">Weather not reported</span>;
  if (!weather || compact && !r && !f) return <span className="director-muted live-no-weather">No weather</span>;
  if (compact) return <div aria-label="Rig weather" className="live-weather">
    {r && <div title={`Reported by NINA from ${r.sample.source_name}`}><span className="director-muted">Observed</span> {r.sample.connected ? `${number(r.sample.values.cloud_percent, '% cloud')} · ` : ''}{state(weather.reported_cloud, weather.evaluated_at_ms, now)}<span className="director-muted">{reportAge(r.received_at_ms)}</span></div>}
    {f && <div title={`The site's ${PROVIDER_LABEL[f.provider]} forecast`}><span className="director-muted">Forecast</span> {number(f.hour?.cloud_total, '% cloud')} · {state(weather.forecast_conditions, weather.evaluated_at_ms, now)}</div>}
  </div>;
  return <div aria-label="Rig weather">
    <div><strong>NINA reported:</strong> {r ? <>{r.sample.source_name}{!r.sample.connected && ' (disconnected)'} · {number(r.sample.values.cloud_percent, '% cloud')} · {state(weather.reported_cloud, weather.evaluated_at_ms, now)}<span className="director-muted">{reportAge(r.received_at_ms)}</span></> : 'unavailable'}</div>
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
  const forecastNow = weather ? serverNow(weather, now, current.dataUpdatedAt) : null;
  return <section aria-label="Weather telemetry">
    <div className="director-toolbar"><h4>Weather</h4><button type="button" disabled={current.isFetching} onClick={() => { void current.refetch(); if (historyOpen) void history.refetch(); }}>Refresh weather</button></div>
    {current.isPending && <p role="status">Loading weather...</p>}
    {current.isError && <p role="alert">Weather could not be loaded. {current.error.message}</p>}
    {weather && <>
      <RigWeatherSummary weather={weather} now={now} receivedAt={current.dataUpdatedAt} />
      {weather.forecast_conditions.reason === 'forecast_location_mismatch' && <p role="status">The site's forecast does not match this rig's location. In the Site tab, select the site where the rig is located and check the rig's latitude and longitude.</p>}
      {weather.forecast_conditions.reason === 'forecast_location_unavailable' && <p role="status">The forecast location could not be checked. Set the planning site's coordinates under Settings → Presets → Sites and check the rig's location in its Site tab.</p>}
      <p>Clear at or under {weather.cloud_threshold_percent}% cloud. {weather.site_id
        ? <>The planning site sets the forecast, cloud limit and dew limit. <button type="button" className="link-button" onClick={() => openSettings('sites', { field: weather.site_id ?? undefined })}>Edit site weather</button></>
        : 'Choose a planning site in the Site tab to set the forecast, cloud limit and dew limit.'}</p>
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
      {weather.forecast && forecastNow !== null && <p className="director-muted">Forecast fetched {formatAge(forecastNow - weather.forecast.fetched_at_ms)}</p>}
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
