import type { ForecastProvider } from '../../api/directorTypes';

export const PROVIDER_LABEL: Record<ForecastProvider, string> = {
  open_meteo: 'Open-Meteo',
  met_norway: 'MET Norway',
};

/** The provider's own page, which every credit links to. */
export const PROVIDER_URL: Record<ForecastProvider, string> = {
  open_meteo: 'https://open-meteo.com/',
  met_norway: 'https://api.met.no/',
};

/** The threshold a site starts with, as the server's. */
export const DEFAULT_CLOUD_THRESHOLD = 30;

/** The largest dew limit, in °C, as the server's. */
export const MAX_DEW_LIMIT = 20;
