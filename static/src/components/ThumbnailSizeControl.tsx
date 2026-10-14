import { Image as ImageIcon } from 'lucide-react';
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
  /** A small picture at each end of the slider, for a toolbar that shows
   *  no caption; the name becomes its tooltip. */
  icons?: boolean;
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
  icons = false,
}: ThumbnailSizeControlProps) {
  const shown = valueText ?? `${value}px`;
  return (
    <div className="size-control compact" title={icons ? `${label}: ${shown}` : undefined}>
      <label htmlFor={id}>{label}</label>
      {icons && <ImageIcon size={12} aria-hidden="true" />}
      <input
        id={id}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      {icons && <ImageIcon size={16} aria-hidden="true" />}
      <output htmlFor={id} className="size-value">{shown}</output>
    </div>
  );
}
