import {
  THUMBNAIL_SIZE_MAX,
  THUMBNAIL_SIZE_MIN,
  THUMBNAIL_SIZE_STEP,
} from '../utils/thumbnailSizing';

interface ThumbnailSizeControlProps {
  id: string;
  value: number;
  onChange: (value: number) => void;
}

export default function ThumbnailSizeControl({
  id,
  value,
  onChange,
}: ThumbnailSizeControlProps) {
  return (
    <div className="size-control compact">
      <label htmlFor={id}>Size:</label>
      <input
        id={id}
        type="range"
        min={THUMBNAIL_SIZE_MIN}
        max={THUMBNAIL_SIZE_MAX}
        step={THUMBNAIL_SIZE_STEP}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <output htmlFor={id} className="size-value">{value}px</output>
    </div>
  );
}
