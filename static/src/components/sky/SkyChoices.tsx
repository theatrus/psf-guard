import type { SkyFrame, SkyMode } from '../../utils/skyProjection';
import './SkyChoices.css';

/** One choice among a few, as a row of chips: a radio group. */
export function SkyChoice<T extends string>({ label, options, value, onChange }: { label: string; options: ReadonlyArray<readonly [T, string]>; value: T; onChange: (next: T) => void }) {
  return (
    <div className="sky-control-group" role="radiogroup" aria-label={label}>
      {options.map(([option, text]) => (
        <button key={option} type="button" role="radio" aria-checked={value === option} className="sky-chip" onClick={() => onChange(option)}>
          {text}
        </button>
      ))}
    </div>
  );
}

const FRAMES: ReadonlyArray<readonly [SkyFrame, string]> = [['equatorial', 'Equatorial'], ['galactic', 'Galactic']];
const SHAPES: ReadonlyArray<readonly [SkyMode, string]> = [['aitoff', 'Flat'], ['globe', 'Globe']];

/** The coordinate frame and the map's shape: the two choices the Sky view
 *  offers for its map. */
export function SkyShapeChoices({ frame, mode, onFrame, onMode, shapeFirst = false }: { frame: SkyFrame; mode: SkyMode; onFrame: (next: SkyFrame) => void; onMode: (next: SkyMode) => void; shapeFirst?: boolean }) {
  const frames = <SkyChoice label="Coordinate frame" options={FRAMES} value={frame} onChange={onFrame} />;
  const shapes = <SkyChoice label="Map shape" options={SHAPES} value={mode} onChange={onMode} />;
  return shapeFirst ? <>{shapes}{frames}</> : <>{frames}{shapes}</>;
}
