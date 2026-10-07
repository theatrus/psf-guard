import {
  THUMBNAIL_SIZE_MAX,
  THUMBNAIL_SIZE_MIN,
  THUMBNAIL_SIZE_STEP,
} from '../utils/thumbnailSizing';

interface ThumbnailSizeControlProps {
  id: string;
  value: number;
  onChange: (value: number) => void;
  /** What the slider sizes, as a screen reader names it. */
  label?: string;
  /** The grid's thumbnail range unless a page sizes something else. */
  min?: number;
  max?: number;
  step?: number;
  /** Said in place of the pixel width, such as "Fit". */
  valueText?: string;
}

export default function ThumbnailSizeControl({
  id,
  value,
  onChange,
  label = 'Size',
  min = THUMBNAIL_SIZE_MIN,
  max = THUMBNAIL_SIZE_MAX,
  step = THUMBNAIL_SIZE_STEP,
  valueText,
}: ThumbnailSizeControlProps) {
  return (
    <div className="size-control compact">
      <label htmlFor={id}>{label}:</label>
      <input
        id={id}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <output htmlFor={id} className="size-value">{valueText ?? `${value}px`}</output>
    </div>
  );
}
