import PillSelect from './toolbar/PillSelect';
import {
  CUTOFF_METRICS,
  cutoffMetricOf,
  formatCutoff,
  parseCutoffMetric,
  type CutoffMetric,
  type CutoffScale,
} from '../utils/sequenceCutoff';

/**
 * The Sequence view's cutoff: what it measures, and a slider over that
 * measure's range, 0 to 100 for the score and the readings on screen for
 * stars and HFR. The chart and the Below the cutoff rule follow it.
 */
export default function SequenceCutoffControl({
  metric,
  scale,
  value,
  onMetricChange,
  onChange,
}: {
  metric: CutoffMetric;
  /** Null while no frame on screen has a reading in this measure. */
  scale: CutoffScale | null;
  value: number | null;
  onMetricChange: (metric: CutoffMetric) => void;
  onChange: (value: number) => void;
}) {
  const info = cutoffMetricOf(metric);
  const side = info.worse === 'below' ? 'under' : 'over';
  return (
    <span
      className="toolbar-range sequence-cutoff"
      title={`Frames ${side} the cutoff are marked; the ${info.ruleLabel} rule selects them`}
    >
      <PillSelect
        label="Cutoff"
        aria-label="Cutoff measure"
        id="sequence-cutoff-metric"
        active={metric !== 'score'}
        value={metric}
        onChange={(event) => onMetricChange(parseCutoffMetric(event.target.value))}
      >
        {CUTOFF_METRICS.map((option) => (
          <option key={option.id} value={option.id}>{option.label}</option>
        ))}
      </PillSelect>
      {scale && value !== null ? (
        <>
          <input
            id="sequence-threshold"
            aria-label={`${info.label} cutoff`}
            type="range"
            min={scale.min}
            max={scale.max}
            step={scale.step}
            value={value}
            onChange={(event) => onChange(parseFloat(event.target.value))}
          />
          <output htmlFor="sequence-threshold">{formatCutoff(metric, value)}</output>
        </>
      ) : (
        <span className="sequence-cutoff-empty">No {info.label} readings</span>
      )}
    </span>
  );
}
