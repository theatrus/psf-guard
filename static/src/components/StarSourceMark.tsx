import type { Image, StarSource } from '../api/types';
import { formatReading, starReadings, type StarMetric, type StarReadings } from '../utils/starReadings';

const MARKS: Record<StarSource, { letter: string; name: string; title: string }> = {
  capture: { letter: 'N', name: 'N.I.N.A.', title: 'Recorded by N.I.N.A. during capture' },
  scan: { letter: 'P', name: 'PSF Guard', title: "Measured by PSF Guard's quality scan" },
};

/** The superscript letter that says where a star count or HFR came from:
 *  N for N.I.N.A., P for PSF Guard. Screen readers hear the source's name. */
export function StarSourceMark({ source }: { source: StarSource }) {
  const mark = MARKS[source];
  return (
    <>
      <sup className="star-source-mark" title={mark.title} aria-hidden="true">{mark.letter}</sup>
      <span className="visually-hidden"> ({mark.name})</span>
    </>
  );
}

/** Every source's reading with its mark, N.I.N.A.'s first. The one scoring
 *  used stands out when there are two. */
export function StarReadingList({ metric, readings }: { metric: StarMetric; readings: StarReadings }) {
  const sources = (['capture', 'scan'] as const).filter((source) => readings[source] !== null);
  if (sources.length === 0) return null;
  return (
    <span className="star-readings">
      {sources.map((source) => (
        <span
          key={source}
          className={`star-reading${sources.length > 1 && readings.used !== source ? ' is-unused' : ''}`}
        >
          {formatReading(metric, readings[source]!)}
          <StarSourceMark source={source} />
        </span>
      ))}
    </span>
  );
}

/** HFR and star rows for a view without scoring results: the catalog's
 *  value, marked with the source that wrote it. */
export function StarReadingRows({ image }: { image: Image }) {
  return (
    <>
      {(['hfr', 'stars'] as const).map((metric) => {
        const readings = starReadings(metric, undefined, image);
        if (readings.used === null) return null;
        return (
          <div key={metric} className="info-row">
            <strong>{metric === 'hfr' ? 'HFR:' : 'Stars:'}</strong>{' '}
            <StarReadingList metric={metric} readings={readings} />
          </div>
        );
      })}
    </>
  );
}
