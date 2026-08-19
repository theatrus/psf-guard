import { useMemo } from 'react';
import type { ProgressiveSnr, SnrVerdict } from '../api/types';

interface StackSnrCurveProps {
  curve: ProgressiveSnr;
  /** Names the chart for a screen reader. */
  label: string;
}

const VERDICT_COPY: Record<SnrVerdict, string> = {
  improving: 'Still improving',
  diminishing: 'Diminishing returns',
  plateau: 'Plateau',
  degrading: 'Getting worse',
};

const WIDTH = 320;
const HEIGHT = 120;
const PADDING = { top: 8, right: 8, bottom: 18, left: 30 };

function formatHours(seconds: number) {
  const hours = seconds / 3600;
  return hours >= 10 ? `${Math.round(hours)} h` : `${hours.toFixed(1)} h`;
}

/**
 * The progressive signal-to-noise curve of one stack.
 *
 * Both axes are logarithmic, because perfect averaging is a straight line
 * there and nothing else makes "am I still on the ideal?" a question you can
 * answer by looking. The dashed line is that ideal, anchored on the first
 * measured depth; the solid line is what the frames actually did. The gap
 * between them is the whole reading.
 */
export default function StackSnrCurve({ curve, label }: StackSnrCurveProps) {
  const points = curve.points.filter((point) => point.frames >= 1 && point.snr > 0);
  const geometry = useMemo(() => {
    if (points.length < 2) return null;
    const xs = points.map((point) => Math.log(point.frames));
    const first = points[0];
    // The ideal runs from the first measured depth at the square-root rate.
    const ideal = points.map((point) => first.snr * Math.sqrt(point.frames / first.frames));
    const ys = [...points.map((point) => point.snr), ...ideal].map(Math.log);
    const [minX, maxX] = [Math.min(...xs), Math.max(...xs)];
    const [minY, maxY] = [Math.min(...ys), Math.max(...ys)];
    const spanX = maxX - minX || 1;
    const spanY = maxY - minY || 1;
    const plotWidth = WIDTH - PADDING.left - PADDING.right;
    const plotHeight = HEIGHT - PADDING.top - PADDING.bottom;
    const toX = (frames: number) =>
      PADDING.left + ((Math.log(frames) - minX) / spanX) * plotWidth;
    const toY = (snr: number) =>
      PADDING.top + plotHeight - ((Math.log(snr) - minY) / spanY) * plotHeight;
    const path = points.map((point) => `${toX(point.frames)},${toY(point.snr)}`).join(' ');
    const idealPath = points
      .map((point, index) => `${toX(point.frames)},${toY(ideal[index])}`)
      .join(' ');
    return {
      path,
      idealPath,
      marks: points.map((point) => ({
        x: toX(point.frames),
        y: toY(point.snr),
        frames: point.frames,
        snr: point.snr,
        seconds: point.exposure_seconds,
      })),
    };
  }, [points]);

  if (points.length < 2) return null;
  const analysis = curve.analysis;
  const verdict = analysis?.verdict;

  return (
    <section className="stack-snr">
      <div className="stack-snr-head">
        <strong>Signal-to-noise vs depth</strong>
        <span className="stack-snr-order">{curve.order} order</span>
        {verdict && <span className={`stack-snr-verdict ${verdict}`}>{VERDICT_COPY[verdict]}</span>}
      </div>

      {geometry && (
        <svg
          className="stack-snr-chart"
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          role="img"
          aria-label={`${label}: signal-to-noise ratio against frame count, on log axes`}
        >
          <polyline className="stack-snr-ideal" points={geometry.idealPath} />
          <polyline className="stack-snr-measured" points={geometry.path} />
          {geometry.marks.map((mark) => (
            <circle key={mark.frames} cx={mark.x} cy={mark.y} r={2.5}>
              <title>
                {`${mark.frames} frames (${formatHours(mark.seconds)}): ratio ${mark.snr.toFixed(1)}`}
              </title>
            </circle>
          ))}
          <text className="stack-snr-axis" x={PADDING.left} y={HEIGHT - 5}>
            {geometry.marks[0].frames}
          </text>
          <text
            className="stack-snr-axis"
            x={WIDTH - PADDING.right}
            y={HEIGHT - 5}
            textAnchor="end"
          >
            {geometry.marks[geometry.marks.length - 1].frames} frames
          </text>
          <text className="stack-snr-axis" x={2} y={PADDING.top + 8}>
            SNR
          </text>
        </svg>
      )}

      {analysis ? (
        <>
          <p className="stack-snr-summary">{analysis.summary}</p>
          <div className="stack-snr-metrics">
            <div>
              <strong>{analysis.noise_exponent.toFixed(2)}</strong>
              <span>noise exponent (ideal -0.50)</span>
            </div>
            <div>
              <strong>{Math.round(analysis.efficiency * 100)}%</strong>
              <span>of ideal averaging</span>
            </div>
            {analysis.frames_for_90_percent !== null && (
              <div>
                <strong>{analysis.frames_for_90_percent}</strong>
                <span>frames for 90% of the ratio</span>
              </div>
            )}
            {analysis.projections.map((projection) => (
              <div key={projection.gain}>
                <strong>+{projection.extra_frames}</strong>
                <span>
                  {`frames (${formatHours(projection.extra_seconds)}) for ${Math.round(
                    (projection.gain - 1) * 100
                  )}% more`}
                </span>
              </div>
            ))}
          </div>
          {analysis.regressions.length > 0 && (
            <p className="stack-snr-regression">
              {`Noise rose across ${analysis.regressions.length} span${
                analysis.regressions.length === 1 ? '' : 's'
              }: ${analysis.regressions
                .map(
                  (regression) =>
                    `${regression.from_frames}→${regression.to_frames} frames (+${Math.round(
                      regression.noise_increase * 100
                    )}%)`
                )
                .join(', ')}. ${
                curve.order === 'quality'
                  ? 'In quality order that is where the weaker frames stop paying for themselves.'
                  : 'Those frames made the stack noisier.'
              }`}
            </p>
          )}
        </>
      ) : (
        <p className="stack-snr-summary">
          Measured at {points.length} depth{points.length === 1 ? '' : 's'}. Three are needed
          before a trend can be read.
        </p>
      )}
    </section>
  );
}
