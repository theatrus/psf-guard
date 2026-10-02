import {
  THUMBNAIL_SIZE_MAX,
  THUMBNAIL_SIZE_MIN,
  THUMBNAIL_SIZE_STEP,
} from '../utils/thumbnailSizing';

interface ThumbnailSizeControlProps {
  id: string;
  value: number;
  onChange: (value: number) => void;
  /** The grid's thumbnail range unless a page sizes something else. */
  min?: number;
  max?: number;
  step?: number;
}

export default function ThumbnailSizeControl({
  id,
  value,
  onChange,
  min = THUMBNAIL_SIZE_MIN,
  max = THUMBNAIL_SIZE_MAX,
  step = THUMBNAIL_SIZE_STEP,
}: ThumbnailSizeControlProps) {
  return (
    <div className="size-control compact">
      <label htmlFor={id}>Size:</label>
      <input
        id={id}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <output htmlFor={id} className="size-value">{value}px</output>
    </div>
  );
}
