import type { Image, ImageQualityResult, StarSource } from '../api/types';
import { STAR_SOURCE_LABELS } from '../utils/scoring';
import { formatReading, starReadings, type StarMetric, type StarReadings } from '../utils/starReadings';

const MARKS: Record<StarSource, { letter: string; title: string }> = {
  capture: { letter: 'N', title: 'Recorded by N.I.N.A. during capture' },
  scan: { letter: 'P', title: "Measured by PSF Guard's quality scan" },
};


/** The superscript letter that says where a star count or HFR came from:
 *  N for N.I.N.A., P for PSF Guard. Screen readers hear the source's name. */
export function StarSourceMark({ source }: { source: StarSource }) {
  const mark = MARKS[source];
  return (
    <>
      <sup className="star-source-mark" title={mark.title} aria-hidden="true">{mark.letter}</sup>
      <span className="visually-hidden"> ({STAR_SOURCE_LABELS[source]})</span>
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

/** HFR and star rows: every source's reading, marked, the one scoring used
 *  standing out. Before scoring arrives, the catalog's value. */
export function StarReadingRows({ image, quality }: { image: Image; quality?: ImageQualityResult | null }) {
  return (
    <>
      {(['hfr', 'stars'] as const).map((metric) => {
        const readings = starReadings(metric, quality, image);
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
