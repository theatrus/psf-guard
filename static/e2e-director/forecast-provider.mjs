#!/usr/bin/env node
// A stand-in for Open-Meteo, for the Director suite: the same cloud for a
// given night at a given longitude every run, so checks and screenshots do
// not depend on the weather. Nights cycle: clear, overcast, then clear only
// from 21:00 to 01:00 local mean time.
//
// Usage: node e2e-director/forecast-provider.mjs <port>

import http from 'node:http';

const HOUR = 3_600_000;
const DAY = 24 * HOUR;

/** Total cloud cover, in percent, for the hour starting at `t`. */
export function cover(t, longitudeDegrees) {
  const local = t + (longitudeDegrees / 15) * HOUR;
  const night = Math.floor((local - 12 * HOUR) / DAY);
  const hour = new Date(local).getUTCHours();
  switch (((night % 3) + 3) % 3) {
    case 0: return 5;
    case 1: return 95;
    default: return hour >= 21 || hour < 1 ? 10 : 80;
  }
}

const port = Number(process.argv[2]);
if (!Number.isInteger(port) || port <= 0) {
  console.error('Usage: forecast-provider.mjs <port>');
  process.exit(2);
}

http.createServer((request, response) => {
  const url = new URL(request.url ?? '/', 'http://localhost');
  if (url.pathname === '/health') {
    response.end('ok');
    return;
  }
  if (url.pathname !== '/v1/forecast') {
    response.statusCode = 404;
    response.end();
    return;
  }
  const longitude = Number(url.searchParams.get('longitude') ?? 0);
  // From half a day back, for 16 days, as Open-Meteo's hourly forecast.
  const start = Math.floor(Date.now() / HOUR) * HOUR - 12 * HOUR;
  const hourly = { time: [], cloud_cover: [], cloud_cover_low: [], cloud_cover_mid: [], cloud_cover_high: [] };
  for (let t = start; t < start + 16 * DAY; t += HOUR) {
    const total = cover(t, longitude);
    hourly.time.push(t / 1000);
    hourly.cloud_cover.push(total);
    hourly.cloud_cover_low.push(total > 50 ? total - 10 : 0);
    hourly.cloud_cover_mid.push(total > 50 ? 40 : 0);
    hourly.cloud_cover_high.push(Math.min(total, 10));
  }
  response.setHeader('content-type', 'application/json');
  response.end(JSON.stringify({ hourly }));
}).listen(port, '127.0.0.1');
