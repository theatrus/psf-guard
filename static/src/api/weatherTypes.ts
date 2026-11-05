import type { ForecastHour, ForecastProvider } from './directorTypes';

export interface WeatherValues {
  cloud_percent: number | null;
  temperature_celsius: number | null;
  dew_point_celsius: number | null;
  humidity_percent: number | null;
  pressure_hpa: number | null;
  rain_mm_per_hour: number | null;
  sky_brightness_lux: number | null;
  sky_quality_mag_arcsec2: number | null;
  sky_temperature_celsius: number | null;
  seeing_arcsec: number | null;
  wind_direction_degrees: number | null;
  wind_speed_m_s: number | null;
  wind_gust_m_s: number | null;
}
export interface WeatherObservation {
  rig_id: string;
  received_at_ms: number;
  sample: {
    sample_id: string;
    profile_id: string;
    reported_at_ms: number;
    source_name: string;
    source_key: string;
    connected: boolean;
    values: WeatherValues;
  };
}
export interface CloudAssessment {
  state: 'clear' | 'cloudy' | 'dewy' | 'unknown';
  reason: string;
  valid_for_ms: number;
}
export interface RigWeather {
  rig_id: string;
  site_id: string | null;
  evaluated_at_ms: number;
  cloud_threshold_percent: number;
  dew_limit_celsius?: number | null;
  reported: WeatherObservation | null;
  forecast: {
    provider: ForecastProvider;
    attribution: string;
    fetched_at_ms: number;
    expires_at_ms: number;
    hour: ForecastHour | null;
    fresh: boolean;
  } | null;
  reported_cloud: CloudAssessment;
  forecast_conditions: CloudAssessment;
}
